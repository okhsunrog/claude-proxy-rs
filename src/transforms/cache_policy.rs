use crate::compatibility::CachePolicy;
use serde_json::Value;

pub fn apply(body: Value, policy: CachePolicy) -> Value {
    // Top-level automatic caching already owns a breakpoint slot and chooses
    // its own moving boundary. Do not add block-level markers behind its back.
    if policy == CachePolicy::Preserve || body.get("cache_control").is_some() {
        return body;
    }
    if policy == CachePolicy::Auto5m {
        return llm_relay::anthropic::cache_control::ensure_cache_control(body);
    }
    let existing: std::collections::HashSet<_> = paths(&body)
        .into_iter()
        .filter(|p| body.pointer(p).is_some())
        .collect();
    let mut body = llm_relay::anthropic::cache_control::ensure_cache_control(body);
    let mut short_seen = false;
    for path in paths(&body) {
        let Some(marker) = body.pointer_mut(&path) else {
            continue;
        };
        if existing.contains(&path) {
            if marker.get("ttl").and_then(Value::as_str) != Some("1h") {
                short_seen = true;
            }
        } else if !short_seen && let Some(marker) = marker.as_object_mut() {
            marker.insert("ttl".into(), Value::String("1h".into()));
        }
    }
    body
}

fn paths(body: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    for section in ["tools", "system"] {
        if let Some(items) = body.get(section).and_then(Value::as_array) {
            paths.extend((0..items.len()).map(|i| format!("/{section}/{i}/cache_control")));
        }
    }
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for (i, message) in messages.iter().enumerate() {
            if let Some(content) = message.get("content").and_then(Value::as_array) {
                paths.extend(
                    (0..content.len()).map(|j| format!("/messages/{i}/content/{j}/cache_control")),
                );
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body() -> Value {
        json!({"tools":[{"name":"tool","input_schema":{"type":"object"}}],"system":[{"type":"text","text":"system"}],"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}]})
    }

    #[test]
    fn policies_preserve_explicit_markers_and_automatic_cache() {
        let original = body();
        assert_eq!(apply(original.clone(), CachePolicy::Preserve), original);
        let short = apply(original.clone(), CachePolicy::Auto5m);
        let long = apply(original, CachePolicy::Auto1h);
        let markers: Vec<_> = paths(&long)
            .into_iter()
            .filter_map(|p| long.pointer(&p))
            .collect();
        assert!(!markers.is_empty());
        assert!(markers.len() <= 4);
        assert!(markers.iter().all(|m| m["ttl"] == "1h"));
        assert!(
            paths(&short)
                .iter()
                .filter_map(|p| short.pointer(p))
                .all(|m| m.get("ttl").is_none())
        );
        let mut explicit = body();
        explicit["tools"][0]["cache_control"] = json!({"type":"ephemeral"});
        let result = apply(explicit, CachePolicy::Auto1h);
        let markers: Vec<_> = paths(&result)
            .into_iter()
            .filter_map(|p| result.pointer(&p))
            .collect();
        assert!(markers.iter().all(|m| m.get("ttl").is_none()));
        let mut automatic = body();
        automatic["cache_control"] = json!({"type":"ephemeral","ttl":"1h"});
        assert_eq!(apply(automatic.clone(), CachePolicy::Auto5m), automatic);
    }
}
