//! Bounded, process-local alias state. A cache miss asks the client to replay;
//! the proxy never reconstructs or resends private conversation history.
use crate::{error::ProxyError, transforms::tool_aliases::ToolNameMap};
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const TTL: Duration = Duration::from_secs(300);
const MAX_ENTRIES: usize = 1024;
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct Scope {
    pub key: String,
    pub account: String,
    pub model: String,
}

#[derive(Clone)]
pub struct ThreadState {
    pub tools: ToolNameMap,
    pub session: String,
    pub transform_thinking: bool,
}

struct Entry {
    scope: Scope,
    id: String,
    state: ThreadState,
    expires: Instant,
}
impl Entry {
    fn bytes(&self) -> usize {
        self.scope.key.len()
            + self.scope.account.len()
            + self.scope.model.len()
            + self.id.len()
            + self.state.session.len()
            + self.state.tools.estimated_bytes()
            + std::mem::size_of::<Self>()
    }
}

#[derive(Default)]
pub struct ThreadStore {
    entries: Mutex<VecDeque<Entry>>,
}

impl ThreadStore {
    pub async fn load(&self, scope: &Scope, id: &str) -> Option<ThreadState> {
        let mut entries = self.entries.lock().await;
        let now = Instant::now();
        entries.retain(|e| e.expires > now);
        entries
            .iter()
            .find(|e| e.scope == *scope && e.id == id)
            .map(|e| e.state.clone())
    }
    async fn save(&self, scope: Scope, id: String, state: ThreadState) {
        let now = Instant::now();
        let entry = Entry {
            scope,
            id,
            state,
            expires: now + TTL,
        };
        let size = entry.bytes();
        if size > MAX_BYTES {
            return;
        }
        let mut entries = self.entries.lock().await;
        entries.retain(|e| e.expires > now && !(e.scope == entry.scope && e.id == entry.id));
        let mut bytes: usize = entries.iter().map(Entry::bytes).sum();
        while entries.len() >= MAX_ENTRIES || bytes.saturating_add(size) > MAX_BYTES {
            if let Some(old) = entries.pop_front() {
                bytes = bytes.saturating_sub(old.bytes());
            } else {
                break;
            }
        }
        entries.push_back(entry);
    }
}

#[derive(Clone)]
pub struct Turn {
    pub store: Arc<ThreadStore>,
    pub scope: Scope,
    pub state: ThreadState,
}
impl Turn {
    pub async fn record(&self, message: &Value) {
        if let Some(id) = message
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 512)
        {
            self.store
                .save(self.scope.clone(), id.to_owned(), self.state.clone())
                .await;
        }
    }
}

pub fn mode(body: &Value) -> Result<Option<&'static str>, ProxyError> {
    let Some(thread) = body.get("thread") else {
        return Ok(None);
    };
    match thread.get("type").and_then(Value::as_str) {
        Some("create") => Ok(Some("create")),
        Some("continue") => {
            if body.get("system").is_some() || body.get("tools").is_some() {
                return Err(ProxyError::InvalidRequest("Thread continuation must omit system and tools; replay with thread create to change them".into()));
            }
            if !thread
                .get("previous_message_id")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty() && id.len() <= 512)
            {
                return Err(ProxyError::InvalidRequest(
                    "Thread continuation requires previous_message_id".into(),
                ));
            }
            Ok(Some("continue"))
        }
        _ => Err(ProxyError::InvalidRequest(
            "Expected thread type create or continue".into(),
        )),
    }
}

/// Save only completed streams, before their terminal event reaches the client.
pub fn track(
    events: impl futures_util::Stream<Item = Result<Value, std::io::Error>> + Send,
    turn: Option<Turn>,
) -> impl futures_util::Stream<Item = Result<Value, std::io::Error>> + Send {
    use futures_util::StreamExt;
    async_stream::stream! {
        let mut events=std::pin::pin!(events);
        let mut message=None;
        while let Some(event)=events.next().await {
            if let Ok(event)=&event {
                match event.get("type").and_then(Value::as_str) {
                    Some("message_start")=>message=event.get("message").cloned(),
                    Some("message_stop")=>if let (Some(turn),Some(message))=(&turn,&message) {turn.record(message).await;},
                    _=>{}
                }
            }
            yield event;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope(key: &str, account: &str) -> Scope {
        Scope {
            key: key.into(),
            account: account.into(),
            model: "model".into(),
        }
    }
    fn state() -> ThreadState {
        ThreadState {
            tools: ToolNameMap::default(),
            session: "session".into(),
            transform_thinking: true,
        }
    }
    #[tokio::test]
    async fn isolates_keys_accounts_models_expires_and_evicts() {
        let store = ThreadStore::default();
        let a = scope("a", "account");
        store.save(a.clone(), "id".into(), state()).await;
        assert!(store.load(&a, "id").await.is_some());
        assert!(store.load(&scope("b", "account"), "id").await.is_none());
        assert!(store.load(&scope("a", "other"), "id").await.is_none());
        let mut other = a.clone();
        other.model = "other".into();
        assert!(store.load(&other, "id").await.is_none());
        {
            let mut entries = store.entries.lock().await;
            for e in entries.iter_mut() {
                e.expires = Instant::now();
            }
        }
        assert!(store.load(&a, "id").await.is_none());
        for n in 0..=MAX_ENTRIES {
            store.save(a.clone(), n.to_string(), state()).await;
        }
        assert!(store.load(&a, "0").await.is_none());
        assert!(store.load(&a, &MAX_ENTRIES.to_string()).await.is_some());
    }

    #[tokio::test]
    async fn only_completed_streams_publish_state() {
        use futures_util::{StreamExt, stream};
        for complete in [false, true] {
            let store = Arc::new(ThreadStore::default());
            let scope = scope("key", "account");
            let turn = Turn {
                store: store.clone(),
                scope: scope.clone(),
                state: state(),
            };
            let mut events = vec![Ok(
                serde_json::json!({"type":"message_start","message":{"id":"response"}}),
            )];
            if complete {
                events.push(Ok(serde_json::json!({"type":"message_stop"})));
            }
            let _: Vec<_> = track(stream::iter(events), Some(turn)).collect().await;
            assert_eq!(store.load(&scope, "response").await.is_some(), complete);
        }
    }
}
