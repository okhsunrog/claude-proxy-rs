//! macOS x64 inference transport, verified against a local 2.1.326 VM capture.
//! See fixtures/claude-2.1.326-macos-x64-tls.json for capture provenance.
//! Header identity and model-visible working context are separate concerns.
//! Upstream profile attribution and MIT notice: licenses/CLIProxyAPI.txt.
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::TryStreamExt;
use wreq::{
    header::OrigHeaderMap,
    tls::{AlpnProtocol, ExtensionType as E, KeyShare, TlsOptions, TlsVersion},
};

const HEADER_ORDER: &[&str] = &[
    "Accept",
    "Authorization",
    "Content-Type",
    "User-Agent",
    "X-Claude-Code-Session-Id",
    "X-Stainless-Arch",
    "X-Stainless-Lang",
    "X-Stainless-OS",
    "X-Stainless-Package-Version",
    "X-Stainless-Retry-Count",
    "X-Stainless-Runtime",
    "X-Stainless-Runtime-Version",
    "X-Stainless-Timeout",
    "anthropic-beta",
    "anthropic-dangerous-direct-browser-access",
    "anthropic-version",
    "x-app",
    "x-claude-1p-request-class",
    "x-claude-code-prompt-id",
    "x-claude-code-request-class",
    "x-claude-code-sandy-plover",
    "x-client-request-id",
    "Connection",
    "Host",
    "Accept-Encoding",
    "Content-Length",
];

fn tls_options() -> TlsOptions {
    TlsOptions::builder()
        .alpn_protocols([AlpnProtocol::HTTP1])
        .min_tls_version(TlsVersion::TLS_1_2)
        .max_tls_version(TlsVersion::TLS_1_3)
        .grease_enabled(false)
        .permute_extensions(false)
        .enable_ocsp_stapling(true)
        .enable_signed_cert_timestamps(true)
        .curves_list("X25519MLKEM768:X25519:P-256:P-384")
        .key_shares(vec![KeyShare::X25519_MLKEM768, KeyShare::X25519])
        .cipher_list(concat!(
            "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:",
            "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:",
            "ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:",
            "ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:",
            "ECDHE-ECDSA-AES128-SHA:ECDHE-RSA-AES128-SHA:",
            "ECDHE-ECDSA-AES256-SHA:ECDHE-RSA-AES256-SHA:",
            "AES128-GCM-SHA256:AES256-GCM-SHA384:AES128-SHA:AES256-SHA"
        ))
        .preserve_tls13_cipher_list(true)
        .sigalgs_list("ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:rsa_pkcs1_sha256:ecdsa_secp384r1_sha384:rsa_pss_rsae_sha384:rsa_pkcs1_sha384:rsa_pss_rsae_sha512:rsa_pkcs1_sha512:ed25519:ecdsa_secp521r1_sha512:rsa_pkcs1_sha1")
        .extension_permutation(vec![
            E::SERVER_NAME, E::EXTENDED_MASTER_SECRET, E::RENEGOTIATE,
            E::SUPPORTED_GROUPS, E::EC_POINT_FORMATS, E::SESSION_TICKET,
            E::APPLICATION_LAYER_PROTOCOL_NEGOTIATION, E::STATUS_REQUEST,
            E::SIGNATURE_ALGORITHMS, E::CERTIFICATE_TIMESTAMP, E::KEY_SHARE,
            E::PSK_KEY_EXCHANGE_MODES, E::SUPPORTED_VERSIONS, E::PADDING, E::PRE_SHARED_KEY,
        ])
        .build()
}

pub struct AnthropicTransport {
    client: wreq::Client,
}

impl AnthropicTransport {
    pub fn new() -> Result<Self> {
        Self::build(false)
    }

    fn build(no_proxy: bool) -> Result<Self> {
        let mut order = OrigHeaderMap::new();
        for &name in HEADER_ORDER {
            order.insert(name);
        }
        let builder = wreq::Client::builder()
            .tls_options(tls_options())
            .http1_only()
            .orig_headers(order)
            .redirect(wreq::redirect::Policy::none())
            .timeout(Duration::from_secs(300));
        let builder = if no_proxy {
            builder.no_proxy()
        } else {
            builder
        };
        Ok(Self {
            client: builder
                .build()
                .context("Failed to initialize inference TLS profile")?,
        })
    }

    /// Keep existing response/streaming handlers while using the dedicated TLS
    /// connection for inference. The streaming body is never buffered here.
    pub async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let request = request.build()?;
        let body = request
            .body()
            .map(|b| {
                b.as_bytes()
                    .context("Inference body must be replayable JSON")
            })
            .transpose()?
            .unwrap_or_default()
            .to_vec();
        let body = if request.url().path() == "/v1/messages" {
            crate::transforms::billing::finalize(&body)?
        } else {
            body
        };
        let mut headers = request.headers().clone();
        // Adding a CCH field can change the body length. Let the transport
        // calculate framing from the finalized bytes, including on retries.
        headers.remove(reqwest::header::CONTENT_LENGTH);
        let response = self
            .client
            .request(request.method().clone(), request.url().as_str())
            .headers(headers)
            .body(body)
            .send()
            .await?;
        let mut result = axum::http::Response::builder()
            .status(response.status())
            .version(response.version());
        *result
            .headers_mut()
            .context("Invalid upstream response headers")? = response.headers().clone();
        let stream = response.bytes_stream().map_err(std::io::Error::other);
        Ok(reqwest::Response::from(
            result.body(reqwest::Body::wrap_stream(stream))?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn take<'a>(input: &mut &'a [u8], n: usize) -> &'a [u8] {
        let (value, rest) = input.split_at(n);
        *input = rest;
        value
    }
    fn short(input: &mut &[u8]) -> usize {
        let bytes = take(input, 2);
        u16::from_be_bytes([bytes[0], bytes[1]]) as usize
    }
    fn shorts(input: &[u8]) -> Vec<u16> {
        input
            .chunks_exact(2)
            .map(|x| u16::from_be_bytes([x[0], x[1]]))
            .collect()
    }

    #[tokio::test]
    async fn finalizes_checksum_and_content_length_on_the_wire() {
        let mut body: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/claude-2.1.326-macos-x64-cch.json"))
                .unwrap();
        body["metadata"]["user_id"] = "proxy-owner".into();
        body["system"][0]["text"] = body["system"][0]["text"]
            .as_str()
            .unwrap()
            .replace(" cch=23526;", "")
            .into();
        let input_length = serde_json::to_vec(&body).unwrap().len();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/messages?beta=true",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                let Some(offset) = bytes.windows(4).position(|b| b == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&bytes[..offset]);
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                if bytes.len() < offset + 4 + length {
                    continue;
                }
                assert_eq!(length, input_length + " cch=00000;".len());
                let value: serde_json::Value =
                    serde_json::from_slice(&bytes[offset + 4..]).unwrap();
                // Independently calculated from the native VM fixture with only
                // metadata.user_id changed; excludes model/dispatch fields.
                assert!(
                    value["system"][0]["text"]
                        .as_str()
                        .unwrap()
                        .contains(" cch=b662b;")
                );
                assert_eq!(value["metadata"]["user_id"], "proxy-owner");
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .await
                    .unwrap();
                break;
            }
        });
        let request = reqwest::Client::new()
            .post(url)
            .header(reqwest::header::CONTENT_LENGTH, input_length)
            .json(&body);
        let response = AnthropicTransport::build(true)
            .unwrap()
            .send(request)
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn client_hello_matches_macos_vm_capture() {
        let reference: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/claude-2.1.326-macos-x64-tls.json"))
                .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = AnthropicTransport::build(true).unwrap();
        let request = reqwest::Client::new()
            .post(format!("https://localhost:{port}/v1/messages"))
            .body("{}");
        let task = tokio::spawn(async move { client.send(request).await });
        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut record = [0; 5];
        stream.read_exact(&mut record).await.unwrap();
        assert_eq!(record[0], 22);
        let mut bytes = vec![0; usize::from(u16::from_be_bytes([record[3], record[4]]))];
        stream.read_exact(&mut bytes).await.unwrap();
        // Close before a certificate or any HTTP data is transmitted.
        drop(stream);
        assert!(task.await.unwrap().is_err());
        let mut hello = bytes.as_slice();
        assert_eq!(take(&mut hello, 1), &[1]);
        take(&mut hello, 3 + 2 + 32);
        let session_length = take(&mut hello, 1)[0] as usize;
        take(&mut hello, session_length);
        let cipher_length = short(&mut hello);
        let ciphers = shorts(take(&mut hello, cipher_length));
        assert_eq!(serde_json::json!(ciphers), reference["ciphers"]);
        let compression_length = take(&mut hello, 1)[0] as usize;
        assert_eq!(take(&mut hello, compression_length), &[0]);
        let extension_length = short(&mut hello);
        let mut extensions = take(&mut hello, extension_length);
        let mut ids = Vec::new();
        let mut data = std::collections::BTreeMap::new();
        while !extensions.is_empty() {
            let id = short(&mut extensions) as u16;
            let len = short(&mut extensions);
            ids.push(id);
            data.insert(id, take(&mut extensions, len).to_vec());
        }
        assert_eq!(serde_json::json!(ids), reference["extensions"]);
        assert_eq!(
            serde_json::json!(shorts(&data[&10][2..])),
            reference["groups"]
        );
        assert_eq!(
            serde_json::json!(&data[&11][1..]),
            reference["point_formats"]
        );
        assert_eq!(&data[&16], b"\x00\x09\x08http/1.1");
        assert_eq!(&data[&43], &[4, 3, 4, 3, 3]);
        assert_eq!(
            serde_json::json!(shorts(&data[&13][2..])),
            reference["signature_algorithms"]
        );
        let mut shares = &data[&51][2..];
        let mut share_sizes = Vec::new();
        while !shares.is_empty() {
            let group = short(&mut shares);
            let len = short(&mut shares);
            take(&mut shares, len);
            share_sizes.push([group, len]);
        }
        assert_eq!(serde_json::json!(share_sizes), reference["key_shares"]);
    }

    #[tokio::test]
    async fn preserves_http_header_order_and_streams_without_buffering() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/messages?beta=true",
            listener.local_addr().unwrap()
        );
        let (release, released) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                received.extend_from_slice(&buffer[..n]);
                if let Some(end) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&received[..end]);
                    let length: usize = head
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap();
                    if received.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let wire = String::from_utf8(received).unwrap();
            assert!(wire.starts_with("POST /v1/messages?beta=true HTTP/1.1\r\n"));
            assert!(wire.contains("X-Stainless-OS: MacOS\r\n"));
            assert!(wire.contains("X-Stainless-Arch: x64\r\n"));
            let mut previous = 0;
            for name in HEADER_ORDER {
                if let Some(pos) = wire.find(&format!("\r\n{name}:")) {
                    assert!(pos > previous, "header order: {name}");
                    previous = pos;
                }
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nfirst\r\n").await.unwrap();
            released.await.unwrap();
            socket.write_all(b"6\r\nsecond\r\n0\r\n\r\n").await.unwrap();
        });
        let client = AnthropicTransport::build(true).unwrap();
        let request = crate::routes::auth::build_anthropic_request(
            &reqwest::Client::new(),
            &url,
            "local-test-token",
            None,
            "session",
            &serde_json::json!({}),
            None,
        )
        .body("{}");
        let response = tokio::time::timeout(Duration::from_secs(5), client.send(request))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        assert_eq!(stream.next().await.unwrap().unwrap().as_ref(), b"first");
        release.send(()).unwrap();
        assert_eq!(stream.next().await.unwrap().unwrap().as_ref(), b"second");
        assert!(stream.next().await.is_none());
        server.await.unwrap();
    }
}
