use std::env;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::time::{SystemTime, UNIX_EPOCH};

use async_stream::stream;
use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::header::HeaderMap as ReqwestHeaderMap;
use serde_json::to_string_pretty;
use serde_json::{Value, json};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tracing::warn;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct CaptureConfig {
    dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Capture {
    dir: PathBuf,
}

impl CaptureConfig {
    pub fn from_env() -> Self {
        let dir = env::var("CLAUDE_PROXY_CAPTURE_DIR")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        Self { dir }
    }

    pub fn is_enabled(&self) -> bool {
        self.dir.is_some()
    }
}

impl Capture {
    pub async fn begin(
        config: &CaptureConfig,
        protocol: &str,
        endpoint: &str,
        model: &str,
        stream: bool,
        client_headers: &HeaderMap,
        inbound_body: &Value,
    ) -> Option<Self> {
        let base_dir = config.dir.as_ref()?;
        let id = format!(
            "{}-{}-{}",
            now_millis(),
            sanitize_filename(protocol),
            Uuid::new_v4()
        );
        let dir = base_dir.join(id);
        if let Err(e) = fs::create_dir_all(&dir).await {
            warn!("Failed to create capture directory {}: {e}", dir.display());
            return None;
        }

        let capture = Self { dir };
        capture
            .write_json(
                "meta.json",
                &json!({
                    "captured_at_ms": now_millis(),
                    "protocol": protocol,
                    "endpoint": endpoint,
                    "model": model,
                    "stream": stream,
                    "client_headers": headers_to_json(client_headers),
                }),
            )
            .await;
        capture.write_json("inbound.json", inbound_body).await;
        Some(capture)
    }

    pub async fn write_prepared(&self, body: &Value, betas: &[String], cloak: bool) {
        self.write_json(
            "prepared.json",
            &json!({
                "cloak": cloak,
                "betas": betas,
                "body": body,
            }),
        )
        .await;
    }

    pub async fn write_upstream_response(
        &self,
        status: reqwest::StatusCode,
        headers: &ReqwestHeaderMap,
    ) {
        self.write_json(
            "upstream_response.json",
            &json!({
                "status": status.as_u16(),
                "headers": reqwest_headers_to_json(headers),
            }),
        )
        .await;
    }

    pub async fn write_upstream_body(&self, body: &str) {
        self.write_text("upstream_body.txt", body).await;
    }

    pub fn upstream_stream_path(&self) -> PathBuf {
        self.dir.join("upstream_stream.sse")
    }

    async fn write_json(&self, name: &str, value: &Value) {
        match to_string_pretty(value) {
            Ok(text) => self.write_text(name, &format!("{text}\n")).await,
            Err(e) => warn!("Failed to serialize capture file {name}: {e}"),
        }
    }

    async fn write_text(&self, name: &str, text: &str) {
        let path = self.dir.join(name);
        if let Err(e) = fs::write(&path, text).await {
            warn!("Failed to write capture file {}: {e}", path.display());
        }
    }
}

pub fn capture_byte_stream<S, E>(
    body: S,
    path: Option<PathBuf>,
) -> impl Stream<Item = Result<Bytes, E>> + Send
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    stream! {
        let mut body = pin!(body);
        let mut file = match path {
            Some(path) => open_append(&path).await,
            None => None,
        };

        while let Some(item) = body.next().await {
            if let (Some(writer), Ok(bytes)) = (file.as_mut(), &item)
                && let Err(e) = writer.write_all(bytes).await
            {
                warn!("Failed to append upstream stream capture: {e}");
                file = None;
            }
            yield item;
        }
    }
}

async fn open_append(path: &Path) -> Option<File> {
    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    {
        Ok(file) => Some(file),
        Err(e) => {
            warn!("Failed to open stream capture {}: {e}", path.display());
            None
        }
    }
}

fn headers_to_json(headers: &HeaderMap) -> Value {
    Value::Object(
        headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    Value::String(sanitize_header(
                        name.as_str(),
                        value.to_str().unwrap_or("<binary>"),
                    )),
                )
            })
            .collect(),
    )
}

fn reqwest_headers_to_json(headers: &ReqwestHeaderMap) -> Value {
    Value::Object(
        headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    Value::String(sanitize_header(
                        name.as_str(),
                        value.to_str().unwrap_or("<binary>"),
                    )),
                )
            })
            .collect(),
    )
}

fn sanitize_header(name: &str, value: &str) -> String {
    let name = name.to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "authorization" | "x-api-key" | "cookie" | "set-cookie" | "proxy-authorization"
    ) {
        "<redacted>".to_string()
    } else {
        value.to_string()
    }
}

fn sanitize_filename(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
