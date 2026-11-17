//! Opt-in full HTTP/DB audit using real SDK clients and a local upstream.
//! No production database, provider credentials or model calls are used.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
use std::{convert::Infallible, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::header,
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, process::Command, sync::Mutex};

use crate::{
    AppState,
    auth::{AuthStore, ClientKeysStore, ModelsStore, OAuthManager, storage::Auth},
    compatibility::Compatibility,
};

async fn upstream(
    State(captures): State<Arc<Mutex<Vec<Value>>>>,
    Json(body): Json<Value>,
) -> Response {
    captures.lock().await.push(body.clone());
    let cancel = body["messages"][0]["content"]
        .to_string()
        .contains("audit cancel");
    let usage = json!({"input_tokens":11,"output_tokens":7,"cache_read_input_tokens":3,"cache_creation_input_tokens":4});
    let turn = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .filter(|b| b["type"] == "tool_result")
        .count();
    if turn > 0 {
        let results: Vec<_> = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["content"].as_array())
            .flatten()
            .filter(|b| b["type"] == "tool_result")
            .collect();
        let expected = ["before", "file missing", "edited"];
        for (i, result) in results.iter().enumerate() {
            assert!(
                result["content"].to_string().contains(expected[i]),
                "{result}"
            );
        }
    }
    let content = match turn {
        0 => {
            json!({"type":"tool_use","id":"call0","name":"mcp_Read","input":{"file_path":"probe.txt"}})
        }
        1 => {
            json!({"type":"tool_use","id":"call1","name":"mcp_Read","input":{"file_path":"missing.txt"}})
        }
        2 => {
            json!({"type":"tool_use","id":"call2","name":"mcp_Edit","input":{"file_path":"probe.txt","old_string":"before","new_string":"after"}})
        }
        _ => json!({"type":"text","text":"done"}),
    };
    let stop = if turn < 3 { "tool_use" } else { "end_turn" };
    let message = json!({"id":"msg_audit","type":"message","role":"assistant","model":"claude-sonnet-4-6","content":[content.clone()],"stop_reason":stop,"stop_sequence":null,"usage":usage});
    if body["stream"] != true {
        return Json(message).into_response();
    }
    let mut events = vec![
        json!({"type":"message_start","message":{"id":"msg_audit","type":"message","role":"assistant","model":"claude-sonnet-4-6","content":[],"usage":{"input_tokens":11,"output_tokens":0,"cache_read_input_tokens":3,"cache_creation_input_tokens":4},"stop_reason":null,"stop_sequence":null}}),
    ];
    if cancel {
        events.push(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}));
        events.push(json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":2}}));
        events.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}));
    } else {
        let mut start = content.clone();
        if content["type"] == "tool_use" {
            start["input"] = json!({});
        } else {
            start["text"] = json!("");
        }
        events.push(json!({"type":"content_block_start","index":0,"content_block":start}));
        if content["type"] == "tool_use" {
            let arguments = content["input"].to_string();
            for fragment in arguments.as_bytes().chunks(3) {
                events.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":std::str::from_utf8(fragment).unwrap()}}));
            }
        } else {
            events.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"done"}}));
        }
        events.push(json!({"type":"content_block_stop","index":0}));
        events.push(json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},"usage":{"output_tokens":7}}));
        events.push(json!({"type":"message_stop"}));
    }
    let stream = async_stream::stream! {
        for event in events {
            yield Ok::<_,Infallible>(format!("event: {}\ndata: {event}\n\n",event["type"].as_str().unwrap()));
        }
        if cancel {std::future::pending::<()>().await;}
    };
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        Body::from_stream(stream),
    )
        .into_response()
}

#[tokio::test]
#[ignore = "Requires isolated loopback PostgreSQL via PROXY_AUDIT_DATABASE_URL, uv and PROXY_AUDIT_ROOT"]
async fn real_clients_through_http_and_postgres() {
    let url = std::env::var("PROXY_AUDIT_DATABASE_URL").expect("isolated audit database required");
    let parsed = url::Url::parse(&url).unwrap();
    assert!(matches!(parsed.host_str(), Some("127.0.0.1" | "localhost")));
    assert_eq!(parsed.path(), "/proxy_compat");
    let root = PathBuf::from(
        std::env::var("PROXY_AUDIT_ROOT").expect("audit artifact directory required"),
    );
    std::fs::create_dir_all(&root).unwrap();
    crate::db::init_db(&url).await.unwrap();
    let captures = Arc::new(Mutex::new(Vec::new()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let mock = Router::new()
        .route("/v1/messages", post(upstream))
        .with_state(captures.clone());
    let mock_task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let auth_store = Arc::new(AuthStore::new());
    auth_store
        .set(
            "anthropic",
            Auth::OAuth {
                access: "audit-not-a-real-token".into(),
                refresh: "unused".into(),
                expires: u64::MAX / 2,
                account_id: Some("00000000-0000-4000-8000-000000000001".into()),
                enterprise_url: None,
            },
        )
        .await
        .unwrap();
    let state = Arc::new(AppState {
        chatgpt: crate::auth::chatgpt::ChatGptAuth::new(client.clone(), auth_store.clone()),
        oauth: OAuthManager::new(client.clone(), auth_store.clone()),
        auth_store,
        client_keys: Arc::new(ClientKeysStore::new()),
        models: Arc::new(ModelsStore::new()),
        http_client: client.clone(),
        anthropic_transport: crate::anthropic_transport::AnthropicTransport::new().unwrap(),
        inference_url: upstream_url,
        count_tokens_url: "http://127.0.0.1:1/unused".into(),
        transcription_limits: crate::routes::transcription::TranscriptionLimits::new(120, 4),
        admin_credentials: crate::admin_session::AdminCredentials {
            username: "audit".into(),
            password: "unused".into(),
        },
        secure_cookies: false,
        disable_auth: false,
        cloak_mode: crate::config::CloakMode::Always,
        compatibility: Compatibility::default(),
        usage_cache: crate::usage::UsageCache::new(),
        device_id: "audit-device".into(),
        capture: crate::capture::CaptureConfig::from_env(),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/v1/messages", post(crate::routes::anthropic::messages))
        .route(
            "/v1/chat/completions",
            post(crate::routes::openai::chat_completions),
        )
        .with_state(state.clone());
    let proxy_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for mode in ["native", "native-stream", "openai", "openai-stream"] {
        let key = state
            .client_keys
            .create(format!("audit-{mode}"))
            .await
            .unwrap();
        let output = tokio::time::timeout(
            Duration::from_secs(90),
            Command::new("uv")
                .args([
                    "run",
                    "--project",
                    "tests/clients",
                    "python",
                    "tests/clients/audit.py",
                ])
                .env("PROXY_AUDIT_URL", &proxy_url)
                .env("PROXY_AUDIT_KEY", &key.key)
                .env("PROXY_AUDIT_MODE", mode)
                .env("PROXY_AUDIT_WORKSPACE", root.join(mode))
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        println!("{}", String::from_utf8_lossy(&output.stdout));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let entries = state.client_keys.get_model_usage(&key.id).await.unwrap();
                if let Some(entry) = entries.first()
                    && entry.total.input == 55
                {
                    assert_eq!(entry.total.output, 30);
                    assert_eq!(entry.total.cache_read, 15);
                    assert_eq!(entry.total.cache_write, 20);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("usage must reach DB once for every completion/cancellation");
        let response = client
            .post(format!("{proxy_url}/v1/messages"))
            .header("x-api-key", &key.key)
            .header("x-proxy-client-profile", "typo")
            .json(&json!({"model":"claude-sonnet-4-6","messages":[]}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
    }
    let bodies = captures.lock().await;
    assert_eq!(bodies.len(), 20);
    std::fs::write(
        root.join("upstream.json"),
        serde_json::to_vec_pretty(&*bodies).unwrap(),
    )
    .unwrap();
    for body in bodies.iter().filter(|b| b.get("tools").is_some()) {
        assert_eq!(body["tools"][0]["name"], "mcp_Read");
        assert!(
            body["tools"][0]["input_schema"]["properties"]
                .get("file_path")
                .is_some()
        );
        assert_eq!(body["tools"][1]["name"], "mcp_Edit");
        assert!(
            body["system"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["cache_control"]["ttl"] == "1h")
        );
    }
    proxy_task.abort();
    mock_task.abort();
}
