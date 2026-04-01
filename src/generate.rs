use anyhow::Result;
use serde_json::Value;

use crate::cli::Args;
use crate::discovery::ResolvedResource;
use crate::interactive;
use crate::schema::{FieldSchema, FieldType, ResourceSchema};

// ---------------------------------------------------------------------------
// YAML scaffold tree
// ---------------------------------------------------------------------------

/// A node in the YAML scaffold tree.  Built from the schema, then emitted as
/// YAML text in a second pass.  Separating construction from emission keeps
/// both halves simple and makes indentation/array-item formatting automatic.
enum YamlNode {
    /// A scalar value, already formatted for YAML (e.g. `"\"hello\""`, `"0"`).
    Scalar(String),
    /// An ordered mapping: `(key, value, optional_description)`.
    Object(Vec<(String, YamlNode, Option<String>)>),
    /// A sequence with one example item.
    Array(Vec<YamlNode>),
    /// A free-form map — rendered as a `# key: value` placeholder comment.
    Map,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Generate YAML for the resource using its schema and smart defaults.
pub fn generate_yaml(
    resolved: &ResolvedResource,
    schema: &ResourceSchema,
    args: &Args,
) -> Result<String> {
    let name = args.name.as_deref().unwrap_or("my-resource");
    let comments = !args.no_comments;
    let full = args.full;
    let minimal = args.minimal;

    // --- Build document tree ------------------------------------------------
    let mut entries: Vec<(String, YamlNode, Option<String>)> = Vec::new();

    // apiVersion
    let api_version = if resolved.group.is_empty() {
        resolved.version.clone()
    } else {
        format!("{}/{}", resolved.group, resolved.version)
    };
    entries.push(("apiVersion".into(), YamlNode::Scalar(api_version), None));

    // kind
    entries.push((
        "kind".into(),
        YamlNode::Scalar(resolved.api_resource.kind.clone()),
        None,
    ));

    // metadata
    let mut meta = vec![("name".into(), YamlNode::Scalar(name.into()), None)];
    if let Some(ref ns) = args.namespace {
        if resolved.namespaced {
            meta.push(("namespace".into(), YamlNode::Scalar(ns.clone()), None));
        }
    } else if resolved.namespaced {
        meta.push(("namespace".into(), YamlNode::Scalar("default".into()), None));
    }
    entries.push(("metadata".into(), YamlNode::Object(meta), None));

    // Spec-level fields (skip apiVersion, kind, metadata, status)
    let fields_to_render: Vec<&FieldSchema> = schema
        .fields
        .iter()
        .filter(|f| {
            !matches!(
                f.name.as_str(),
                "apiVersion" | "kind" | "metadata" | "status"
            )
        })
        .filter(|f| if minimal { f.required } else { true })
        .collect();

    // Interactive values: -i alone prompts required only, -i --full prompts all
    let interactive_values = if args.interactive {
        Some(interactive::prompt_for_fields(&fields_to_render, !full)?)
    } else {
        None
    };

    for field in &fields_to_render {
        let iv = interactive_values.as_ref().and_then(|m| m.get(&field.name));
        let node = build_node(field, full, minimal, iv);
        let desc = field
            .description
            .as_ref()
            .map(|d| first_sentence(d).to_string());
        entries.push((field.name.clone(), node, desc));
    }

    // --- Emit YAML ----------------------------------------------------------
    let yaml = emit_yaml(&YamlNode::Object(entries), comments);
    Ok(format!("---\n{}", yaml))
}

// ---------------------------------------------------------------------------
// Tree builder: FieldSchema -> YamlNode
// ---------------------------------------------------------------------------

fn build_node(
    field: &FieldSchema,
    full: bool,
    minimal: bool,
    interactive_val: Option<&Value>,
) -> YamlNode {
    // Interactive value overrides everything
    if let Some(val) = interactive_val {
        return value_to_node(val);
    }

    // Schema default — skip empty {} / [] when we have structural sub-fields
    if let Some(ref default) = field.default {
        let is_empty = matches!(default, Value::Object(m) if m.is_empty())
            || matches!(default, Value::Array(a) if a.is_empty());
        let is_structured = matches!(&field.field_type, FieldType::Object(f) if !f.is_empty())
            || matches!(&field.field_type, FieldType::Array(_))
            || matches!(&field.field_type, FieldType::Map(Some(_)));
        if !is_empty || !is_structured {
            return YamlNode::Scalar(format_value(default));
        }
    }

    // Enum: use first value
    if let Some(ref enum_vals) = field.enum_values
        && let Some(first) = enum_vals.first()
    {
        return YamlNode::Scalar(format_value(first));
    }

    // Type-based scaffold
    match &field.field_type {
        FieldType::String => {
            let s = match field.format.as_deref() {
                Some("date-time") => "\"2025-01-01T00:00:00Z\"",
                _ => "\"\"",
            };
            YamlNode::Scalar(s.into())
        }
        FieldType::Integer => YamlNode::Scalar("0".into()),
        FieldType::Number => YamlNode::Scalar("0.0".into()),
        FieldType::Boolean => YamlNode::Scalar("false".into()),
        FieldType::Array(item_schema) => {
            let item = build_node(item_schema, full, minimal, None);
            YamlNode::Array(vec![item])
        }
        FieldType::Object(sub_fields) => {
            if sub_fields.is_empty() {
                return YamlNode::Scalar("{}".into());
            }
            build_object_node(sub_fields, full, minimal)
        }
        FieldType::Map(_) => YamlNode::Map,
        FieldType::Any => YamlNode::Scalar("null".into()),
    }
}

/// Build an Object node from a list of sub-fields, applying the
/// minimal / full / commonly-needed filter.
fn build_object_node(sub_fields: &[FieldSchema], full: bool, minimal: bool) -> YamlNode {
    let shown = filter_fields(sub_fields, minimal, full);
    let entries = shown
        .into_iter()
        .map(|f| {
            let node = build_node(f, full, minimal, None);
            let desc = f
                .description
                .as_ref()
                .map(|d| first_sentence(d).to_string());
            (f.name.clone(), node, desc)
        })
        .collect();
    YamlNode::Object(entries)
}

/// Convert a serde_json::Value (from interactive input) into a YamlNode.
fn value_to_node(value: &Value) -> YamlNode {
    match value {
        Value::Object(map) => {
            let entries = map
                .iter()
                .map(|(k, v)| (k.clone(), value_to_node(v), None))
                .collect();
            YamlNode::Object(entries)
        }
        Value::Array(arr) => YamlNode::Array(arr.iter().map(value_to_node).collect()),
        other => YamlNode::Scalar(format_value(other)),
    }
}

// ---------------------------------------------------------------------------
// YAML emitter: YamlNode -> String
// ---------------------------------------------------------------------------

fn emit_yaml(node: &YamlNode, comments: bool) -> String {
    let mut lines = Vec::new();
    if let YamlNode::Object(entries) = node {
        for (key, value, desc) in entries {
            emit_entry(&mut lines, key, value, desc.as_deref(), 0, comments);
        }
    }
    lines.join("\n")
}

/// Emit a single key-value entry at the given indentation level.
fn emit_entry(
    lines: &mut Vec<String>,
    key: &str,
    value: &YamlNode,
    desc: Option<&str>,
    indent: usize,
    comments: bool,
) {
    let pfx = "  ".repeat(indent);
    let cmt = inline_comment(desc, comments);

    match value {
        YamlNode::Scalar(s) => {
            lines.push(format!("{}{}: {}{}", pfx, key, s, cmt));
        }
        YamlNode::Object(entries) if entries.is_empty() => {
            lines.push(format!("{}{}: {{}}{}", pfx, key, cmt));
        }
        YamlNode::Object(entries) => {
            lines.push(format!("{}{}:{}", pfx, key, cmt));
            for (k, v, d) in entries {
                emit_entry(lines, k, v, d.as_deref(), indent + 1, comments);
            }
        }
        YamlNode::Array(items) if items.is_empty() => {
            lines.push(format!("{}{}: []{}", pfx, key, cmt));
        }
        YamlNode::Array(items) => {
            lines.push(format!("{}{}:{}", pfx, key, cmt));
            for item in items {
                emit_array_item(lines, item, indent + 1, comments);
            }
        }
        YamlNode::Map => {
            lines.push(format!("{}{}:{}", pfx, key, cmt));
            let inner = "  ".repeat(indent + 1);
            lines.push(format!("{}# key: value", inner));
        }
    }
}

/// Emit an array item (the `- …` line(s)).
fn emit_array_item(lines: &mut Vec<String>, item: &YamlNode, indent: usize, comments: bool) {
    let pfx = "  ".repeat(indent.saturating_sub(1));

    match item {
        YamlNode::Scalar(s) => {
            lines.push(format!("{}- {}", pfx, s));
        }
        YamlNode::Object(entries) if entries.is_empty() => {
            lines.push(format!("{}- {{}}", pfx));
        }
        YamlNode::Object(entries) => {
            // Reorder: scalar fields first for clean `- key: value` syntax,
            // then complex fields (objects, arrays, maps) below.
            let (scalars, complex): (Vec<_>, Vec<_>) = entries
                .iter()
                .partition(|(_, v, _)| matches!(v, YamlNode::Scalar(_)));
            let ordered: Vec<_> = scalars.into_iter().chain(complex).collect();

            for (i, (key, value, desc)) in ordered.iter().enumerate() {
                let cmt = inline_comment(desc.as_deref(), comments);

                if i == 0 {
                    emit_first_array_field(lines, key, value, &cmt, indent, comments);
                } else {
                    emit_entry(lines, key, value, desc.as_deref(), indent, comments);
                }
            }
        }
        YamlNode::Array(items) => {
            // Array of arrays — unusual but handle it
            for sub in items {
                emit_array_item(lines, sub, indent, comments);
            }
        }
        YamlNode::Map => {
            lines.push(format!("{}- {{}}", pfx));
        }
    }
}

/// Emit the first field of an object that lives inside an array.
/// Scalar fields get the compact `- key: value` form.
/// Complex fields get `- key:\n  <sub-structure>`.
fn emit_first_array_field(
    lines: &mut Vec<String>,
    key: &str,
    value: &YamlNode,
    cmt: &str,
    indent: usize,
    comments: bool,
) {
    let pfx = "  ".repeat(indent.saturating_sub(1));

    match value {
        YamlNode::Scalar(s) => {
            lines.push(format!("{}- {}: {}{}", pfx, key, s, cmt));
        }
        YamlNode::Object(sub) if sub.is_empty() => {
            lines.push(format!("{}- {}: {{}}{}", pfx, key, cmt));
        }
        YamlNode::Object(sub) => {
            lines.push(format!("{}- {}:{}", pfx, key, cmt));
            for (sk, sv, sd) in sub {
                emit_entry(lines, sk, sv, sd.as_deref(), indent + 1, comments);
            }
        }
        YamlNode::Array(sub) if sub.is_empty() => {
            lines.push(format!("{}- {}: []{}", pfx, key, cmt));
        }
        YamlNode::Array(sub) => {
            lines.push(format!("{}- {}:{}", pfx, key, cmt));
            for sub_item in sub {
                emit_array_item(lines, sub_item, indent + 1, comments);
            }
        }
        YamlNode::Map => {
            lines.push(format!("{}- {}:{}", pfx, key, cmt));
            let inner = "  ".repeat(indent + 1);
            lines.push(format!("{}# key: value", inner));
        }
    }
}

/// Format an optional description as an inline YAML comment.
fn inline_comment(desc: Option<&str>, comments: bool) -> String {
    if comments {
        desc.map(|d| format!("  # {}", d)).unwrap_or_default()
    } else {
        String::new()
    }
}

// ---------------------------------------------------------------------------
// Field filtering
// ---------------------------------------------------------------------------

/// Select which sub-fields to show.  When nothing matches the filter we fall
/// back to showing ALL sub-fields so we never discard known schema structure.
fn filter_fields(sub_fields: &[FieldSchema], minimal: bool, full: bool) -> Vec<&FieldSchema> {
    let filtered: Vec<&FieldSchema> = sub_fields
        .iter()
        .filter(|f| {
            if minimal {
                f.required
            } else if full {
                true
            } else {
                f.required || is_commonly_needed(f)
            }
        })
        .collect();
    if filtered.is_empty() {
        sub_fields.iter().collect()
    } else {
        filtered
    }
}

/// Heuristic: is this field commonly needed even if not required?
fn is_commonly_needed(field: &FieldSchema) -> bool {
    matches!(
        field.name.as_str(),
        // Core identifiers & config
        "name"
            | "image"
            | "containerPort"
            | "port"
            | "protocol"
            | "replicas"
            | "selector"
            | "template"
            | "containers"
            | "ports"
            | "env"
            | "resources"
            | "labels"
            | "annotations"
            | "type"
            | "data"
            | "stringData"
            // Env detail
            | "value"
            | "valueFrom"
            // Resource limits
            | "limits"
            | "requests"
            | "claims"
            | "request"
            // Container execution
            | "command"
            | "args"
            // Volumes
            | "volumeMounts"
            | "mountPath"
            | "volumes"
            // RBAC
            | "rules"
            | "subjects"
            | "roleRef"
            // Structural
            | "spec"
            // NetworkPolicy
            | "podSelector"
            | "policyTypes"
            | "ingress"
            | "egress"
            | "from"
            | "to"
            | "matchLabels"
            | "matchExpressions"
            // Ingress
            | "ingressClassName"
            | "tls"
            // PV / PVC
            | "accessModes"
            | "storageClassName"
            // StatefulSet
            | "serviceName"
            | "volumeClaimTemplates"
            // Jobs / CronJobs
            | "schedule"
            | "jobTemplate"
            // HPA
            | "minReplicas"
            | "maxReplicas"
            | "metrics"
    )
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn format_value(value: &Value) -> String {
    match value {
        Value::String(s) => format!("\"{}\"", s.replace('"', "\\\"")),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(arr) if arr.is_empty() => "[]".to_string(),
        Value::Object(map) if map.is_empty() => "{}".to_string(),
        // For complex defaults, fall back to inline JSON-ish
        _ => serde_json::to_string(value).unwrap_or_else(|_| "null".to_string()),
    }
}

/// Extract the first sentence from a description string, capped at 120 chars.
fn first_sentence(desc: &str) -> &str {
    let clean = desc.split('\n').next().unwrap_or(desc);
    let end = clean
        .find(". ")
        .map(|i| i + 1)
        .or_else(|| clean.find(".\n").map(|i| i + 1))
        .unwrap_or(clean.len());

    let result = &clean[..end];
    if result.len() > 120 {
        &result[..120]
    } else {
        result
    }
}
