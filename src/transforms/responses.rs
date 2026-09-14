//! Subscription-specific restrictions; protocol codecs live in llm-relay.
use serde_json::{Value, json};
type Validation<T> = Result<T, String>;
pub fn prepare(mut body: Value) -> Validation<Value> {
    let obj = body.as_object_mut().ok_or("Expected a JSON object")?;
    if obj.get("store").and_then(Value::as_bool) == Some(true) {
        return Err("Subscription requests require store=false".into());
    }
    for field in [
        "previous_response_id",
        "conversation",
        "background",
        "max_output_tokens",
        "temperature",
        "top_p",
        "truncation",
    ] {
        if obj
            .get(field)
            .is_some_and(|v| !v.is_null() && v != &Value::Bool(false))
        {
            return Err(format!(
                "{field} is not supported by this subscription endpoint; send full conversation history"
            ));
        }
        obj.remove(field);
    }
    match obj.get_mut("input") {
        Some(input) if input.is_string() => {
            *input = json!([{"role":"user","content":input.as_str()}])
        }
        Some(input) if input.is_array() => {}
        _ => return Err("input must be a string or array".into()),
    }
    if obj.get("instructions").is_none_or(Value::is_null) {
        obj.insert("instructions".into(), json!(""));
    }
    if !obj.get("instructions").is_some_and(Value::is_string) {
        return Err("instructions must be a string".into());
    }
    if let Some(v) = obj.get("stream")
        && !v.is_boolean()
    {
        return Err("stream must be a boolean".into());
    }
    obj.insert("store".into(), json!(false));
    obj.insert("stream".into(), json!(true));
    Ok(body)
}

/// The subscription backend supports fewer controls than the public wire protocol.
/// The caller explicitly chooses whether documented lossy controls may be ignored.
pub fn prepare_request(
    source: llm_relay::protocol::Protocol,
    body: &Value,
    policy: llm_relay::protocol::Policy,
) -> Result<llm_relay::protocol::Translation, String> {
    use llm_relay::protocol::{Diagnostic, Protocol, translate_request};
    // Messages context editing is a server-side provider capability. The
    // subscription backend cannot perform it; retain the full supplied history.
    let mut body = body.clone();
    let context_management = if source == Protocol::Messages {
        body.as_object_mut()
            .and_then(|object| object.remove("context_management"))
    } else {
        None
    };
    let inline_system = if source == Protocol::Messages {
        normalize_inline_system(&mut body)?
    } else {
        false
    };
    let mut translated = translate_request(source, Protocol::Responses, &body)?;
    if inline_system {
        translated.diagnostics.push(Diagnostic {
            field: "messages.system".into(),
            reason: "Inline system messages moved to the system instruction field".into(),
        });
    }
    if context_management.is_some_and(|value| !value.is_null()) {
        translated.diagnostics.push(Diagnostic {
            field: "context_management".into(),
            reason: "Server-side context editing unavailable; full history retained".into(),
        });
    }
    let object = translated.body.as_object_mut().ok_or("Expected object")?;
    for field in ["max_output_tokens", "temperature", "top_p", "top_k", "stop"] {
        if object.get(field).is_some_and(|v| !v.is_null()) {
            translated.diagnostics.push(Diagnostic {
                field: field.into(),
                reason: "Not supported by subscription backend; ignored in compatible mode".into(),
            });
        }
        object.remove(field);
    }
    translated = translated.enforce(policy)?;
    translated.body = prepare(translated.body)?;
    Ok(translated)
}

// Recent clients send environment instructions as inline system messages,
// although the Messages wire format places system instructions at the top level.
fn normalize_inline_system(body: &mut Value) -> Result<bool, String> {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return Ok(false);
    };
    let mut instructions = Vec::new();
    for message in messages
        .iter()
        .filter(|message| message["role"] == "system")
    {
        match &message["content"] {
            Value::String(text) => instructions.push(json!({"type":"text", "text":text})),
            Value::Array(parts)
                if parts
                    .iter()
                    .all(|part| part["type"] == "text" && part["text"].is_string()) =>
            {
                instructions.extend(parts.iter().cloned())
            }
            _ => return Err("Inline system messages must contain text".into()),
        }
    }
    let changed = messages.iter().any(|message| message["role"] == "system");
    if !changed {
        return Ok(false);
    }
    messages.retain(|message| message["role"] != "system");
    let mut system = match body.get("system") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(text)) => vec![json!({"type":"text", "text":text})],
        Some(Value::Array(parts)) => parts.clone(),
        _ => return Err("system must be text or an array".into()),
    };
    system.extend(instructions);
    body["system"] = json!(system);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_relay::protocol::{Policy, Protocol};

    #[test]
    fn required_messages_limit_is_reported_or_rejected() {
        let body = json!({"model":"gpt-test","max_tokens":128,"messages":[{"role":"user","content":"Hi"}]});
        let result = prepare_request(Protocol::Messages, &body, Policy::Compatible).unwrap();
        assert_eq!(result.diagnostics[0].field, "max_output_tokens");
        assert!(result.body.get("max_output_tokens").is_none());
        assert_eq!(result.body["stream"], true);
        assert_eq!(result.body["store"], false);
        prepare_request(Protocol::Messages, &body, Policy::Strict).unwrap_err();
    }

    #[test]
    fn messages_context_management_preserves_history_and_reports_loss() {
        let body = json!({
            "model": "gpt-test",
            "messages": [{"role":"user","content":"Keep this conversation"}],
            "thinking": {"type":"adaptive", "display":"omitted"},
            "output_config": {"effort":"high"},
            "context_management": {"edits":[{"type":"clear_thinking_20251015","keep":"all"}]}
        });
        let converted = prepare_request(Protocol::Messages, &body, Policy::Compatible).unwrap();
        assert!(converted.body.get("context_management").is_none());
        assert_eq!(
            converted.body["input"][0]["content"][0]["text"],
            "Keep this conversation"
        );
        assert_eq!(converted.body["reasoning"]["effort"], "high");
        assert!(
            converted
                .diagnostics
                .iter()
                .any(|d| d.field == "context_management")
        );
        let error = prepare_request(Protocol::Messages, &body, Policy::Strict).unwrap_err();
        assert!(error.contains("context_management"));
        assert!(body.get("context_management").is_some());
    }

    #[test]
    fn inline_system_instructions_are_retained_and_reported() {
        let body = json!({"model":"gpt-test", "system":"Original instruction", "messages":[
            {"role":"user", "content":"Hi"},
            {"role":"system", "content":[{"type":"text", "text":"Environment", "cache_control":{"type":"ephemeral"}}]},
            {"role":"system", "content":"Additional instruction"}
        ]});
        let converted = prepare_request(Protocol::Messages, &body, Policy::Compatible).unwrap();
        assert_eq!(converted.body["input"][0]["role"], "developer");
        assert_eq!(
            converted.body["input"][0]["content"][0]["text"],
            "Original instruction"
        );
        assert_eq!(
            converted.body["input"][0]["content"][1]["text"],
            "Environment"
        );
        assert_eq!(
            converted.body["input"][0]["content"][2]["text"],
            "Additional instruction"
        );
        assert_eq!(converted.body["input"][1]["role"], "user");
        assert!(
            converted
                .diagnostics
                .iter()
                .any(|d| d.field == "messages.system")
        );
        prepare_request(Protocol::Messages, &body, Policy::Strict).unwrap_err();
        let bad = json!({"model":"gpt-test","messages":[{"role":"system","content":[{"type":"tool_use"}]}]});
        prepare_request(Protocol::Messages, &bad, Policy::Compatible).unwrap_err();
    }

    #[test]
    fn native_stateful_requests_are_never_silently_downgraded() {
        for field in [
            "previous_response_id",
            "conversation",
            "background",
            "store",
        ] {
            let mut body = json!({"model":"gpt-test","input":"Hi"});
            body[field] = if ["background", "store"].contains(&field) {
                json!(true)
            } else {
                json!("id")
            };
            for policy in [Policy::Strict, Policy::Compatible] {
                assert!(
                    prepare_request(Protocol::Responses, &body, policy).is_err(),
                    "{field}"
                );
            }
        }
    }
}
