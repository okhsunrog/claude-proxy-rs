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
    let mut translated = translate_request(source, Protocol::Responses, body)?;
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
