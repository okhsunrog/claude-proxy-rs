//! Opt-in audits using private local captures. Credentials are read from stdin
//! only for the live test and are never written or printed.
use super::*;
use axum::http::{HeaderMap, HeaderName};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn captures() -> Vec<(String, Value)> {
    let root = std::env::var("CLAUDE_PROXY_AUDIT_DIR").expect("Set CLAUDE_PROXY_AUDIT_DIR");
    let mut cases: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(Result::unwrap)
        .filter(|e| {
            e.file_name().to_string_lossy().contains("-audit-")
                && e.file_name().to_string_lossy().ends_with("-request.json")
        })
        .filter_map(|e| {
            let value: Value = serde_json::from_slice(&std::fs::read(e.path()).unwrap()).unwrap();
            value["path"]
                .as_str()
                .filter(|p| p.starts_with("/v1/messages?"))?;
            Some((e.file_name().to_string_lossy().into_owned(), value))
        })
        .collect();
    cases.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!cases.is_empty(), "No inference captures found");
    cases
}

fn incoming(case: &Value) -> HeaderMap {
    case["headers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| {
            (
                HeaderName::from_bytes(pair[0].as_str().unwrap().as_bytes()).unwrap(),
                pair[1].as_str().unwrap().parse().unwrap(),
            )
        })
        .collect()
}

fn prepare(
    body: Value,
    headers: &HeaderMap,
    device: &str,
    account: Option<&str>,
    cloak: bool,
) -> crate::transforms::prepare::PreparedRequest {
    let session = crate::identity::session_id(&body, headers, "audit-client", device);
    let mut body = body;
    crate::transforms::prepare::apply_identity(&mut body, device, account, &session);
    crate::transforms::prepare::prepare_anthropic_request(body, cloak)
}

#[tokio::test]
#[ignore = "Requires private macOS captures in CLAUDE_PROXY_AUDIT_DIR; local mock only"]
async fn captured_matrix_survives_production_request_pipeline() {
    let cases = captures();
    for (name, case) in &cases {
        let headers = incoming(case);
        let id: Value =
            serde_json::from_str(case["body"]["metadata"]["user_id"].as_str().unwrap()).unwrap();
        let prepared = prepare(
            case["body"].clone(),
            &headers,
            id["device_id"].as_str().unwrap(),
            id["account_uuid"].as_str(),
            false,
        );
        assert!(
            prepared.body == case["body"],
            "Preparation changed native body: {name}"
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let expected = case.clone();
        let label = name.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut raw = Vec::new();
            loop {
                let mut chunk = [0; 8192];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                raw.extend_from_slice(&chunk[..n]);
                let Some(offset) = raw.windows(4).position(|b| b == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&raw[..offset]);
                let actual: Vec<_> = head
                    .lines()
                    .skip(1)
                    .map(|l| {
                        let (k, v) = l.split_once(':').unwrap();
                        (k.to_owned(), v.trim().to_owned())
                    })
                    .collect();
                let length: usize = actual
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .unwrap()
                    .1
                    .parse()
                    .unwrap();
                if raw.len() < offset + 4 + length {
                    continue;
                }
                assert_eq!(
                    actual.len(),
                    expected["headers"].as_array().unwrap().len(),
                    "Header count: {label}"
                );
                for ((name, value), source) in
                    actual.iter().zip(expected["headers"].as_array().unwrap())
                {
                    assert_eq!(
                        name,
                        source[0].as_str().unwrap(),
                        "Header order/case: {label}"
                    );
                    if !name.eq_ignore_ascii_case("authorization") {
                        assert!(
                            value == source[1].as_str().unwrap(),
                            "Header value {name}: {label}"
                        );
                    }
                }
                let value: Value = serde_json::from_slice(&raw[offset + 4..]).unwrap();
                assert!(value == expected["body"], "Final body/checksum: {label}");
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .await
                    .unwrap();
                break;
            }
        });
        let request = crate::routes::auth::build_anthropic_request(
            &reqwest::Client::new(),
            &format!("http://{address}{}", case["path"].as_str().unwrap()),
            "local-audit-token",
            Some(&prepared.betas),
            "fallback",
            &prepared.body,
            Some(&headers),
        )
        .header(reqwest::header::HOST, "api.anthropic.com")
        .json(&prepared.body);
        AnthropicTransport::build(true)
            .unwrap()
            .send(request)
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        server.await.unwrap();
        println!("matched {name}");
    }
    println!(
        "Matched {} captured requests, including HTTP header order/case and final JSON/checksum",
        cases.len()
    );
}

#[tokio::test]
#[ignore = "Paid live inference: requires capture directory and access token JSON on stdin"]
async fn live_native_and_generated_inference() {
    use std::io::Read;
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).unwrap();
    let credentials: Value = serde_json::from_str(&raw).unwrap();
    let token = credentials["access_token"].as_str().unwrap();
    let (_, case) = captures()
        .into_iter()
        .find(|(n, _)| n.contains("sonnet-4-6-plain"))
        .unwrap();
    let id: Value =
        serde_json::from_str(case["body"]["metadata"]["user_id"].as_str().unwrap()).unwrap();
    for native in [true, false] {
        let mut body = if native {
            case["body"].clone()
        } else {
            json!({
                "model":"claude-sonnet-4-6", "system":"You are a helpful assistant.",
                "messages":[{"role":"user","content":"Reply exactly OK."}],"stream":true
            })
        };
        body["max_tokens"] = 64.into();
        let headers = if native {
            incoming(&case)
        } else {
            HeaderMap::new()
        };
        let prepared = prepare(
            body,
            &headers,
            id["device_id"].as_str().unwrap(),
            id["account_uuid"].as_str(),
            !native,
        );
        let request = crate::routes::auth::build_anthropic_request(
            &reqwest::Client::new(),
            crate::constants::ANTHROPIC_API_URL,
            token,
            Some(&prepared.betas),
            "fallback",
            &prepared.body,
            Some(&headers),
        )
        .json(&prepared.body)
        .timeout(Duration::from_secs(40));
        let response = AnthropicTransport::new()
            .unwrap()
            .send(request)
            .await
            .unwrap();
        let status = response.status();
        let response = response.text().await.unwrap();
        let mode = if native { "native" } else { "generated" };
        assert!(status.is_success(), "{mode} returned {status}");
        let mut answer = String::new();
        for line in response.lines().filter_map(|l| l.strip_prefix("data: ")) {
            if let Ok(event) = serde_json::from_str::<Value>(line)
                && let Some(text) = event.pointer("/delta/text").and_then(Value::as_str)
            {
                answer.push_str(text);
            }
        }
        assert!(answer.contains("OK"), "No expected response for {mode}");
        println!("{mode}: HTTP {status}; response {answer:?}");
    }
}
