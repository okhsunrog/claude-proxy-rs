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
    let threaded = body.get("thread").is_some();
    let retry_probe = body["messages"][0]["content"]
        .to_string()
        .contains("audit retry");
    if retry_probe {
        let marker = body["messages"][0]["content"].to_string();
        let attempt = captures
            .lock()
            .await
            .iter()
            .filter(|b| b["messages"][0]["content"].to_string() == marker)
            .count();
        if attempt == 1 || marker.contains("retry-max") {
            return (axum::http::StatusCode::from_u16(529).unwrap(),[("retry-after-ms","0")],Json(json!({"type":"error","error":{"type":"overloaded_error","message":"retry audit"}}))).into_response();
        }
    }
    if body.pointer("/thread/type").and_then(Value::as_str) == Some("continue") {
        assert!(body.get("system").is_none() && body.get("tools").is_none());
        assert_eq!(body["tool_choice"]["name"], "mcp_Read");
        assert_eq!(
            body["messages"][0]["content"][0]["content"][0]["tool_name"],
            "mcp_Read"
        );
        let captures = captures.lock().await;
        let created = captures
            .iter()
            .rev()
            .find(|b| b.pointer("/thread/type").and_then(Value::as_str) == Some("create"))
            .unwrap();
        let created: Value =
            serde_json::from_str(created["metadata"]["user_id"].as_str().unwrap()).unwrap();
        let continued: Value =
            serde_json::from_str(body["metadata"]["user_id"].as_str().unwrap()).unwrap();
        assert_eq!(created["session_id"], continued["session_id"]);
    }
    if body["messages"][0]["content"]
        .to_string()
        .contains("audit rate-limit")
    {
        return (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            [
                ("retry-after", "3"),
                ("retry-after-ms", "3000"),
                ("x-should-retry", "false"),
                ("request-id", "audit-limit"),
                ("set-cookie", "private-cookie"),
            ],
            Json(
                json!({"type":"error","error":{"type":"rate_limit_error","message":"audit limit"}}),
            ),
        )
            .into_response();
    }
    let opencode = body["messages"][0]["content"]
        .to_string()
        .contains("audit opencode");
    let cancel = body["messages"][0]["content"]
        .to_string()
        .contains("audit cancel");
    let usage = json!({"input_tokens":11,"output_tokens":7,"cache_read_input_tokens":3,"cache_creation_input_tokens":4,"cache_creation":{"ephemeral_5m_input_tokens":2,"ephemeral_1h_input_tokens":2}});
    let mut turn = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .filter(|b| b["type"] == "tool_result")
        .count();
    if retry_probe {
        turn = 3;
    }
    // OpenCode also makes auxiliary title/summary calls without tools.
    if opencode && body.get("tools").is_none() {
        turn = 3;
    }
    if turn > 0 && body.get("tools").is_some() && !threaded {
        let results: Vec<_> = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["content"].as_array())
            .flatten()
            .filter(|b| b["type"] == "tool_result")
            .collect();
        let expected = if opencode {
            ["before", "File not found", "Edited"]
        } else {
            ["before", "file missing", "edited"]
        };
        for (i, result) in results.iter().enumerate() {
            assert!(
                result["content"].to_string().contains(expected[i]),
                "{result}"
            );
            if opencode && i == 1 {
                assert_eq!(result["is_error"], true);
            }
        }
    }
    let mut content = match if threaded { 0 } else { turn } {
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
    if opencode && turn < 3 {
        let needle = if turn < 2 { "read" } else { "edit" };
        let tool = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| {
                t["name"]
                    .as_str()
                    .is_some_and(|n| n.to_ascii_lowercase().ends_with(needle))
            })
            .expect("real client must register file tools");
        content["name"] = tool["name"].clone();
        let properties = &tool["input_schema"]["properties"];
        let path_key = ["path", "filePath", "file_path"]
            .into_iter()
            .find(|k| properties.get(*k).is_some())
            .unwrap();
        let mut args = json!({});
        args[path_key] = json!(if turn == 1 {
            "missing.txt"
        } else {
            "probe.txt"
        });
        if turn == 2 {
            let old = if properties.get("oldString").is_some() {
                "oldString"
            } else {
                "old_string"
            };
            let new = if properties.get("newString").is_some() {
                "newString"
            } else {
                "new_string"
            };
            args[old] = json!("before");
            args[new] = json!("after");
        }
        content["input"] = args;
    }
    let stop = if turn < 3 || threaded {
        "tool_use"
    } else {
        "end_turn"
    };
    let message = json!({"id":"msg_audit","type":"message","role":"assistant","model":"claude-sonnet-4-6","content":[content.clone()],"stop_reason":stop,"stop_sequence":null,"usage":usage});
    if body["stream"] != true {
        return Json(message).into_response();
    }
    let mut start_usage = usage.clone();
    start_usage["output_tokens"] = json!(0);
    let mut events = vec![
        json!({"type":"message_start","message":{"id":"msg_audit","type":"message","role":"assistant","model":"claude-sonnet-4-6","content":[],"usage":start_usage,"stop_reason":null,"stop_sequence":null}}),
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
        threads: Arc::new(crate::thread_state::ThreadStore::default()),
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
        device_id: "a".repeat(64),
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
                .kill_on_drop(true)
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
                    let (_, usage) = state.client_keys.get_usage(&key.id).await.unwrap().unwrap();
                    assert_eq!(
                        usage.total_tokens, 715,
                        "mixed 5m/1h costs, including cancellation"
                    );
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
        for endpoint in ["/v1/messages", "/v1/chat/completions"] {
            let response=client.post(format!("{proxy_url}{endpoint}")).bearer_auth(&key.key).json(&json!({"model":"claude-sonnet-4-6","messages":[{"role":"user","content":"audit rate-limit"}]})).send().await.unwrap();
            assert_eq!(response.status(), 429);
            assert_eq!(response.headers()["retry-after"], "3");
            assert_eq!(response.headers()["retry-after-ms"], "3000");
            assert_eq!(response.headers()["x-should-retry"], "false");
            assert_eq!(response.headers()["request-id"], "audit-limit");
            assert!(!response.headers().contains_key("set-cookie"));
        }
    }
    if let Ok(binary) = std::env::var("PROXY_AUDIT_OPENCODE_BIN") {
        let workspace = root.join("opencode");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("probe.txt"), "before\n").unwrap();
        let key = state
            .client_keys
            .create("audit-opencode".into())
            .await
            .unwrap();
        let config = json!({"update":"disable","share":"disabled","snapshots":false,"providers":{"audit":{"package":"@ai-sdk/anthropic","settings":{"baseURL":format!("{proxy_url}/v1"),"apiKey":key.key},"headers":{"x-proxy-client-profile":"opencode"},"models":{"claude-sonnet-4-6":{"name":"Audit","limit":{"context":200000,"output":4096},"capabilities":{"tools":true,"reasoning":{"supported":false}}}}}}});
        std::fs::write(workspace.join("opencode.json"), config.to_string()).unwrap();
        let home = root.join("opencode-home");
        std::fs::create_dir_all(&home).unwrap();
        let output=tokio::time::timeout(Duration::from_secs(60),Command::new(binary).args(["run","--standalone","--auto","--archived","--format","json","--model","audit/claude-sonnet-4-6","audit opencode: read probe.txt, read missing.txt, replace before with after in probe.txt"])
            .current_dir(&workspace).env_clear().env("PATH",std::env::var("PATH").unwrap()).env("HOME",&home)
            .env("XDG_CONFIG_HOME",home.join("config")).env("XDG_DATA_HOME",home.join("data")).env("XDG_CACHE_HOME",home.join("cache")).env("XDG_STATE_HOME",home.join("state"))
            .env("TMPDIR",&root).kill_on_drop(true).output()).await.expect("OpenCode audit timeout").unwrap();
        std::fs::write(root.join("opencode.stdout"), &output.stdout).unwrap();
        std::fs::write(root.join("opencode.stderr"), &output.stderr).unwrap();
        assert!(
            output.status.success(),
            "OpenCode: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("probe.txt")).unwrap(),
            "after\n",
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let entries = state.client_keys.get_model_usage(&key.id).await.unwrap();
                let count = captures
                    .lock()
                    .await
                    .iter()
                    .filter(|b| {
                        b["messages"][0]["content"]
                            .to_string()
                            .contains("audit opencode")
                    })
                    .count() as u64;
                if let Some(entry) = entries.first()
                    && entry.total.input == count * 11
                {
                    assert_eq!(entry.total.output, count * 7);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("OpenCode usage in DB");
        println!("OpenCode read/error/edit tool loop passed");
    }
    let retry_key = state
        .client_keys
        .create("retry-audit".into())
        .await
        .unwrap();
    for endpoint in ["messages", "chat/completions"] {
        for streaming in [false, true] {
            let marker = format!("audit retry {endpoint} {streaming}");
            let response=client.post(format!("{proxy_url}/v1/{endpoint}")).bearer_auth(&retry_key.key).json(&json!({"model":"claude-sonnet-4-6","stream":streaming,"messages":[{"role":"user","content":marker}]})).send().await.unwrap();
            assert_eq!(response.status(), 200);
            assert!(response.text().await.unwrap().contains("done"));
            let attempts = captures
                .lock()
                .await
                .iter()
                .filter(|b| b["messages"][0]["content"].to_string().contains(&marker))
                .count();
            assert_eq!(attempts, 2);
        }
    }
    let response=client.post(format!("{proxy_url}/v1/messages")).bearer_auth(&retry_key.key).json(&json!({"model":"claude-sonnet-4-6","messages":[{"role":"user","content":"audit retry-max"}]})).send().await.unwrap();
    assert_eq!(response.status(), 529);
    assert_eq!(
        captures
            .lock()
            .await
            .iter()
            .filter(|b| b["messages"][0]["content"]
                .to_string()
                .contains("audit retry-max"))
            .count(),
        3
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let entries = state
                .client_keys
                .get_model_usage(&retry_key.id)
                .await
                .unwrap();
            if let Some(entry) = entries.first()
                && entry.total.input == 44
            {
                assert_eq!(entry.total.output, 28);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Retries must record successful usage only once");
    println!("HTTP retry audit passed: JSON/SSE, both formats, attempt cap and DB usage");
    let key = state
        .client_keys
        .create("audit-threads".into())
        .await
        .unwrap();
    let output = Command::new("uv")
        .args([
            "run",
            "--project",
            "tests/clients",
            "python",
            "tests/clients/threads.py",
        ])
        .env("PROXY_AUDIT_URL", &proxy_url)
        .env("PROXY_AUDIT_KEY", &key.key)
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
    let response=client.post(format!("{proxy_url}/v1/messages")).bearer_auth(state.client_keys.create("other-thread-client".into()).await.unwrap().key).json(&json!({"model":"claude-sonnet-4-6","thread":{"type":"continue","previous_message_id":"msg_audit"},"messages":[]})).send().await.unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["details"]["error_code"],
        "thread_not_found"
    );
    let bodies = captures.lock().await;
    assert!(bodies.len() >= 20);
    std::fs::write(
        root.join("upstream.json"),
        serde_json::to_vec_pretty(&*bodies).unwrap(),
    )
    .unwrap();
    for body in bodies.iter().filter(|b| b.get("tools").is_some()) {
        if body.get("thread").is_some() {
            continue;
        }
        if body["messages"][0]["content"]
            .to_string()
            .contains("audit opencode")
        {
            continue;
        }
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
