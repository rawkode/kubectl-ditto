use anyhow::Result;
use serde_json::Value;

use crate::cli::Args;
use crate::discovery::ResolvedResource;
use crate::interactive;
use crate::schema::{FieldSchema, FieldType, ResourceSchema};

/// Generate YAML for the resource using its schema and smart defaults.
pub fn generate_yaml(
    resolved: &ResolvedResource,
    schema: &ResourceSchema,
    args: &Args,
) -> Result<String> {
    let name = args.name.as_deref().unwrap_or("my-resource");
    let include_comments = !args.no_comments;
    let include_optional = args.full;
    let minimal = args.minimal;

    let mut out = YamlWriter::new(include_comments);

    out.line("---");

    // apiVersion
    let api_version = if resolved.group.is_empty() {
        resolved.version.clone()
    } else {
        format!("{}/{}", resolved.group, resolved.version)
    };
    out.field("apiVersion", &api_version, None, 0);

    // kind
    out.field("kind", &resolved.api_resource.kind, None, 0);

    // metadata
    out.key("metadata", None, 0);
    out.field("name", name, None, 1);
    if let Some(ref ns) = args.namespace {
        if resolved.namespaced {
            out.field("namespace", ns, None, 1);
        }
    } else if resolved.namespaced {
        out.field("namespace", "default", None, 1);
    }

    // Collect fields to render (skip apiVersion, kind, metadata, status)
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

    // If interactive, prompt for values
    let interactive_values = if args.interactive {
        Some(interactive::prompt_for_fields(&fields_to_render)?)
    } else {
        None
    };

    for field in &fields_to_render {
        let interactive_val = interactive_values
            .as_ref()
            .and_then(|iv| iv.get(&field.name));

        write_field(
            &mut out,
            field,
            0,
            include_optional,
            minimal,
            interactive_val,
        );
    }

    Ok(out.finish())
}

fn write_field(
    out: &mut YamlWriter,
    field: &FieldSchema,
    indent: usize,
    include_optional: bool,
    minimal: bool,
    interactive_val: Option<&Value>,
) {
    // If we have an interactive value, use it directly
    if let Some(val) = interactive_val {
        write_value_at_key(out, &field.name, val, field.description.as_deref(), indent);
        return;
    }

    // Check for default value — skip empty object/array defaults when the field
    // has structured sub-fields (the sub-fields are more useful than "{}" or "[]")
    if let Some(ref default) = field.default {
        let is_empty_default = matches!(
            default,
            Value::Object(m) if m.is_empty()
        ) || matches!(
            default,
            Value::Array(a) if a.is_empty()
        );
        let has_sub_fields = matches!(&field.field_type, FieldType::Object(f) if !f.is_empty())
            || matches!(&field.field_type, FieldType::Array(_));

        if !is_empty_default || !has_sub_fields {
            write_value_at_key(
                out,
                &field.name,
                default,
                field.description.as_deref(),
                indent,
            );
            return;
        }
    }

    // Handle enum: use first value
    if let Some(ref enum_vals) = field.enum_values
        && let Some(first) = enum_vals.first()
    {
        let comment = field.description.as_ref().map(|d| {
            let options: Vec<String> = enum_vals
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            format!("{} (options: {})", d, options.join(", "))
        });
        write_value_at_key(
            out,
            &field.name,
            first,
            comment.as_deref().or(field.description.as_deref()),
            indent,
        );
        return;
    }

    match &field.field_type {
        FieldType::String => {
            let placeholder = if let Some(ref fmt) = field.format {
                match fmt.as_str() {
                    "date-time" => "\"2025-01-01T00:00:00Z\"",
                    "byte" => "\"\"",
                    _ => "\"\"",
                }
            } else {
                "\"\""
            };
            out.field_raw(
                &field.name,
                placeholder,
                field.description.as_deref(),
                indent,
            );
        }
        FieldType::Integer => {
            out.field_raw(&field.name, "0", field.description.as_deref(), indent);
        }
        FieldType::Number => {
            out.field_raw(&field.name, "0.0", field.description.as_deref(), indent);
        }
        FieldType::Boolean => {
            out.field_raw(&field.name, "false", field.description.as_deref(), indent);
        }
        FieldType::Array(item_schema) => {
            out.key(&field.name, field.description.as_deref(), indent);
            write_array_item(out, item_schema, indent + 1, include_optional, minimal);
        }
        FieldType::Object(sub_fields) => {
            if sub_fields.is_empty() {
                out.field_raw(&field.name, "{}", field.description.as_deref(), indent);
                return;
            }

            out.key(&field.name, field.description.as_deref(), indent);

            let fields_to_show: Vec<&FieldSchema> = sub_fields
                .iter()
                .filter(|f| {
                    if minimal {
                        f.required
                    } else if include_optional {
                        true
                    } else {
                        f.required || is_commonly_needed(f)
                    }
                })
                .collect();

            if fields_to_show.is_empty() {
                // Show at least one field or empty object
                if let Some(first) = sub_fields.first() {
                    write_field(out, first, indent + 1, include_optional, minimal, None);
                }
            } else {
                for sub in fields_to_show {
                    write_field(out, sub, indent + 1, include_optional, minimal, None);
                }
            }
        }
        FieldType::Map(_value_schema) => {
            out.key(&field.name, field.description.as_deref(), indent);
            out.comment("key: value", indent + 1);
        }
        FieldType::Any => {
            out.field_raw(&field.name, "null", field.description.as_deref(), indent);
        }
    }
}

fn write_array_item(
    out: &mut YamlWriter,
    item: &FieldSchema,
    indent: usize,
    include_optional: bool,
    minimal: bool,
) {
    match &item.field_type {
        FieldType::Object(sub_fields) => {
            let fields_to_show: Vec<&FieldSchema> = sub_fields
                .iter()
                .filter(|f| {
                    if minimal {
                        f.required
                    } else if include_optional {
                        true
                    } else {
                        f.required || is_commonly_needed(f)
                    }
                })
                .collect();

            let fields = if fields_to_show.is_empty() {
                sub_fields.iter().take(1).collect::<Vec<_>>()
            } else {
                fields_to_show
            };

            for (i, sub) in fields.iter().enumerate() {
                if i == 0 {
                    // First field gets the "- " prefix
                    out.array_item_key(&sub.name, sub.description.as_deref(), indent);
                    write_field_value_inline(out, sub, indent + 1, include_optional, minimal);
                } else {
                    write_field(out, sub, indent + 1, include_optional, minimal, None);
                }
            }
        }
        FieldType::String => {
            out.array_item_value("\"\"", indent);
        }
        FieldType::Integer => {
            out.array_item_value("0", indent);
        }
        _ => {
            out.array_item_value("{}", indent);
        }
    }
}

/// Write the value part of a field that was started as an array item key.
fn write_field_value_inline(
    out: &mut YamlWriter,
    field: &FieldSchema,
    _indent: usize,
    _include_optional: bool,
    _minimal: bool,
) {
    if let Some(ref default) = field.default {
        out.append_value(&format_value(default));
        return;
    }

    match &field.field_type {
        FieldType::String => out.append_value("\"\""),
        FieldType::Integer => out.append_value("0"),
        FieldType::Number => out.append_value("0.0"),
        FieldType::Boolean => out.append_value("false"),
        _ => out.append_value("{}"),
    }
}

fn write_value_at_key(
    out: &mut YamlWriter,
    key: &str,
    value: &Value,
    description: Option<&str>,
    indent: usize,
) {
    let formatted = format_value(value);
    out.field_raw(key, &formatted, description, indent);
}

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

/// Heuristic: is this field commonly needed even if not required?
fn is_commonly_needed(field: &FieldSchema) -> bool {
    matches!(
        field.name.as_str(),
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
            | "rules"
            | "subjects"
            | "roleRef"
            | "spec"
            // NetworkPolicy
            | "podSelector"
            | "policyTypes"
            | "ingress"
            | "egress"
            | "from"
            | "to"
            | "matchLabels"
            // Ingress
            | "ingressClassName"
            | "tls"
            // PV/PVC
            | "accessModes"
            | "storageClassName"
            // Jobs/CronJobs
            | "schedule"
            | "jobTemplate"
            // HPA
            | "minReplicas"
            | "maxReplicas"
            | "metrics"
    )
}

/// Custom YAML writer that supports inline comments.
struct YamlWriter {
    lines: Vec<String>,
    include_comments: bool,
}

impl YamlWriter {
    fn new(include_comments: bool) -> Self {
        Self {
            lines: Vec::new(),
            include_comments,
        }
    }

    /// Write a raw line.
    fn line(&mut self, text: &str) {
        self.lines.push(text.to_string());
    }

    /// Write a comment line.
    fn comment(&mut self, text: &str, indent: usize) {
        if self.include_comments {
            let prefix = "  ".repeat(indent);
            self.lines.push(format!("{}# {}", prefix, text));
        }
    }

    /// Write a key-value field with an optional description comment.
    fn field(&mut self, key: &str, value: &str, description: Option<&str>, indent: usize) {
        let prefix = "  ".repeat(indent);
        if let Some(desc) = description {
            if self.include_comments {
                // Truncate long descriptions to first sentence
                let short = first_sentence(desc);
                self.lines
                    .push(format!("{}{}: {}  # {}", prefix, key, value, short));
            } else {
                self.lines.push(format!("{}{}: {}", prefix, key, value));
            }
        } else {
            self.lines.push(format!("{}{}: {}", prefix, key, value));
        }
    }

    /// Write a key-value field with a raw (unquoted) value.
    fn field_raw(&mut self, key: &str, raw_value: &str, description: Option<&str>, indent: usize) {
        self.field(key, raw_value, description, indent);
    }

    /// Write just a key (for nested objects).
    fn key(&mut self, key: &str, description: Option<&str>, indent: usize) {
        let prefix = "  ".repeat(indent);
        if let Some(desc) = description {
            if self.include_comments {
                let short = first_sentence(desc);
                self.lines.push(format!("{}{}:  # {}", prefix, key, short));
            } else {
                self.lines.push(format!("{}{}:", prefix, key));
            }
        } else {
            self.lines.push(format!("{}{}:", prefix, key));
        }
    }

    /// Write an array item that is a key (first field of an object in an array).
    fn array_item_key(&mut self, key: &str, description: Option<&str>, indent: usize) {
        let prefix = "  ".repeat(indent.saturating_sub(1));
        if let Some(desc) = description {
            if self.include_comments {
                let short = first_sentence(desc);
                self.lines.push(format!("{}- {}: ", prefix, key));
                // Comment on previous line would be awkward, skip for array items
                let _ = short;
            } else {
                self.lines.push(format!("{}- {}: ", prefix, key));
            }
        } else {
            self.lines.push(format!("{}- {}: ", prefix, key));
        }
    }

    /// Append a value to the last line (for array item inline values).
    fn append_value(&mut self, value: &str) {
        if let Some(last) = self.lines.last_mut() {
            last.push_str(value);
        }
    }

    /// Write a simple array item value.
    fn array_item_value(&mut self, value: &str, indent: usize) {
        let prefix = "  ".repeat(indent.saturating_sub(1));
        self.lines.push(format!("{}- {}", prefix, value));
    }

    fn finish(self) -> String {
        self.lines.join("\n")
    }
}

/// Extract the first sentence from a description string.
fn first_sentence(desc: &str) -> &str {
    // Remove newlines, take first sentence
    let clean = desc.split('\n').next().unwrap_or(desc);
    let end = clean
        .find(". ")
        .map(|i| i + 1)
        .or_else(|| clean.find(".\n").map(|i| i + 1))
        .unwrap_or(clean.len());

    let result = &clean[..end];
    // Cap at reasonable length
    if result.len() > 120 {
        &result[..120]
    } else {
        result
    }
}
