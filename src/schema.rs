use anyhow::{bail, Result};
use kube::Client;
use openapiv3 as oa;
use serde_json::Value;

use crate::discovery::ResolvedResource;

/// A single field in the schema.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FieldSchema {
    pub name: String,
    pub description: Option<String>,
    pub field_type: FieldType,
    pub required: bool,
    pub default: Option<Value>,
    pub enum_values: Option<Vec<Value>>,
    /// For oneOf/anyOf, the possible variant schemas.
    pub variants: Option<Vec<FieldSchema>>,
    /// Format hint (e.g. "int32", "date-time", "int-or-string")
    pub format: Option<String>,
}

/// The type of a schema field.
#[derive(Debug, Clone)]
pub enum FieldType {
    String,
    Integer,
    Number,
    Boolean,
    Array(Box<FieldSchema>),
    Object(Vec<FieldSchema>),
    /// Free-form object (additionalProperties or no properties defined)
    Map(Option<Box<FieldSchema>>),
    /// Unknown/any
    Any,
}

/// The parsed schema for a resource.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ResourceSchema {
    pub fields: Vec<FieldSchema>,
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Fetch the raw OpenAPI schema definition for a resource (for debugging).
pub async fn fetch_raw_schema(
    client: &Client,
    resolved: &ResolvedResource,
) -> Result<Value> {
    let spec = fetch_gv_spec_json(client, resolved).await?;
    let schemas = spec
        .get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(|s| s.as_object())
        .ok_or_else(|| anyhow::anyhow!("No components/schemas in v3 spec"))?;

    let key = find_definition_key(&schemas.keys().cloned().collect::<Vec<_>>(), resolved)?;
    schemas
        .get(&key)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Definition not found: {}", key))
}

/// Fetch the schema for a resolved resource. Tries OpenAPI v3 first, falls back to v2.
pub async fn fetch_schema(
    client: &Client,
    resolved: &ResolvedResource,
) -> Result<ResourceSchema> {
    if let Ok(schema) = fetch_schema_v3(client, resolved).await {
        return Ok(schema);
    }
    fetch_schema_v2(client, resolved).await
}

// ---------------------------------------------------------------------------
// V3: fetch + openapiv3 deserialization + manual $ref resolution
// ---------------------------------------------------------------------------

/// Fetch the per-group-version OpenAPI v3 spec as raw JSON.
async fn fetch_gv_spec_json(
    client: &Client,
    resolved: &ResolvedResource,
) -> Result<Value> {
    let discovery: Value = client
        .request(
            http::Request::builder()
                .uri("/openapi/v3")
                .body(Default::default())?,
        )
        .await?;

    let paths = discovery
        .get("paths")
        .and_then(|p| p.as_object())
        .ok_or_else(|| anyhow::anyhow!("No paths in OpenAPI v3 discovery"))?;

    let gv_path = if resolved.group.is_empty() {
        "api/v1".to_string()
    } else {
        format!("apis/{}/{}", resolved.group, resolved.version)
    };

    let spec_info = paths
        .get(&gv_path)
        .ok_or_else(|| anyhow::anyhow!("No v3 spec for {}", gv_path))?;

    let spec_url = spec_info
        .get("serverRelativeURL")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("No serverRelativeURL"))?;

    let spec: Value = client
        .request(
            http::Request::builder()
                .uri(spec_url)
                .body(Default::default())?,
        )
        .await?;
    Ok(spec)
}

async fn fetch_schema_v3(
    client: &Client,
    resolved: &ResolvedResource,
) -> Result<ResourceSchema> {
    let spec_json = fetch_gv_spec_json(client, resolved).await?;

    // Deserialize into typed OpenAPI
    let openapi: oa::OpenAPI = serde_json::from_value(spec_json)?;

    let components = openapi
        .components
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No components in spec"))?;

    let key =
        find_definition_key(&components.schemas.keys().cloned().collect::<Vec<_>>(), resolved)?;

    let ctx = Ctx {
        schemas: &components.schemas,
    };

    let schema = ctx
        .resolve_ref_or(components.schemas.get(&key).ok_or_else(|| {
            anyhow::anyhow!("Definition not found: {}", key)
        })?)
        .ok_or_else(|| anyhow::anyhow!("Could not resolve definition: {}", key))?;

    Ok(ctx.resource_schema(schema))
}

// ---------------------------------------------------------------------------
// Schema context: holds the schemas map for $ref resolution
// ---------------------------------------------------------------------------

struct Ctx<'a> {
    schemas: &'a indexmap::IndexMap<String, oa::ReferenceOr<oa::Schema>>,
}

impl<'a> Ctx<'a> {
    /// Resolve a ReferenceOr<Schema> to &Schema.
    fn resolve_ref_or(&self, ref_or: &'a oa::ReferenceOr<oa::Schema>) -> Option<&'a oa::Schema> {
        match ref_or {
            oa::ReferenceOr::Item(s) => Some(s),
            oa::ReferenceOr::Reference { reference } => {
                let key = reference.strip_prefix("#/components/schemas/")?;
                match self.schemas.get(key)? {
                    oa::ReferenceOr::Item(s) => Some(s),
                    _ => None,
                }
            }
        }
    }

    /// Resolve a ReferenceOr<Schema> (non-boxed, e.g. in AdditionalProperties).
    fn resolve_ref_or_schema(
        &self,
        ref_or: &'a oa::ReferenceOr<oa::Schema>,
    ) -> Option<&'a oa::Schema> {
        match ref_or {
            oa::ReferenceOr::Item(s) => Some(s),
            oa::ReferenceOr::Reference { reference } => {
                let key = reference.strip_prefix("#/components/schemas/")?;
                match self.schemas.get(key)? {
                    oa::ReferenceOr::Item(s) => Some(s),
                    _ => None,
                }
            }
        }
    }

    /// Resolve a ReferenceOr<Box<Schema>> to &Schema.
    fn resolve_boxed(
        &self,
        ref_or: &'a oa::ReferenceOr<Box<oa::Schema>>,
    ) -> Option<&'a oa::Schema> {
        match ref_or {
            oa::ReferenceOr::Item(s) => Some(s.as_ref()),
            oa::ReferenceOr::Reference { reference } => {
                let key = reference.strip_prefix("#/components/schemas/")?;
                match self.schemas.get(key)? {
                    oa::ReferenceOr::Item(s) => Some(s),
                    _ => None,
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Schema → our types
    // -----------------------------------------------------------------------

    fn resource_schema(&self, schema: &'a oa::Schema) -> ResourceSchema {
        let description = schema.schema_data.description.clone();
        let (fields, _) = self.collect_object_fields(&schema.schema_kind, 0);
        ResourceSchema {
            fields,
            description,
        }
    }

    /// Collect object properties from a SchemaKind, flattening allOf and resolving $ref.
    fn collect_object_fields(
        &self,
        kind: &'a oa::SchemaKind,
        depth: usize,
    ) -> (Vec<FieldSchema>, Vec<String>) {
        if depth > 10 {
            return (vec![], vec![]);
        }
        match kind {
            oa::SchemaKind::Type(oa::Type::Object(obj)) => {
                let fields = obj
                    .properties
                    .iter()
                    .filter_map(|(name, prop_ref)| {
                        let schema = self.resolve_boxed(prop_ref)?;
                        let mut field = self.field(name, schema, depth + 1);
                        field.required = obj.required.contains(name);
                        Some(field)
                    })
                    .collect();
                (fields, obj.required.clone())
            }
            oa::SchemaKind::AllOf { all_of } => {
                let mut fields = Vec::new();
                let mut required = Vec::new();
                for item in all_of {
                    if let Some(schema) = self.resolve_ref_or(item) {
                        let (sub_fields, sub_req) =
                            self.collect_object_fields(&schema.schema_kind, depth + 1);
                        fields.extend(sub_fields);
                        required.extend(sub_req);
                    }
                }
                for f in &mut fields {
                    if required.contains(&f.name) {
                        f.required = true;
                    }
                }
                (fields, required)
            }
            oa::SchemaKind::Any(any) if !any.properties.is_empty() => {
                let fields = any
                    .properties
                    .iter()
                    .filter_map(|(name, prop_ref)| {
                        let schema = self.resolve_boxed(prop_ref)?;
                        let mut field = self.field(name, schema, depth + 1);
                        field.required = any.required.contains(name);
                        Some(field)
                    })
                    .collect();
                (fields, any.required.clone())
            }
            _ => (vec![], vec![]),
        }
    }

    fn field(&self, name: &str, schema: &'a oa::Schema, depth: usize) -> FieldSchema {
        if depth > 10 {
            return any_field(name);
        }

        let description = schema.schema_data.description.clone();
        let default = schema.schema_data.default.clone();
        let enum_values = oa_enum_values(&schema.schema_kind);
        let format = oa_format(&schema.schema_kind);
        let variants = self.variants(&schema.schema_kind, depth);
        let field_type = self.field_type(&schema.schema_kind, depth);

        FieldSchema {
            name: name.to_string(),
            description,
            field_type,
            required: false,
            default,
            enum_values,
            variants,
            format,
        }
    }

    fn field_type(&self, kind: &'a oa::SchemaKind, depth: usize) -> FieldType {
        if depth > 10 {
            return FieldType::Any;
        }
        match kind {
            oa::SchemaKind::Type(typ) => match typ {
                oa::Type::String(_) => FieldType::String,
                oa::Type::Integer(_) => FieldType::Integer,
                oa::Type::Number(_) => FieldType::Number,
                oa::Type::Boolean(_) => FieldType::Boolean,
                oa::Type::Object(obj) => self.object_type(obj, depth),
                oa::Type::Array(arr) => self.array_type(arr, depth),
            },
            oa::SchemaKind::AllOf { .. } => {
                let (fields, _) = self.collect_object_fields(kind, depth);
                if fields.is_empty() {
                    FieldType::Any
                } else {
                    FieldType::Object(fields)
                }
            }
            oa::SchemaKind::Any(any) => self.any_type(any, depth),
            _ => FieldType::Any,
        }
    }

    fn object_type(&self, obj: &'a oa::ObjectType, depth: usize) -> FieldType {
        if !obj.properties.is_empty() {
            let fields = obj
                .properties
                .iter()
                .filter_map(|(name, prop_ref)| {
                    let schema = self.resolve_boxed(prop_ref)?;
                    let mut field = self.field(name, schema, depth + 1);
                    field.required = obj.required.contains(name);
                    Some(field)
                })
                .collect();
            FieldType::Object(fields)
        } else if let Some(additional) = &obj.additional_properties {
            match additional {
                oa::AdditionalProperties::Schema(s) => {
                    if let Some(schema) = self.resolve_ref_or_schema(s.as_ref()) {
                        FieldType::Map(Some(Box::new(self.field("value", schema, depth + 1))))
                    } else {
                        FieldType::Map(None)
                    }
                }
                oa::AdditionalProperties::Any(_) => FieldType::Map(None),
            }
        } else {
            FieldType::Map(None)
        }
    }

    fn array_type(&self, arr: &'a oa::ArrayType, depth: usize) -> FieldType {
        match &arr.items {
            Some(ref_or) => {
                if let Some(schema) = self.resolve_boxed(ref_or) {
                    FieldType::Array(Box::new(self.field("items", schema, depth + 1)))
                } else {
                    FieldType::Array(Box::new(any_field("items")))
                }
            }
            None => FieldType::Array(Box::new(any_field("items"))),
        }
    }

    fn any_type(&self, any: &'a oa::AnySchema, depth: usize) -> FieldType {
        // AnySchema with allOf — flatten
        if !any.all_of.is_empty() {
            let kind = oa::SchemaKind::AllOf {
                all_of: any.all_of.clone(),
            };
            return self.field_type(&kind, depth);
        }

        if !any.properties.is_empty() {
            let fields = any
                .properties
                .iter()
                .filter_map(|(name, prop_ref)| {
                    let schema = self.resolve_boxed(prop_ref)?;
                    let mut field = self.field(name, schema, depth + 1);
                    field.required = any.required.contains(name);
                    Some(field)
                })
                .collect();
            FieldType::Object(fields)
        } else if let Some(ref typ) = any.typ {
            match typ.as_str() {
                "string" => FieldType::String,
                "integer" => FieldType::Integer,
                "number" => FieldType::Number,
                "boolean" => FieldType::Boolean,
                "array" => match &any.items {
                    Some(ref_or) => {
                        if let Some(schema) = self.resolve_boxed(ref_or) {
                            FieldType::Array(Box::new(self.field("items", schema, depth + 1)))
                        } else {
                            FieldType::Array(Box::new(any_field("items")))
                        }
                    }
                    None => FieldType::Array(Box::new(any_field("items"))),
                },
                _ => FieldType::Any,
            }
        } else {
            FieldType::Any
        }
    }

    fn variants(
        &self,
        kind: &'a oa::SchemaKind,
        depth: usize,
    ) -> Option<Vec<FieldSchema>> {
        if depth > 10 {
            return None;
        }
        let items = match kind {
            oa::SchemaKind::OneOf { one_of } => one_of,
            oa::SchemaKind::AnyOf { any_of } => any_of,
            _ => return None,
        };
        let parsed: Vec<FieldSchema> = items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                let schema = self.resolve_ref_or(item)?;
                Some(self.field(&format!("variant_{}", i), schema, depth + 1))
            })
            .collect();
        if parsed.is_empty() {
            None
        } else {
            Some(parsed)
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn oa_enum_values(kind: &oa::SchemaKind) -> Option<Vec<Value>> {
    match kind {
        oa::SchemaKind::Type(oa::Type::String(s)) if !s.enumeration.is_empty() => {
            let vals: Vec<Value> = s
                .enumeration
                .iter()
                .filter_map(|v| v.as_ref().map(|s| Value::String(s.clone())))
                .collect();
            if vals.is_empty() { None } else { Some(vals) }
        }
        oa::SchemaKind::Any(any) if !any.enumeration.is_empty() => {
            let vals: Vec<Value> = any.enumeration.iter().filter_map(|v| Some(v.clone())).collect();
            if vals.is_empty() { None } else { Some(vals) }
        }
        _ => None,
    }
}

fn oa_format(kind: &oa::SchemaKind) -> Option<String> {
    match kind {
        oa::SchemaKind::Type(typ) => match typ {
            oa::Type::String(s) => variant_or_unknown_to_string(&s.format),
            oa::Type::Integer(i) => variant_or_unknown_to_string(&i.format),
            oa::Type::Number(n) => variant_or_unknown_to_string(&n.format),
            _ => None,
        },
        oa::SchemaKind::Any(any) => any.format.clone(),
        _ => None,
    }
}

fn variant_or_unknown_to_string<T: std::fmt::Debug>(
    v: &oa::VariantOrUnknownOrEmpty<T>,
) -> Option<String> {
    match v {
        oa::VariantOrUnknownOrEmpty::Item(item) => Some(format!("{:?}", item).to_lowercase()),
        oa::VariantOrUnknownOrEmpty::Unknown(s) => Some(s.clone()),
        oa::VariantOrUnknownOrEmpty::Empty => None,
    }
}

fn any_field(name: &str) -> FieldSchema {
    FieldSchema {
        name: name.to_string(),
        description: None,
        field_type: FieldType::Any,
        required: false,
        default: None,
        enum_values: None,
        variants: None,
        format: None,
    }
}

// ---------------------------------------------------------------------------
// V2 fallback: raw JSON with manual $ref resolution
// ---------------------------------------------------------------------------

async fn fetch_schema_v2(
    client: &Client,
    resolved: &ResolvedResource,
) -> Result<ResourceSchema> {
    let openapi: Value = client
        .request(
            http::Request::builder()
                .uri("/openapi/v2")
                .body(Default::default())?,
        )
        .await?;

    let definitions = openapi
        .get("definitions")
        .ok_or_else(|| anyhow::anyhow!("No definitions found in OpenAPI v2 spec"))?;

    let candidates: Vec<String> = definitions
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();

    let key = find_definition_key(&candidates, resolved)?;

    let definition = definitions
        .get(&key)
        .ok_or_else(|| anyhow::anyhow!("Definition not found: {}", key))?;

    let resolver = V2Resolver { root: &openapi };
    v2_parse_resource(definition, &resolver)
}

struct V2Resolver<'a> {
    root: &'a Value,
}

impl<'a> V2Resolver<'a> {
    fn resolve(&self, schema: &'a Value) -> &'a Value {
        if let Some(ref_str) = schema.get("$ref").and_then(|v| v.as_str()) {
            if let Some(path) = ref_str.strip_prefix("#/definitions/") {
                if let Some(resolved) = self.root.get("definitions").and_then(|d| d.get(path)) {
                    return resolved;
                }
            }
        }
        schema
    }
}

fn v2_parse_resource(definition: &Value, resolver: &V2Resolver) -> Result<ResourceSchema> {
    let definition = resolver.resolve(definition);

    let description = definition
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let properties = definition
        .get("properties")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let required_names: Vec<String> = definition
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let mut fields = Vec::new();
    for (name, prop_schema) in &properties {
        let mut field = v2_parse_field(name, prop_schema, resolver, 0);
        field.required = required_names.contains(name);
        fields.push(field);
    }

    Ok(ResourceSchema {
        fields,
        description,
    })
}

fn v2_parse_field(
    name: &str,
    schema: &Value,
    resolver: &V2Resolver,
    depth: usize,
) -> FieldSchema {
    if depth > 10 {
        return any_field(name);
    }
    let schema = resolver.resolve(schema);

    let description = schema
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let default = schema.get("default").cloned();
    let enum_values = schema.get("enum").and_then(|v| v.as_array()).cloned();
    let format = schema
        .get("format")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let field_type = v2_parse_type(schema, resolver, depth);

    FieldSchema {
        name: name.to_string(),
        description,
        field_type,
        required: false,
        default,
        enum_values,
        variants: None,
        format,
    }
}

fn v2_parse_type(schema: &Value, resolver: &V2Resolver, depth: usize) -> FieldType {
    if depth > 10 {
        return FieldType::Any;
    }

    let type_str = schema
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match type_str {
        "string" => FieldType::String,
        "integer" => FieldType::Integer,
        "number" => FieldType::Number,
        "boolean" => FieldType::Boolean,
        "array" => {
            if let Some(items) = schema.get("items") {
                let item = v2_parse_field("items", items, resolver, depth + 1);
                FieldType::Array(Box::new(item))
            } else {
                FieldType::Array(Box::new(any_field("items")))
            }
        }
        "object" | "" => {
            if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
                let required_names: Vec<String> = schema
                    .get("required")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                let fields = props
                    .iter()
                    .map(|(name, prop_schema)| {
                        let mut f = v2_parse_field(name, prop_schema, resolver, depth + 1);
                        f.required = required_names.contains(name);
                        f
                    })
                    .collect();
                FieldType::Object(fields)
            } else if let Some(additional) = schema.get("additionalProperties") {
                let value_schema = v2_parse_field("value", additional, resolver, depth + 1);
                FieldType::Map(Some(Box::new(value_schema)))
            } else {
                FieldType::Map(None)
            }
        }
        _ => FieldType::Any,
    }
}

// ---------------------------------------------------------------------------
// Shared: definition key lookup
// ---------------------------------------------------------------------------

fn find_definition_key(candidates: &[String], resolved: &ResolvedResource) -> Result<String> {
    let kind = &resolved.api_resource.kind;
    let version = &resolved.version;
    let group = &resolved.group;

    // Strategy 1: Match by kind + version + group suffix
    for key in candidates {
        let lower = key.to_lowercase();
        if lower.ends_with(&format!(
            ".{}.{}",
            version.to_lowercase(),
            kind.to_lowercase()
        )) {
            return Ok(key.clone());
        }
    }

    // Strategy 2: ending with version.Kind (case-sensitive)
    for key in candidates {
        if key.ends_with(&format!("{}.{}", version, kind)) {
            return Ok(key.clone());
        }
    }

    // Strategy 3: ends with Kind and contains group
    for key in candidates {
        if key.ends_with(kind)
            && (group.is_empty() || key.to_lowercase().contains(&group.to_lowercase()))
        {
            return Ok(key.clone());
        }
    }

    // Strategy 4: just ends with Kind
    for key in candidates {
        if key.ends_with(kind) {
            return Ok(key.clone());
        }
    }

    bail!(
        "Could not find OpenAPI definition for {}/{} kind={}",
        group,
        version,
        kind
    );
}
