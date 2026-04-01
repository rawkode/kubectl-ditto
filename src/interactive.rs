use std::collections::HashMap;

use anyhow::Result;
use console::Style;
use dialoguer::{Confirm, Input, Select};
use serde_json::Value;

use crate::schema::{FieldSchema, FieldType};

/// Prompt the user interactively for values of the given fields.
/// Returns a map of field_name -> user-provided value.
pub fn prompt_for_fields(fields: &[&FieldSchema]) -> Result<HashMap<String, Value>> {
    let mut values = HashMap::new();
    let header = Style::new().bold().cyan();
    let dim = Style::new().dim();

    eprintln!(
        "{}",
        header.apply_to("Interactive mode — fill in resource fields (Enter to skip optional)")
    );
    eprintln!();

    for field in fields {
        if let Some(val) = prompt_field(field, &header, &dim, 0)? {
            values.insert(field.name.clone(), val);
        }
    }

    eprintln!();
    eprintln!("{}", header.apply_to("✓ Done! Generating YAML..."));
    eprintln!();

    Ok(values)
}

fn prompt_field(
    field: &FieldSchema,
    header: &Style,
    dim: &Style,
    depth: usize,
) -> Result<Option<Value>> {
    // Don't go too deep interactively
    if depth > 3 {
        return Ok(None);
    }

    let req_marker = if field.required { " (required)" } else { "" };

    // Show description if available
    if let Some(ref desc) = field.description {
        let short = desc.lines().next().unwrap_or(desc);
        if short.len() <= 120 {
            eprintln!("  {}", dim.apply_to(short));
        } else {
            eprintln!("  {}", dim.apply_to(&short[..120]));
        }
    }

    match &field.field_type {
        FieldType::String => {
            // If there are enum values, show a select menu
            if let Some(ref enum_vals) = field.enum_values {
                let options: Vec<String> = enum_vals
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();

                if options.is_empty() {
                    return prompt_string_field(field, req_marker);
                }

                let selection = Select::new()
                    .with_prompt(format!("{}{}", field.name, req_marker))
                    .items(&options)
                    .default(0)
                    .interact_opt()?;

                match selection {
                    Some(idx) => Ok(Some(Value::String(options[idx].clone()))),
                    None if field.required => {
                        eprintln!("  This field is required!");
                        prompt_field(field, header, dim, depth)
                    }
                    None => Ok(None),
                }
            } else {
                prompt_string_field(field, req_marker)
            }
        }
        FieldType::Integer => {
            let input: String = Input::new()
                .with_prompt(format!("{}{} (integer)", field.name, req_marker))
                .default(
                    field
                        .default
                        .as_ref()
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0)
                        .to_string(),
                )
                .allow_empty(!field.required)
                .interact_text()?;

            if input.is_empty() {
                return Ok(None);
            }

            match input.parse::<i64>() {
                Ok(n) => Ok(Some(Value::Number(n.into()))),
                Err(_) => {
                    eprintln!("  Invalid integer, using 0");
                    Ok(Some(Value::Number(0.into())))
                }
            }
        }
        FieldType::Number => {
            let input: String = Input::new()
                .with_prompt(format!("{}{} (number)", field.name, req_marker))
                .default("0".to_string())
                .allow_empty(!field.required)
                .interact_text()?;

            if input.is_empty() {
                return Ok(None);
            }

            match input.parse::<f64>() {
                Ok(n) => Ok(Some(
                    serde_json::Number::from_f64(n)
                        .map(Value::Number)
                        .unwrap_or(Value::Number(0.into())),
                )),
                Err(_) => {
                    eprintln!("  Invalid number, using 0");
                    Ok(Some(Value::Number(0.into())))
                }
            }
        }
        FieldType::Boolean => {
            let default_val = field
                .default
                .as_ref()
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let confirmed = Confirm::new()
                .with_prompt(format!("{}{}", field.name, req_marker))
                .default(default_val)
                .interact_opt()?;

            match confirmed {
                Some(b) => Ok(Some(Value::Bool(b))),
                None => Ok(None),
            }
        }
        FieldType::Object(sub_fields) if !sub_fields.is_empty() && depth < 2 => {
            eprintln!("  {}", header.apply_to(format!("── {} ──", field.name)));

            let mut map = serde_json::Map::new();
            for sub in sub_fields {
                if let Some(val) = prompt_field(sub, header, dim, depth + 1)? {
                    map.insert(sub.name.clone(), val);
                }
            }

            if map.is_empty() {
                Ok(None)
            } else {
                Ok(Some(Value::Object(map)))
            }
        }
        FieldType::Array(item_schema) if depth < 2 => {
            eprintln!(
                "  {}",
                header.apply_to(format!("── {} (array, Enter empty to stop) ──", field.name))
            );

            let mut items = Vec::new();
            loop {
                let item = prompt_field(item_schema, header, dim, depth + 1)?;
                match item {
                    Some(v) if v != Value::Null => items.push(v),
                    _ => break,
                }
            }

            if items.is_empty() {
                Ok(None)
            } else {
                Ok(Some(Value::Array(items)))
            }
        }
        FieldType::Map(_) => {
            // For maps, prompt for key=value pairs
            eprintln!(
                "  {}",
                header.apply_to(format!(
                    "── {} (key=value pairs, Enter empty to stop) ──",
                    field.name
                ))
            );

            let mut map = serde_json::Map::new();
            loop {
                let input: String = Input::new()
                    .with_prompt("key=value")
                    .allow_empty(true)
                    .interact_text()?;

                if input.is_empty() {
                    break;
                }

                if let Some((k, v)) = input.split_once('=') {
                    map.insert(k.trim().to_string(), Value::String(v.trim().to_string()));
                } else {
                    eprintln!("  Format: key=value");
                }
            }

            if map.is_empty() {
                Ok(None)
            } else {
                Ok(Some(Value::Object(map)))
            }
        }
        _ => {
            // Fallback: prompt as string
            prompt_string_field(field, req_marker)
        }
    }
}

fn prompt_string_field(field: &FieldSchema, req_marker: &str) -> Result<Option<Value>> {
    let default = field
        .default
        .as_ref()
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let input: String = Input::new()
        .with_prompt(format!("{}{}", field.name, req_marker))
        .default(default)
        .allow_empty(!field.required)
        .interact_text()?;

    if input.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Value::String(input)))
    }
}
