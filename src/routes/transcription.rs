//! File transcription is independently limited; upstream returns no token/cost usage.
use crate::{AppState, error::ProxyError};
use axum::{
    Json,
    extract::{Multipart, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};

pub const UPLOAD_LIMIT: usize = 25 * 1024 * 1024;
pub const TRANSCRIPTION_MODEL: &str = "chatgpt-transcribe";
const URL: &str = "https://chatgpt.com/backend-api/transcribe";

pub struct TranscriptionLimits {
    concurrency: Semaphore,
    requests: Mutex<HashMap<String, (Instant, u32)>>,
}
impl TranscriptionLimits {
    pub fn new() -> Self {
        Self {
            concurrency: Semaphore::new(2),
            requests: Mutex::new(HashMap::new()),
        }
    }
    async fn admit(&self, key: &str) -> bool {
        let mut requests = self.requests.lock().await;
        requests.retain(|_, (start, _)| start.elapsed() < Duration::from_secs(60));
        let (_, count) = requests.entry(key.into()).or_insert((Instant::now(), 0));
        if *count >= 10 {
            return false;
        }
        *count += 1;
        true
    }
}
#[derive(Deserialize, Serialize)]
struct Transcript {
    text: String,
}
fn error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({"error": {"message": message, "type": "transcription_error"}})),
    )
        .into_response()
}

pub async fn transcribe(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Response {
    let Some(key) = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    else {
        return ProxyError::InvalidApiKey.to_openai_response();
    };
    let key = match state.client_keys.validate(key).await {
        Ok(Some(key)) => key,
        Ok(None) => return ProxyError::InvalidApiKey.to_openai_response(),
        Err(e) => return e.to_openai_response(),
    };
    match state
        .client_keys
        .is_model_allowed(&key.id, TRANSCRIPTION_MODEL)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return ProxyError::ModelNotAllowed(TRANSCRIPTION_MODEL.into()).to_openai_response();
        }
        Err(e) => return e.to_openai_response(),
    }
    if !state.transcription_limits.admit(&key.id).await {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "Transcription limit: 10 requests per minute per key",
        );
    }
    let Ok(_permit) = state.transcription_limits.concurrency.try_acquire() else {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "Transcription is busy; retry shortly",
        );
    };
    let upload = match tokio::time::timeout(Duration::from_secs(60), parse_upload(multipart)).await
    {
        Ok(Ok(upload)) => upload,
        Ok(Err((status, message))) => return error(status, message),
        Err(_) => return error(StatusCode::REQUEST_TIMEOUT, "Audio upload timed out"),
    };
    let AudioUpload {
        bytes,
        filename,
        mime,
        language,
    } = upload;
    let (mut token, mut account) = match state.chatgpt.credentials(None).await {
        Ok(credentials) => credentials,
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, &e),
    };
    for attempt in 0..2 {
        let part = match reqwest::multipart::Part::bytes(bytes.to_vec())
            .file_name(filename.clone())
            .mime_str(mime)
        {
            Ok(part) => part,
            Err(_) => return error(StatusCode::BAD_REQUEST, "Invalid audio content type"),
        };
        let mut form = reqwest::multipart::Form::new().part("file", part);
        if let Some(language) = &language {
            form = form.text("language", language.clone());
        }
        let mut request = state
            .http_client
            .post(URL)
            .bearer_auth(&token)
            .header("originator", "Codex Desktop")
            .header("User-Agent", "Codex Desktop/26.901.41600 (X11; Linux; x64)")
            .timeout(Duration::from_secs(60))
            .multipart(form);
        if let Some(account) = &account {
            request = request.header("ChatGPT-Account-Id", account);
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(_) => {
                return error(
                    StatusCode::BAD_GATEWAY,
                    "Cannot reach transcription service",
                );
            }
        };
        if response.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
            (token, account) = match state.chatgpt.credentials(Some(&token)).await {
                Ok(credentials) => credentials,
                Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, &e),
            };
            continue;
        }
        if !response.status().is_success() {
            // Never forward arbitrary upstream bodies (which can contain sensitive diagnostics).
            let status = if response.status() == StatusCode::TOO_MANY_REQUESTS {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::BAD_GATEWAY
            };
            return error(status, "Transcription service rejected the recording");
        }
        return match response.json::<Transcript>().await {
            Ok(transcript) => {
                if let Err(e) = state.client_keys.update_last_used(&key.id).await {
                    tracing::warn!("Cannot update transcription key activity: {e}");
                }
                Json(transcript).into_response()
            }
            Err(_) => error(StatusCode::BAD_GATEWAY, "Invalid transcription response"),
        };
    }
    error(
        StatusCode::BAD_GATEWAY,
        "Transcription authentication failed",
    )
}
struct AudioUpload {
    bytes: bytes::Bytes,
    filename: String,
    mime: &'static str,
    language: Option<String>,
}
fn upload_error(status: StatusCode, message: &'static str) -> (StatusCode, &'static str) {
    (status, message)
}
async fn parse_upload(mut multipart: Multipart) -> Result<AudioUpload, (StatusCode, &'static str)> {
    let mut file = None;
    let mut language = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(e) => {
                return Err(upload_error(
                    e.status(),
                    "Invalid multipart upload or upload too large",
                ));
            }
        };
        match field.name().unwrap_or("") {
            "file" => {
                if file.is_some() {
                    return Err(upload_error(
                        StatusCode::BAD_REQUEST,
                        "Only one file is accepted",
                    ));
                }
                let Some((filename, mime)) = audio_type(field.file_name().unwrap_or("")) else {
                    return Err(upload_error(
                        StatusCode::BAD_REQUEST,
                        "Use an ogg, webm, wav, mp3, m4a, mp4 or flac recording",
                    ));
                };
                let bytes = match field.bytes().await {
                    Ok(bytes) => bytes,
                    Err(e) => return Err(upload_error(e.status(), "Cannot read audio upload")),
                };
                if bytes.is_empty() || bytes.len() > UPLOAD_LIMIT {
                    return Err(upload_error(
                        StatusCode::BAD_REQUEST,
                        "Audio must be non-empty and at most 25 MiB",
                    ));
                }
                file = Some((bytes, filename, mime));
            }
            "language" => {
                let value = match field.text().await {
                    Ok(v) => v,
                    Err(_) => {
                        return Err(upload_error(StatusCode::BAD_REQUEST, "Invalid language"));
                    }
                };
                if !valid_language(&value) || language.is_some() {
                    return Err(upload_error(
                        StatusCode::BAD_REQUEST,
                        "Use one language code, such as en or ru",
                    ));
                }
                language = Some(value);
            }
            "model" => {
                if field.text().await.ok().as_deref() != Some(TRANSCRIPTION_MODEL) {
                    return Err(upload_error(
                        StatusCode::BAD_REQUEST,
                        "The supported model is chatgpt-transcribe",
                    ));
                }
            }
            "response_format" => {
                if field.text().await.ok().as_deref() != Some("json") {
                    return Err(upload_error(
                        StatusCode::BAD_REQUEST,
                        "Only response_format=json is supported",
                    ));
                }
            }
            _ => {
                return Err(upload_error(
                    StatusCode::BAD_REQUEST,
                    "Unsupported transcription field",
                ));
            }
        }
    }
    let Some((bytes, filename, mime)) = file else {
        return Err(upload_error(StatusCode::BAD_REQUEST, "Missing audio file"));
    };
    Ok(AudioUpload {
        bytes,
        filename,
        mime,
        language,
    })
}
fn audio_type(name: &str) -> Option<(String, &'static str)> {
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "ogg" => "audio/ogg",
        "webm" => "audio/webm",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" => "audio/mp4",
        "flac" => "audio/flac",
        _ => return None,
    };
    Some((format!("recording.{ext}"), mime))
}
fn valid_language(value: &str) -> bool {
    (2..=12).contains(&value.len()) && value.bytes().all(|c| c.is_ascii_alphabetic() || c == b'-')
}
#[cfg(test)]
mod tests {
    use super::*;
    async fn upload(
        parts: &[(&str, Option<&str>, &[u8])],
    ) -> Result<AudioUpload, (StatusCode, &'static str)> {
        use axum::{body::Body, extract::FromRequest, http::Request};
        let mut body = Vec::new();
        for (name, filename, data) in parts {
            body.extend_from_slice(
                format!("--boundary\r\nContent-Disposition: form-data; name=\"{name}\"").as_bytes(),
            );
            if let Some(filename) = filename {
                body.extend_from_slice(format!("; filename=\"{filename}\"").as_bytes());
            }
            body.extend_from_slice(b"\r\n\r\n");
            body.extend_from_slice(data);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(b"--boundary--\r\n");
        let request = Request::builder()
            .header("content-type", "multipart/form-data; boundary=boundary")
            .body(Body::from(body))
            .unwrap();
        parse_upload(Multipart::from_request(request, &()).await.unwrap()).await
    }
    #[tokio::test]
    async fn multipart_preserves_audio_and_language() {
        let result = upload(&[
            ("file", Some("voice.ogg"), b"OggS\0\xff\x80"),
            ("language", None, b"ru"),
        ])
        .await
        .unwrap();
        assert_eq!(result.bytes.as_ref(), b"OggS\0\xff\x80");
        assert_eq!(result.language.as_deref(), Some("ru"));
        assert_eq!(result.mime, "audio/ogg");
    }
    #[tokio::test]
    async fn multipart_rejects_missing_duplicate_and_unsupported_fields() {
        let file = ("file", Some("voice.ogg"), b"audio".as_slice());
        for parts in [
            vec![("language", None, b"ru".as_slice())],
            vec![file, file],
            vec![file, ("prompt", None, b"instructions".as_slice())],
        ] {
            assert_eq!(
                upload(&parts).await.err().unwrap().0,
                StatusCode::BAD_REQUEST
            );
        }
    }
    #[tokio::test]
    async fn request_limits_are_per_key() {
        let limits = TranscriptionLimits::new();
        for _ in 0..10 {
            assert!(limits.admit("a").await);
        }
        assert!(!limits.admit("a").await);
        assert!(limits.admit("b").await);
    }
    #[test]
    fn sanitizes_filename_and_rejects_invalid_formats() {
        assert_eq!(
            audio_type("../../voice.OGG"),
            Some(("recording.ogg".into(), "audio/ogg"))
        );
        assert!(audio_type("file.txt").is_none());
        assert!(valid_language("ru"));
        assert!(!valid_language("ru\r\n"));
    }
}
