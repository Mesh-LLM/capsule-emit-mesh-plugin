//! A node with the default settings sends nothing to any peer on its own,
//! idle or serving: the compiled `capsules` binary, driven over the real
//! plugin wire protocol by a fake host, is handed a completed exchange with a
//! known counterparty and must not open a mesh stream, send a channel message
//! or a bulk transfer, or ask for a peer block. The control: the same run with
//! `share_record_at_completion = counterparty` does open a mesh stream to the
//! counterparty, so the quiet result is not just a test that cannot see a
//! push.

use mesh_llm_plugin::proto::{self, envelope::Payload};
use mesh_llm_plugin::{
    read_envelope, write_envelope, LocalListener, LocalStream, PROTOCOL_VERSION,
};
use std::time::Duration;
use tokio::net::UnixListener;
use tokio::process::Command;
use tokio::time::timeout;

const PLUGIN_BIN: &str = env!("CARGO_BIN_EXE_capsules");
const SELF_PEER: &str = "aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11";
const SERVING_PEER: &str = "bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22";

fn nonce() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

struct Host {
    child: tokio::process::Child,
    stream: LocalStream,
    next: u64,
}

impl Host {
    async fn spawn(extra_env: &[(&str, &str)]) -> Self {
        let socket = std::env::temp_dir().join(format!("capsules-quiet-{}.sock", nonce()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let mut cmd = Command::new(PLUGIN_BIN);
        cmd.env("MESH_LLM_PLUGIN_ENDPOINT", &socket)
            .env("MESH_LLM_PLUGIN_TRANSPORT", "unix")
            .env(
                "CAPSULES_DATA_DIR",
                std::env::temp_dir().join(format!("capsules-quiet-data-{}", nonce())),
            )
            .env("CAPSULES_SELF_PEER_ID", SELF_PEER)
            // Never the developer's own mesh config or settings.
            .env("MESH_LLM_CONFIG", "/nonexistent/config.toml")
            .env_remove("CAPSULES_SHARE_RECORD_AT_COMPLETION")
            .env_remove("CAPSULES_SHARE_ADJUDICATIONS")
            .env_remove("CAPSULES_ADJUDICATE_DIFFERING_TWINS")
            .env_remove("CAPSULES_CHECKPOINT_WITNESS_URLS");
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().unwrap();
        let stream = timeout(
            Duration::from_secs(10),
            LocalListener::Unix(listener, socket).accept(),
        )
        .await
        .unwrap()
        .unwrap();
        let mut host = Self {
            child,
            stream,
            next: 1,
        };
        host.send(Payload::InitializeRequest(proto::InitializeRequest {
            host_protocol_version: PROTOCOL_VERSION,
            host_version: "quiet-default-test".into(),
            host_info_json: "{}".into(),
            mesh_visibility: proto::MeshVisibility::Private as i32,
            ..Default::default()
        }))
        .await;
        let reply = host.recv(Duration::from_secs(10)).await.unwrap();
        assert!(matches!(
            reply.payload,
            Some(Payload::InitializeResponse(_))
        ));
        host
    }

    async fn send(&mut self, payload: Payload) {
        let request_id = self.next;
        self.next += 1;
        write_envelope(
            &mut self.stream,
            &proto::Envelope {
                protocol_version: PROTOCOL_VERSION,
                plugin_id: "capsules".into(),
                request_id,
                payload: Some(payload),
            },
        )
        .await
        .unwrap();
    }

    async fn recv(&mut self, wait: Duration) -> Option<proto::Envelope> {
        timeout(wait, read_envelope(&mut self.stream))
            .await
            .ok()?
            .ok()
    }

    /// Everything the plugin sends that would reach a peer (or the routing
    /// plane), over `window`. Rpc replies to the host are not counted.
    async fn peer_bound_within(&mut self, window: Duration) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + window;
        let mut seen = Vec::new();
        while let Some(left) = deadline.checked_duration_since(tokio::time::Instant::now()) {
            let Some(envelope) = self.recv(left).await else {
                break;
            };
            match envelope.payload {
                Some(Payload::OpenMeshStreamRequest(r)) => {
                    seen.push(format!("open_mesh_stream {r:?}"))
                }
                Some(Payload::ChannelMessage(m)) => {
                    seen.push(format!("channel_message {}", m.channel))
                }
                Some(Payload::BulkTransferMessage(_)) => seen.push("bulk_transfer".into()),
                Some(Payload::PeerBlockRequest(r)) => {
                    seen.push(format!("peer_block {}", r.peer_id))
                }
                _ => {}
            }
        }
        seen
    }

    /// A completed exchange this node asked for and `SERVING_PEER` served.
    async fn completed_exchange(&mut self) {
        let event = serde_json::json!({
            "dispatch_path": "remote_mesh",
            "phase": "terminal",
            "model": "quiet-test-model",
            "status": 200,
            "exchange_id": format!("quiet-{}", nonce()),
            "capsule_id": null,
            "nonce": null,
            "request_digest": "11".repeat(32),
            "response_digest": "22".repeat(32),
            "serving_provenance": {"served_by_node_id": SERVING_PEER},
        });
        self.send(Payload::ChannelMessage(proto::ChannelMessage {
            channel: "openai.exchange.v1".into(),
            source_peer_id: String::new(),
            target_peer_id: "capsules".into(),
            content_type: "application/json".into(),
            body: serde_json::to_vec(&event).unwrap(),
            ..Default::default()
        }))
        .await;
    }

    async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}

#[tokio::test]
async fn a_default_node_sends_nothing_to_peers_idle_or_serving() {
    let mut host = Host::spawn(&[]).await;
    assert!(
        host.peer_bound_within(Duration::from_secs(5))
            .await
            .is_empty(),
        "idle"
    );
    host.completed_exchange().await;
    let sent = host.peer_bound_within(Duration::from_secs(8)).await;
    assert!(sent.is_empty(), "serving, default settings: {sent:?}");
    host.stop().await;
}

/// The control: with the push turned on, the same exchange does reach the
/// counterparty, so the test above can see a push when one happens.
#[tokio::test]
async fn with_the_push_turned_on_the_counterparty_is_contacted() {
    let mut host = Host::spawn(&[("CAPSULES_SHARE_RECORD_AT_COMPLETION", "counterparty")]).await;
    host.completed_exchange().await;
    let sent = host.peer_bound_within(Duration::from_secs(8)).await;
    assert!(
        sent.iter()
            .any(|s| s.starts_with("open_mesh_stream") && s.contains(SERVING_PEER)),
        "expected a push to the counterparty, saw {sent:?}"
    );
    host.stop().await;
}

/// A local witness that answers every request with 503 and keeps the request
/// line and `Host` header of each, so a test can see exactly where the plugin
/// went.
fn recording_witness() -> (String, std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let keep = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut buf = vec![0u8; 16 * 1024];
            let n = stream.read(&mut buf).unwrap_or(0);
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            let line = text.lines().next().unwrap_or_default().to_string();
            let host = text
                .lines()
                .find_map(|l| {
                    l.split_once(':')
                        .filter(|(k, _)| k.eq_ignore_ascii_case("host"))
                        .map(|(_, v)| v.trim().to_string())
                })
                .unwrap_or_default();
            keep.lock().unwrap().push((line, host));
            let _ = stream.write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            );
        }
    });
    (addr, seen)
}

/// A witness URL is dialled exactly as the operator gave it: the same host and
/// port, under the same path, for the plugin's own key fetch and for the
/// checkpoint registration alike. No URL is mapped to another service.
#[tokio::test]
async fn a_configured_witness_is_dialled_exactly_as_given() {
    let (addr, seen) = recording_witness();
    let url = format!("http://{addr}/given/prefix");
    let mut host = Host::spawn(&[
        ("CAPSULES_CHECKPOINT_WITNESS_URLS", url.as_str()),
        ("CAPSULES_CHECKPOINT_CADENCE_SECONDS", "1"),
        ("CAPSULES_CHECKPOINT_CADENCE_ENTRIES", "1"),
    ])
    .await;
    host.completed_exchange().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let requests = seen.lock().unwrap().clone();
        let fetched_key = requests
            .iter()
            .any(|(line, _)| line.starts_with("GET /given/prefix/anchor/authority-pubkey "));
        let registered = requests
            .iter()
            .any(|(line, _)| line.starts_with("POST /given/prefix/"));
        if fetched_key && registered {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected the key fetch and a checkpoint registration, saw {requests:?}"
        );
        // Keep the host side drained while the plugin works.
        let _ = host.peer_bound_within(Duration::from_millis(500)).await;
    }
    for (line, host_header) in seen.lock().unwrap().iter() {
        assert_eq!(host_header, &addr, "{line}");
        let path = line.split(' ').nth(1).unwrap_or_default();
        assert!(path.starts_with("/given/prefix/"), "{line}");
    }
    host.stop().await;
}
