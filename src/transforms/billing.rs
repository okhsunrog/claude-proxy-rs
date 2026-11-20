//! Generated billing text and final-body CCH, checked against macOS 2.1.326.
//! Despite its name, x-anthropic-billing-header is system[0].text, not an HTTP
//! header. The SHA-256 prompt suffix and xxHash64 body checksum are independent.
//! Algorithm reference: CLIProxyAPI 7df56074, claude_signing.go (MIT notice in
//! licenses/CLIProxyAPI.txt). This checksum is not an authentication credential.
use anyhow::{Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::constants::CAPTURED_CLAUDE_CODE_VERSION;

// Constants recovered from the client implementation and checked against the
// 2.1.326 macOS capture. This public salt is compatibility data, not a secret.
// These are zero-based UTF-16 offsets; absent positions contribute ASCII '0'.
const PROMPT_SALT: &str = "59cf53e54c78";
const PROMPT_SAMPLE_INDICES: [usize; 3] = [4, 7, 20];
const PROMPT_SUFFIX_HEX_LEN: usize = 3;

/// Build the first system text block for a generated SDK-style request.
/// Native blocks are preserved by preparation; this only synthesizes missing
/// identity. `dispatch` is false for count_tokens, which omits prompt counters.
/// The UUID is also read by routes::auth to populate the prompt-ID header.
pub fn prefix(body: &Value, dispatch: bool) -> String {
    let content = body
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        })
        .and_then(|m| m.get("content"));
    let text = content
        .and_then(|v| {
            v.as_str().or_else(|| {
                v.as_array()
                    .and_then(|a| a.iter().find_map(|b| b.get("text").and_then(Value::as_str)))
            })
        })
        .unwrap_or("");
    let suffix = prompt_suffix(text);
    let mut prefix = format!(
        "x-anthropic-billing-header: cc_version={CAPTURED_CLAUDE_CODE_VERSION}.{suffix}; cc_entrypoint=sdk-cli;"
    );
    if !dispatch {
        return prefix;
    }
    let prompt_id = Uuid::new_v4();
    prefix.push_str(&format!(
        " cc_prompt_id={prompt_id}; cc_turn_origin=sdk; cc_prompt_index=0; cc_turn_index=1;"
    ));
    prefix
}

/// A short prompt/version marker, separate from the final-body CCH below.
/// Three hex digits cannot authenticate a binary or prove server acceptance.
fn prompt_suffix(text: &str) -> String {
    // The installed JS binary indexes UTF-16 code units, not Unicode scalars.
    let units: Vec<u16> = text.encode_utf16().collect();
    let sampled: Vec<u16> = PROMPT_SAMPLE_INDICES
        .iter()
        .map(|&i| units.get(i).copied().unwrap_or(u16::from(b'0')))
        .collect();
    let hash = Sha256::digest(
        format!(
            "{PROMPT_SALT}{}{CAPTURED_CLAUDE_CODE_VERSION}",
            String::from_utf16_lossy(&sampled)
        )
        .as_bytes(),
    );
    let suffix: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    suffix.chars().take(PROMPT_SUFFIX_HEX_LEN).collect()
}

// Captured CCH uses seeded xxHash64 truncated to 20 bits (five hex digits).
const CCH_SEED: u64 = 0x4D659218E32A3268;
const CCH_MASK: u64 = 0xfffff;

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
    let checksum = format!(
        "{:05x}",
        xxhash_rust::xxh64::xxh64(&bytes, CCH_SEED) & CCH_MASK
    );
    billing.replace_range(offset..offset + 5, &checksum);
    *value
        .pointer_mut("/system/0/text")
        .context("Missing billing block")? = Value::String(billing);
    Ok(serde_json::to_vec(&value)?)
}

// Mirror the captured client's recursive normalization. Dispatch model/token
// fields are excluded, while identity, prompts and tools remain checksummed.
// Do not sort object keys: serde_json/preserve_order retains serialization order.
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
    use serde_json::json;

    #[test]
    fn prompt_marker_preserves_utf16_sampling_and_counting_envelope() {
        // Fixed vectors include a surrogate at a sampled position. These were
        // independently calculated using UTF-16LE sampling and SHA-256.
        assert_eq!(prompt_suffix(""), "bc0");
        assert_eq!(prompt_suffix("hi"), "bc0");
        assert_eq!(prompt_suffix("abcd😀efghijklmnopqrstu"), "673");
        let body = json!({"messages":[
            {"role":"assistant","content":"ignored"},
            {"role":"user","content":[{"type":"image"},{"type":"text","text":"Reply exactly OK."}]}
        ]});
        assert_eq!(
            prefix(&body, false),
            "x-anthropic-billing-header: cc_version=2.1.326.dd3; cc_entrypoint=sdk-cli;"
        );
        // Header literals must stay aligned with the version used in the hash.
        assert!(crate::constants::INFERENCE_USER_AGENT.contains(CAPTURED_CLAUDE_CODE_VERSION));
        assert!(crate::constants::USER_AGENT.ends_with(CAPTURED_CLAUDE_CODE_VERSION));
    }

    #[test]
    fn billing_marker_matches_captured_prompt_and_changes_with_prompt() {
        let body = json!({"messages":[{"role":"user","content":"Reply exactly OK."}]});
        let billing = prefix(&body, true);
        assert!(billing.starts_with("x-anthropic-billing-header: cc_version=2.1.326.dd3; cc_entrypoint=sdk-cli; cc_prompt_id="));
        assert!(billing.ends_with("; cc_turn_origin=sdk; cc_prompt_index=0; cc_turn_index=1;"));
        assert_ne!(
            billing.split("; cc_prompt_id=").next(),
            prefix(
                &json!({"messages":[{"role":"user","content":"A different first prompt changes sampled characters."}]}), true
            ).split("; cc_prompt_id=").next()
        );
    }

    fn checksum(body: &Value) -> String {
        let out: Value =
            serde_json::from_slice(&finalize(&serde_json::to_vec(body).unwrap()).unwrap()).unwrap();
        out["system"][0]["text"]
            .as_str()
            .unwrap()
            .split("cch=")
            .nth(1)
            .unwrap()
            .get(..5)
            .unwrap()
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
