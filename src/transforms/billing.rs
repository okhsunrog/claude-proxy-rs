//! Final-body CCH checksum, verified against the macOS 2.1.326 capture.
//! Algorithm reference: CLIProxyAPI 7df56074, claude_signing.go (MIT notice in
//! licenses/CLIProxyAPI.txt). This checksum is not an authentication credential.
use anyhow::{Context, Result};
use serde_json::Value;

const SEED: u64 = 0x4D659218E32A3268;

/// Recompute after account identity, prompts, tools and caching have been changed.
/// The input comes from our JSON serializer, so normalizing a Value preserves the
/// ordering and escaping used in the final outgoing JSON.
pub fn finalize(body: &[u8]) -> Result<Vec<u8>> {
    let mut value: Value = serde_json::from_slice(body)?;
    let Some(text) = value.pointer("/system/0/text").and_then(Value::as_str) else {
        return Ok(body.to_vec());
    };
    if !text.starts_with("x-anthropic-billing-header: ") {
        return Ok(body.to_vec());
    }
    let Some(entrypoint) = text.find("cc_entrypoint=") else {
        return Ok(body.to_vec());
    };
    let Some(end) = text.get(entrypoint..).and_then(|s| s.find(';')) else {
        return Ok(body.to_vec());
    };
    let mut billing = text.to_owned();
    let offset = if let Some(start) = billing.find("; cch=") {
        let digits = start + "; cch=".len();
        let Some(end) = billing.get(digits..).and_then(|s| s.find(';')) else {
            return Ok(body.to_vec());
        };
        billing.replace_range(digits..digits + end, "00000");
        digits
    } else {
        let insert = entrypoint + end + 1;
        billing.insert_str(insert, " cch=00000;");
        insert + " cch=".len()
    };
    *value
        .pointer_mut("/system/0/text")
        .context("Missing billing block")? = Value::String(billing.clone());
    let mut normalized = value.clone();
    normalize(&mut normalized);
    let bytes = serde_json::to_vec(&normalized)?;
    let checksum = format!("{:05x}", xxhash_rust::xxh64::xxh64(&bytes, SEED) & 0xfffff);
    billing.replace_range(offset..offset + 5, &checksum);
    *value
        .pointer_mut("/system/0/text")
        .context("Missing billing block")? = Value::String(billing);
    Ok(serde_json::to_vec(&value)?)
}

fn normalize(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|key, _| {
                !matches!(
                    key.as_str(),
                    "max_tokens" | "fallbacks" | "fallback_credit_token"
                )
            });
            for (key, value) in object {
                if key == "model" && value.is_string() {
                    *value = Value::String(String::new());
                } else {
                    normalize(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checksum(body: &Value) -> String {
        let out: Value =
            serde_json::from_slice(&finalize(&serde_json::to_vec(body).unwrap()).unwrap()).unwrap();
        out["system"][0]["text"]
            .as_str()
            .unwrap()
            .split("cch=")
            .nth(1)
            .unwrap()[..5]
            .to_owned()
    }

    #[test]
    fn matches_native_macos_capture() {
        let body: Value = serde_json::from_str(include_str!(
            "../fixtures/claude-2.1.326-macos-x64-cch.json"
        ))
        .unwrap();
        let actual: Value =
            serde_json::from_slice(&finalize(&serde_json::to_vec(&body).unwrap()).unwrap())
                .unwrap();
        assert_eq!(actual, body);
    }

    #[test]
    fn hashes_identity_and_content_but_excludes_dispatch_fields() {
        let mut body = serde_json::json!({
            "model": "claude-sonnet-4-6", "max_tokens": 100,
            "system": [{"type": "text", "text": "x-anthropic-billing-header: cc_version=2.1.326.dd3; cc_entrypoint=sdk-cli;"}],
            "metadata": {"user_id": "original"},
            "messages": [{"role": "user", "content": "hello 世界 🦀"}]
        });
        let initial = checksum(&body);
        body["model"] = "other-model".into();
        body["max_tokens"] = 200.into();
        body["fallbacks"] = serde_json::json!(["other-model"]);
        body["fallback_credit_token"] = "dispatch-only".into();
        assert_eq!(checksum(&body), initial);
        body["metadata"]["user_id"] = "proxy-owner".into();
        assert_ne!(checksum(&body), initial);
        let identity = checksum(&body);
        body["messages"][0]["content"] = "changed".into();
        assert_ne!(checksum(&body), identity);
        let once = finalize(&serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(finalize(&once).unwrap(), once);
    }

    #[test]
    fn leaves_unsigned_payloads_and_lookalike_user_text_alone() {
        let body = br#"{ "messages": [{"role":"user","content":"cch=12345;"}] }"#;
        assert_eq!(finalize(body).unwrap(), body);
    }
}
