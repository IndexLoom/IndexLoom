//! Authenticated process-local event stream with atomic subscribe/replay and bounded memory.
use crate::AppState;
use axum::{
    Router,
    extract::State,
    http::HeaderMap,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::broadcast;
pub(crate) fn mutation_kind(method: &axum::http::Method, path: &str) -> Option<ChangeKind> {
    use axum::http::Method;
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
        || path == "/api/v1/search/stream"
    {
        return None;
    }
    Some(if path == "/api/v1/search" {
        ChangeKind::Search
    } else if path.starts_with("/api/v1/indexer-tests") {
        ChangeKind::Tests
    } else if path.starts_with("/api/v1/indexers") {
        ChangeKind::Indexers
    } else if path.starts_with("/api/v1/automation") {
        ChangeKind::Automation
    } else if path.starts_with("/api/v1/apps") || path.starts_with("/api/v1/integrations") {
        ChangeKind::Integrations
    } else {
        ChangeKind::Settings
    })
}
const CAPACITY: usize = 256;
/// Nonsensitive resource invalidations shared by every event producer.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Indexers,
    Tests,
    Search,
    Automation,
    Integrations,
    Settings,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub id: String,
    pub kind: ChangeKind,
    pub timestamp: i64,
}
#[derive(Debug)]
struct Inner {
    sequence: u64,
    history: VecDeque<Change>,
    sender: broadcast::Sender<Change>,
}
#[derive(Clone, Debug)]
pub struct EventBus {
    epoch: Arc<str>,
    inner: Arc<Mutex<Inner>>,
}
impl Default for EventBus {
    fn default() -> Self {
        let (sender, _) = broadcast::channel(CAPACITY);
        Self {
            epoch: uuid::Uuid::now_v7().to_string().into(),
            inner: Arc::new(Mutex::new(Inner {
                sequence: 0,
                history: VecDeque::new(),
                sender,
            })),
        }
    }
}
impl EventBus {
    pub(crate) fn emit(&self, kind: ChangeKind) {
        let mut inner = self.inner.lock().expect("event lock");
        inner.sequence += 1;
        let change = Change {
            id: format!("{}:{}", self.epoch, inner.sequence),
            kind,
            timestamp: time::OffsetDateTime::now_utc().unix_timestamp(),
        };
        if inner.history.len() == CAPACITY {
            inner.history.pop_front();
        }
        inner.history.push_back(change.clone());
        let _ = inner.sender.send(change);
    }
    fn subscribe(&self, last: Option<&str>) -> (Vec<Change>, bool, broadcast::Receiver<Change>) {
        let inner = self.inner.lock().expect("event lock");
        let position = last.and_then(|id| inner.history.iter().position(|event| event.id == id));
        let reset = position.is_none();
        let replay = position.map_or_else(Vec::new, |position| {
            inner.history.iter().skip(position + 1).cloned().collect()
        });
        (replay, reset, inner.sender.subscribe())
    }
}
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/events/stream", get(stream))
}
async fn stream(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl axum::response::IntoResponse {
    let last = headers.get("last-event-id").and_then(|v| v.to_str().ok());
    let (replay, reset, mut receiver) = state.events.subscribe(last);
    let expires = tokio::time::sleep(Duration::from_secs(300));
    let mut response = Sse::new(async_stream::stream! {
        tokio::pin!(expires);
        if reset { yield Ok::<_, Infallible>(Event::default().event("reset").data("{}")); }
        for change in replay { yield Ok(event(&change)); }
        loop {
            let outcome = tokio::select! {
                result = receiver.recv() => result,
                () = &mut expires => break,
            };
            match outcome {
                Ok(change) => yield Ok(event(&change)),
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    yield Ok(Event::default().event("reset").data("{}"));
                    break;
                },
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
    .keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    )
    .into_response();
    response.headers_mut().insert(
        "x-accel-buffering",
        axum::http::HeaderValue::from_static("no"),
    );
    response
}
fn event(change: &Change) -> Event {
    Event::default()
        .id(&change.id)
        .event("change")
        .json_data(change)
        .expect("serializable event")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_is_ordered_bounded_and_handles_restart() {
        let bus = EventBus::default();
        bus.emit(ChangeKind::Indexers);
        let id = bus.inner.lock().unwrap().history[0].id.clone();
        bus.emit(ChangeKind::Search);
        let (replay, reset, _) = bus.subscribe(Some(&id));
        assert!(!reset);
        assert_eq!(replay[0].kind, ChangeKind::Search);
        assert!(
            serde_json::to_string(&replay[0])
                .unwrap()
                .contains("timestamp")
        );
        for _ in 0..CAPACITY {
            bus.emit(ChangeKind::Settings);
        }
        assert!(bus.subscribe(Some(&id)).1);
        assert!(bus.subscribe(Some("previous-process:1")).1);
        assert_eq!(bus.inner.lock().unwrap().history.len(), CAPACITY);
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    const TOKEN: &str = "event-test-token-at-least-thirty-two-bytes";
    async fn state() -> AppState {
        AppState::new(
            indexloom_storage::Store::connect("sqlite::memory:")
                .await
                .unwrap(),
            indexloom_storage::SecretVault::generate().unwrap(),
            crate::AdminToken::new(TOKEN).unwrap(),
        )
    }
    #[tokio::test]
    async fn actual_successful_mutation_emits_and_reads_do_not() {
        let state = state().await;
        let app = crate::router(state.clone(), std::path::PathBuf::from("/nonexistent"));
        let response = app.clone().oneshot(Request::builder().method("PUT").uri("/api/v1/automation/jobs/maintenance")
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"kind":"maintenance","intervalSeconds":60,"enabled":true,"target":null}"#)).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            state
                .events
                .inner
                .lock()
                .unwrap()
                .history
                .back()
                .unwrap()
                .kind,
            ChangeKind::Automation
        );
        let before = state.events.inner.lock().unwrap().history.len();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/automation/jobs")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(state.events.inner.lock().unwrap().history.len(), before);
    }
    #[tokio::test]
    async fn auth_replay_frames_and_lag_reset() {
        let state = state().await;
        state.events.emit(ChangeKind::Indexers);
        let id = state.events.inner.lock().unwrap().history[0].id.clone();
        state.events.emit(ChangeKind::Tests);
        let app = crate::router(state.clone(), std::path::PathBuf::from("/nonexistent"));
        for path in [
            "/api/v1/events/stream",
            "/api/v1/system/updates",
            "/api/v1/system/updates/history",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/events/stream")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .header("last-event-id", id)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/event-stream")
        );
        let mut body = response.into_body();
        let frame = body.frame().await.unwrap().unwrap().into_data().unwrap();
        let text = std::str::from_utf8(&frame).unwrap();
        assert!(text.contains("event: change"));
        assert!(text.contains("\"kind\":\"tests\""));
        assert!(text.contains("id: "));
        for _ in 0..=CAPACITY {
            state.events.emit(ChangeKind::Indexers);
        }
        let frame = body.frame().await.unwrap().unwrap().into_data().unwrap();
        assert!(
            std::str::from_utf8(&frame)
                .unwrap()
                .contains("event: reset")
        );
        assert!(body.frame().await.is_none());
    }
}
