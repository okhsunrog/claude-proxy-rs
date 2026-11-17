//! Persistent installation identity and deterministic conversation fallback.
use rand::RngExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, io, path::Path};
use uuid::Uuid;

pub fn load_device_id(path: &Path) -> io::Result<String> {
    fn read(path: &Path) -> io::Result<String> {
        let id = fs::read_to_string(path)?.trim().to_owned();
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid persisted device ID",
            ));
        }
        Ok(id)
    }
    match read(path) {
        Ok(id) => return Ok(id),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let bytes: [u8; 32] = rand::rng().random();
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let staging = path.with_extension(format!("{}.pending", Uuid::new_v4()));
    let result = (|| {
        use io::Write;
        let mut file = options.open(&staging)?;
        file.write_all(id.as_bytes())?;
        file.sync_all()?;
        drop(file);
        // Publish a complete file without replacing another process's identity.
        match fs::hard_link(&staging, path) {
            Ok(()) => Ok(id),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => read(path),
            Err(e) => Err(e),
        }
    })();
    match fs::remove_file(&staging) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    result
}

/// Explicit session IDs are preferred. Without one, the first user turn is a
/// fallback only: two conversations with the same opening need distinct headers.
pub fn session_id(
    body: &Value,
    headers: &axum::http::HeaderMap,
    key: &str,
    device: &str,
) -> String {
    let metadata = body
        .pointer("/metadata/user_id")
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    if let Some(id) = metadata
        .as_ref()
        .and_then(|v| v.get("session_id"))
        .and_then(Value::as_str)
        .filter(|s| Uuid::parse_str(s).is_ok())
    {
        return id.to_owned();
    }
    for name in ["x-claude-code-session-id", "x-session-id"] {
        if let Some(id) = headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .filter(|s| Uuid::parse_str(s).is_ok())
        {
            return id.to_owned();
        }
    }
    let first = body
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        })
        .and_then(|m| m.get("content"));
    let mut hash = Sha256::new();
    for part in [
        device,
        key,
        &first.map(Value::to_string).unwrap_or_default(),
    ] {
        hash.update(part.len().to_le_bytes());
        hash.update(part.as_bytes());
    }
    let digest = hash.finalize();
    let mut bytes = [0u8; 16];
    for (slot, byte) in bytes.iter_mut().zip(digest.iter()) {
        *slot = *byte;
    }
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    Uuid::from_bytes(bytes).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn identity_survives_reload_and_rejects_corruption() {
        let root = std::env::temp_dir().join(format!("proxy-identity-{}", Uuid::new_v4()));
        let path = root.join("device");
        let first = load_device_id(&path).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(first, load_device_id(&path).unwrap());
        fs::write(&path, "broken").unwrap();
        assert!(load_device_id(&path).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn sessions_follow_conversations_and_are_key_scoped() {
        let body = json!({"messages":[{"role":"user","content":"hello"}]});
        let headers = axum::http::HeaderMap::new();
        let first = session_id(&body, &headers, "key-a", "device");
        let mut continued = body.clone();
        continued["messages"].as_array_mut().unwrap().extend([
            json!({"role":"assistant","content":"hi"}),
            json!({"role":"user","content":"next"}),
        ]);
        assert_eq!(first, session_id(&continued, &headers, "key-a", "device"));
        assert_ne!(first, session_id(&body, &headers, "key-b", "device"));
        let mut headers = headers;
        let explicit = Uuid::new_v4().to_string();
        headers.insert("x-session-id", explicit.parse().unwrap());
        assert_eq!(session_id(&body, &headers, "key-a", "device"), explicit);
    }
}
