//! Conservative, reversible input-field adapters. Results remain opaque client
//! content; tool_result IDs and is_error are preserved throughout the pipeline.
use crate::compatibility::ClientProfile;
use serde_json::{Map, Value};

#[derive(Clone, Debug, Default)]
pub struct ArgumentAdapter {
    /// Client property name -> upstream property name.
    fields: Vec<(String, String)>,
}

impl ArgumentAdapter {
    pub fn estimated_bytes(&self) -> usize {
        self.fields
            .iter()
            .map(|(a, b)| a.len() + b.len() + std::mem::size_of::<(String, String)>())
            .sum()
    }
    /// Recognized names alone are insufficient: reject unsupported properties,
    /// ambiguous mappings and complex schemas rather than silently discard them.
    /// None means the caller must retain the tool as a separate MCP contract.
    pub fn select(
        name: &str,
        tool: &Value,
        profile: ClientProfile,
    ) -> Option<(&'static str, Self)> {
        let base = name.strip_prefix("mcp_").unwrap_or(name);
        let (native, required, allowed): (&str, &[&str], &[(&str, &str)]) =
            match base.to_ascii_lowercase().as_str() {
                "read" | "read_file" => (
                    "Read",
                    &["file_path"],
                    &[
                        ("file_path", "string"),
                        ("offset", "number"),
                        ("limit", "number"),
                        ("pages", "string"),
                    ],
                ),
                "write" | "write_file" | "write_to_file" => (
                    "Write",
                    &["file_path", "content"],
                    &[("file_path", "string"), ("content", "string")],
                ),
                "edit" | "edit_file" | "replace_in_file" | "search_replace" => (
                    "Edit",
                    &["file_path", "old_string", "new_string"],
                    &[
                        ("file_path", "string"),
                        ("old_string", "string"),
                        ("new_string", "string"),
                        ("replace_all", "boolean"),
                    ],
                ),
                "bash" | "shell" | "execute_command" => (
                    "Bash",
                    &["command"],
                    &[
                        ("command", "string"),
                        ("description", "string"),
                        ("timeout", "number"),
                        ("run_in_background", "boolean"),
                    ],
                ),
                _ => return None,
            };
        if !profile.permits_alias(base, native) {
            return None;
        }
        let schema = tool.get("input_schema")?;
        if schema.get("type")?.as_str()? != "object"
            || schema.get("additionalProperties") != Some(&Value::Bool(false))
        {
            return None;
        }
        if [
            "$ref",
            "allOf",
            "anyOf",
            "oneOf",
            "patternProperties",
            "if",
            "then",
            "else",
            "dependentSchemas",
            "dependencies",
            "propertyNames",
            "$defs",
            "definitions",
            "dependentRequired",
            "enum",
            "const",
            "default",
            "examples",
        ]
        .iter()
        .any(|key| schema.get(key).is_some())
        {
            return None;
        }
        let properties = schema.get("properties")?.as_object()?;
        // OpenCode 2 read accepts directories too; its path schema is not Read.
        if profile == ClientProfile::OpenCode && native == "Read" && properties.contains_key("path")
        {
            return None;
        }
        let mut fields = Vec::new();
        let mut destinations = std::collections::HashSet::new();
        for (key, property) in properties {
            if ["$ref", "allOf", "anyOf", "oneOf", "if", "then", "else"]
                .iter()
                .any(|key| property.get(key).is_some())
            {
                return None;
            }
            let target = match key.as_str() {
                "path" | "filePath" => "file_path",
                "oldString" => "old_string",
                "newString" => "new_string",
                "replaceAll" => "replace_all",
                _ => key,
            };
            let expected = allowed.iter().find(|(k, _)| *k == target)?.1;
            let actual = property.get("type")?.as_str()?;
            if actual != expected && !(expected == "number" && actual == "integer") {
                return None;
            }
            if !destinations.insert(target.to_owned()) {
                return None;
            }
            if key != target {
                fields.push((key.clone(), target.to_owned()));
            }
            if key == "offset"
                && property
                    .get("minimum")
                    .and_then(Value::as_f64)
                    .is_some_and(|n| n < 1.0)
            {
                return None;
            }
        }
        let required_client = schema.get("required")?.as_array()?;
        if required_client
            .iter()
            .any(|k| !k.as_str().is_some_and(|k| properties.contains_key(k)))
        {
            return None;
        }
        for key in required {
            if !required_client.iter().filter_map(Value::as_str).any(|k| {
                fields
                    .iter()
                    .find(|(old, _)| old == k)
                    .map(|(_, new)| new.as_str())
                    .unwrap_or(k)
                    == *key
            }) {
                return None;
            }
        }
        let canonical = match native {
            "Read" => "mcp_Read",
            "Write" => "mcp_Write",
            "Edit" => "mcp_Edit",
            "Bash" => "mcp_Bash",
            _ => return None,
        };
        Some((canonical, Self { fields }))
    }

    pub fn changes_arguments(&self) -> bool {
        !self.fields.is_empty()
    }

    pub fn schema(&self, schema: &mut Value) {
        if let Some(properties) = schema.get_mut("properties") {
            self.rename(properties, false);
        }
        if let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) {
            for key in required {
                if let Some(new) = key.as_str().and_then(|k| {
                    self.fields
                        .iter()
                        .find(|(old, _)| old == k)
                        .map(|(_, new)| new.clone())
                }) {
                    *key = Value::String(new);
                }
            }
        }
    }

    pub fn forward(&self, value: &mut Value) {
        self.rename(value, false);
    }
    pub fn reverse(&self, value: &mut Value) {
        self.rename(value, true);
    }

    fn rename(&self, value: &mut Value, reverse: bool) {
        let Some(object) = value.as_object_mut() else {
            return;
        };
        if self.fields.iter().any(|(client, upstream)| {
            let (from, to) = if reverse {
                (upstream, client)
            } else {
                (client, upstream)
            };
            object.contains_key(from) && object.contains_key(to)
        }) {
            return;
        }
        let old = std::mem::take(object);
        *object = old
            .into_iter()
            .map(|(key, value)| {
                let name = self
                    .fields
                    .iter()
                    .find_map(|(client, upstream)| {
                        let (from, to) = if reverse {
                            (upstream, client)
                        } else {
                            (client, upstream)
                        };
                        (from == &key).then(|| to.clone())
                    })
                    .unwrap_or(key);
                (name, value)
            })
            .collect::<Map<_, _>>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ambiguous_or_incomplete_schemas_are_not_adapted() {
        let schema = json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false});
        for (key, value) in [
            ("additionalProperties", json!(true)),
            ("required", json!([])),
            ("allOf", json!([])),
            ("dependentRequired", json!({"path":["other"]})),
            ("properties", json!({"path":{"type":"string","oneOf":[]}})),
            (
                "properties",
                json!({"path":{"type":"string"},"file_path":{"type":"string"}}),
            ),
            (
                "properties",
                json!({"path":{"type":"string"},"offset":{"type":"integer","minimum":0}}),
            ),
        ] {
            let mut candidate = schema.clone();
            candidate[key] = value;
            assert!(
                ArgumentAdapter::select(
                    "read_file",
                    &json!({"input_schema":candidate}),
                    ClientProfile::OpenAi
                )
                .is_none(),
                "{key}"
            );
        }
    }
}
