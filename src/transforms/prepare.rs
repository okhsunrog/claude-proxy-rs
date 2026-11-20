//! Prepare requests for the Anthropic API.
//!
//! Routes apply OAuth account/session identity before entering this pipeline.
//! After extracting body betas, recognized native envelopes bypass prompt,
//! tool and cache rewrites. Other clients use compatibility policy to prepare
//! their prompts, tools and cache markers. Continuations reuse the saved tool
//! mapping instead of inventing names without the original definitions.
//!
//! This prepares JSON, not final wire bytes: the inference transport computes
//! CCH after serialization and selects the ordered HTTP/TLS profile.

use rand::RngExt;
use serde_json::{Value, json};
use uuid::Uuid;

use llm_relay::anthropic::tool_names::transform_request_tool_names;

use super::tool_aliases::{ToolNameMap, normalize_tools};
use crate::compatibility::{ClientProfile, Compatibility};
use crate::constants::SYSTEM_PREFIX;

/// Result of preparing a request for Anthropic API.
pub struct PreparedRequest {
    /// The transformed request body
    pub body: Value,
    /// Betas extracted from the body (to be added to headers)
    pub betas: Vec<String>,
    /// Reverse mapping for both native and OpenAI response paths.
    pub tool_name_map: ToolNameMap,
}

/// Test convenience wrapper using the default compatibility policy.
#[cfg(test)]
pub fn prepare_anthropic_request(body: Value, cloak: bool) -> PreparedRequest {
    prepare_with_policy(body, cloak, Compatibility::default())
}

/// Prepare a complete request; native envelopes bypass compatibility rewrites.
/// `cloak` enables generated SDK identity and schema-aware tool adapters. The
/// other non-native path keeps the legacy tool prefix and system sanitization.
pub fn prepare_with_policy(body: Value, cloak: bool, policy: Compatibility) -> PreparedRequest {
    let (betas, body) = extract_betas(body);
    // Native envelopes already carry the client's exact prompts, tool names,
    // cache policy and context edits. Rewriting them changes the wire contract.
    if is_native_envelope(&body) || policy.profile == ClientProfile::Native {
        return PreparedRequest {
            body,
            betas,
            tool_name_map: ToolNameMap::default(),
        };
    }

    let body = disable_thinking_if_forced(body);
    let body = if cloak {
        inject_fake_user_id(body)
    } else {
        body
    };
    let mut body = body;
    if !cloak {
        transform_request_tool_names(&mut body);
    }
    let body = if cloak {
        inject_system_message(body, true)
    } else {
        sanitize_system_only(body)
    };
    let body = super::cache_policy::apply(body, policy.cache);

    let mut body = body;
    let tool_name_map = if cloak {
        normalize_tools(&mut body, policy.profile)
    } else {
        ToolNameMap::default()
    };
    PreparedRequest {
        body,
        betas,
        tool_name_map,
    }
}

/// Continue with the original aliases and thinking policy. The upstream thread
/// already owns system/tools/cache state; do not inject a second envelope.
pub fn prepare_continuation(
    body: Value,
    mut map: ToolNameMap,
    transform_thinking: bool,
) -> PreparedRequest {
    let (betas, body) = extract_betas(body);
    let mut body = if transform_thinking {
        disable_thinking_if_forced(body)
    } else {
        body
    };
    map.forward_request(&mut body);
    PreparedRequest {
        body,
        betas,
        tool_name_map: map,
    }
}

/// Recognize the current native envelope without trusting arbitrary metadata alone.
pub(crate) fn is_native_envelope(body: &Value) -> bool {
    body.get("system")
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks
                .first()
                .and_then(|b| b.get("text"))
                .and_then(Value::as_str)
                .is_some_and(|s| s.starts_with("x-anthropic-billing-header: "))
        })
        && body
            .pointer("/metadata/user_id")
            .and_then(Value::as_str)
            .is_some_and(is_valid_user_id)
}

/// Prepare token counting with the same prompt/tool/cache policy as inference,
/// but without generated prompt dispatch fields. Native profiles bypass these
/// rewrites. This helper does not inject metadata or alter thinking settings.
pub fn prepare_count_with_policy(
    body: Value,
    cloak: bool,
    policy: Compatibility,
) -> PreparedRequest {
    let (betas, body) = extract_betas(body);
    if policy.profile == ClientProfile::Native {
        return PreparedRequest {
            body,
            betas,
            tool_name_map: ToolNameMap::default(),
        };
    }
    let body = if cloak {
        inject_system_message(body, false)
    } else {
        sanitize_system_only(body)
    };
    let body = super::cache_policy::apply(body, policy.cache);

    let mut body = body;
    if cloak {
        normalize_tools(&mut body, policy.profile);
    }

    PreparedRequest {
        body,
        betas,
        tool_name_map: ToolNameMap::default(),
    }
}

/// Extract betas array from request body and remove it.
fn extract_betas(mut body: Value) -> (Vec<String>, Value) {
    let betas = match body.get("betas") {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() {
                vec![]
            } else {
                vec![s.to_string()]
            }
        }
        _ => vec![],
    };

    if let Some(obj) = body.as_object_mut() {
        obj.remove("betas");
    }

    (betas, body)
}

/// Disable thinking if tool_choice forces tool use.
///
/// Anthropic API does not allow thinking when tool_choice.type is "any" or "tool".
/// See: https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking#important-considerations
fn disable_thinking_if_forced(mut body: Value) -> Value {
    let tool_choice_type = body
        .get("tool_choice")
        .and_then(|tc| tc.get("type"))
        .and_then(|t| t.as_str());

    // "auto" is allowed with thinking, but "any" or "tool" (specific tool) are not
    if matches!(tool_choice_type, Some("any") | Some("tool"))
        && let Some(obj) = body.as_object_mut()
    {
        obj.remove("thinking");
    }

    body
}

/// Supply fallback JSON identity only if routes have not supplied a valid ID.
/// Production routes normally use apply_identity with persisted device/account
/// state first; this fallback must not replace that stable identity.
fn inject_fake_user_id(mut body: Value) -> Value {
    let needs_injection = match body.get("metadata") {
        None => true,
        Some(metadata) => match metadata.get("user_id") {
            None => true,
            Some(Value::String(id)) => id.is_empty() || !is_valid_user_id(id),
            _ => true,
        },
    };

    if needs_injection {
        let fake_id = generate_fake_user_id();
        if let Some(obj) = body.as_object_mut() {
            if let Some(Value::Object(metadata)) = obj.get_mut("metadata") {
                metadata.insert("user_id".to_string(), Value::String(fake_id));
            } else {
                obj.insert("metadata".to_string(), json!({"user_id": fake_id}));
            }
        }
    }

    body
}

/// Bind account/session to the selected OAuth owner and conversation. Preserve
/// a valid native device ID and extra fields such as parent_session_id; use the
/// persisted proxy installation ID only when a device ID is absent or malformed.
pub fn apply_identity(body: &mut Value, device: &str, account: Option<&str>, session: &str) {
    let mut id = body
        .pointer("/metadata/user_id")
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    if id
        .get("device_id")
        .and_then(Value::as_str)
        .is_none_or(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        id.insert("device_id".into(), json!(device));
    }
    id.insert("account_uuid".into(), json!(account.unwrap_or("")));
    id.insert("session_id".into(), json!(session));
    if let Some(object) = body.as_object_mut() {
        let mut metadata = object
            .get("metadata")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        metadata.insert("user_id".into(), json!(Value::Object(id).to_string()));
        object.insert("metadata".into(), Value::Object(metadata));
    }
}

/// Inject system message prefix into the request body (Claude Code identity).
///
/// Cache markers are handled separately by cache_policy::apply. Billing prefix
/// generation belongs to billing; the final CCH is deferred to the transport.
fn inject_system_message(mut body: Value, dispatch: bool) -> Value {
    let billing = super::billing::prefix(&body, dispatch);
    let obj = match body.as_object_mut() {
        Some(o) => o,
        None => return body,
    };

    let prefix = json!({
        "type": "text",
        "text": SYSTEM_PREFIX
    });

    let new_system = match obj.get("system").cloned() {
        None => json!([{"type": "text", "text": billing}, prefix]),
        Some(Value::String(s)) => {
            json!([
                {"type": "text", "text": billing},
                prefix,
                {"type": "text", "text": s}
            ])
        }
        Some(Value::Array(arr)) => {
            let mut new_arr = vec![json!({"type": "text", "text": billing}), prefix];
            new_arr.extend(arr);
            Value::Array(new_arr)
        }
        Some(other) => {
            json!([prefix, other])
        }
    };

    obj.insert("system".to_string(), sanitize_system(new_system));
    body
}

/// Sanitize system prompt without injecting the Claude Code prefix.
/// Retain the legacy OpenCode compatibility rewrite for non-native requests.
fn sanitize_system_only(mut body: Value) -> Value {
    let obj = match body.as_object_mut() {
        Some(o) => o,
        None => return body,
    };

    if let Some(system) = obj.get("system").cloned() {
        obj.insert("system".to_string(), sanitize_system(system));
    }

    body
}

/// Generate a fake user ID in Claude Code format.
/// JSON string containing device_id, account_uuid and session_id.
fn generate_fake_user_id() -> String {
    let mut rng = rand::rng();
    let hex_bytes: [u8; 32] = rng.random();
    let hex_part: String = hex_bytes.iter().map(|b| format!("{:02x}", b)).collect();
    let uuid_part = Uuid::new_v4().to_string();
    json!({"device_id": hex_part, "account_uuid": "", "session_id": uuid_part}).to_string()
}

/// Check if a user ID matches Claude Code format.
/// Accept current JSON metadata and legacy IDs from existing clients.
fn is_valid_user_id(user_id: &str) -> bool {
    if let Ok(value) = serde_json::from_str::<Value>(user_id) {
        return value
            .get("device_id")
            .and_then(Value::as_str)
            .is_some_and(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
            && value.get("account_uuid").is_some_and(Value::is_string)
            && value
                .get("session_id")
                .and_then(Value::as_str)
                .is_some_and(|s| Uuid::parse_str(s).is_ok());
    }
    let Some((user_part, uuid_part)) = user_id.split_once("_account__session_") else {
        return false;
    };

    // Check hex part
    let hex_part = user_part.strip_prefix("user_");
    let valid_hex =
        hex_part.is_some_and(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()));

    // Check UUID part (basic validation)
    let valid_uuid = uuid_part.len() == 36 && uuid_part.matches('-').count() == 4;

    valid_hex && valid_uuid
}

/// Sanitize system prompt text blocks.
/// The Anthropic OAuth backend blocks requests containing "OpenCode" in system prompts.
fn sanitize_system(mut system: Value) -> Value {
    if let Value::Array(arr) = &mut system {
        for item in arr.iter_mut() {
            if item.get("type").and_then(|t| t.as_str()) == Some("text")
                && let Some(text) = item
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
            {
                let sanitized = text
                    .replace("OpenCode", "Claude Code")
                    .replace("opencode", "Claude")
                    .replace("Opencode", "Claude")
                    .replace("OPENCODE", "Claude")
                    // Legacy compatibility wording retained from earlier probes.
                    // This replacement alone does not establish server-side
                    // client classification or subscription billing behavior.
                    .replace(
                        "Here is some useful information about the environment you are running in:",
                        "Working context:",
                    );
                if let Some(obj) = item.as_object_mut() {
                    obj.insert("text".to_string(), Value::String(sanitized));
                }
            }
        }
    }
    system
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_betas() {
        let body = json!({
            "model": "claude-3",
            "betas": ["beta1", "beta2"]
        });
        let (betas, body) = extract_betas(body);
        assert_eq!(betas, vec!["beta1", "beta2"]);
        assert!(body.get("betas").is_none());
    }

    #[test]
    fn test_disable_thinking_when_forced() {
        let body = json!({
            "tool_choice": {"type": "any"},
            "thinking": {"type": "enabled", "budget_tokens": 1000}
        });
        let result = disable_thinking_if_forced(body);
        assert!(result.get("thinking").is_none());
    }

    #[test]
    fn test_thinking_preserved_with_auto() {
        let body = json!({
            "tool_choice": {"type": "auto"},
            "thinking": {"type": "enabled", "budget_tokens": 1000}
        });
        let result = disable_thinking_if_forced(body);
        assert!(result.get("thinking").is_some());
    }

    #[test]
    fn test_inject_fake_user_id() {
        let body = json!({"model": "claude-3"});
        let result = inject_fake_user_id(body);
        let user_id = result["metadata"]["user_id"].as_str().unwrap();
        assert!(is_valid_user_id(user_id));
    }

    #[test]
    fn test_inject_system_message() {
        let body = json!({"model": "claude-3"});
        let result = inject_system_message(body, true);
        let system = result["system"].as_array().unwrap();
        assert!(
            system[0]["text"]
                .as_str()
                .unwrap()
                .starts_with("x-anthropic-billing-header: cc_version=2.1.326.")
        );
        assert_eq!(system[1]["text"], SYSTEM_PREFIX);
    }

    #[test]
    fn test_sanitize_system_replaces_opencode() {
        let body = json!({
            "system": "You are OpenCode, an AI assistant. Use opencode tools."
        });
        let result = inject_system_message(body, true);
        let system = result["system"].as_array().unwrap();
        // Billing and identity precede the client prompt.
        let text = system[2]["text"].as_str().unwrap();
        assert!(!text.contains("OpenCode"));
        assert!(!text.contains("opencode"));
        assert!(text.contains("Claude Code"));
        assert!(text.contains("Claude tools"));
    }

    #[test]
    fn test_sanitize_system_array_format() {
        let body = json!({
            "system": [
                {"type": "text", "text": "You are OpenCode assistant"},
                {"type": "text", "text": "Use opencode for help"}
            ]
        });
        let result = inject_system_message(body, true);
        let system = result["system"].as_array().unwrap();
        // Billing and identity precede the two client blocks.
        assert!(!system[2]["text"].as_str().unwrap().contains("OpenCode"));
        assert!(!system[3]["text"].as_str().unwrap().contains("opencode"));
    }

    #[test]
    fn test_sanitize_system_rewords_env_preamble() {
        let body = json!({
            "system": "Here is some useful information about the environment you are running in:\n<env>x</env>"
        });
        let result = inject_system_message(body, true);
        let joined: String = result["system"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["text"].as_str().unwrap_or(""))
            .collect();
        assert!(!joined.contains("Here is some useful information about the environment"));
        assert!(joined.contains("Working context:"));
    }

    #[test]
    fn test_generate_fake_user_id_format() {
        let id = generate_fake_user_id();
        serde_json::from_str::<Value>(&id).unwrap();
        assert!(is_valid_user_id(&id));
    }

    #[test]
    fn test_is_valid_user_id() {
        let valid = "user_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef_account__session_12345678-1234-1234-1234-123456789012";
        assert!(is_valid_user_id(valid));

        assert!(!is_valid_user_id("invalid"));
        assert!(!is_valid_user_id("user_short_account__session_uuid"));
        assert!(!is_valid_user_id(""));
    }
    #[test]
    fn oauth_identity_replaces_account_but_preserves_native_device_and_extras() {
        let mut body = json!({"metadata":{"user_id":json!({"device_id":"a".repeat(64),"account_uuid":"wrong-owner","session_id":"old","parent_session_id":"parent"}).to_string()}});
        apply_identity(
            &mut body,
            &"b".repeat(64),
            Some("token-owner"),
            "conversation",
        );
        let id: Value =
            serde_json::from_str(body["metadata"]["user_id"].as_str().unwrap()).unwrap();
        assert_eq!(id["account_uuid"], "token-owner");
        assert_eq!(id["device_id"], "a".repeat(64));
        assert_eq!(id["session_id"], "conversation");
        assert_eq!(id["parent_session_id"], "parent");
        let mut empty = json!({"messages":[]});
        apply_identity(&mut empty, &"b".repeat(64), None, "conversation");
        let id: Value =
            serde_json::from_str(empty["metadata"]["user_id"].as_str().unwrap()).unwrap();
        assert_eq!(id["device_id"], "b".repeat(64));
        assert_eq!(id["account_uuid"], "");
    }
}
