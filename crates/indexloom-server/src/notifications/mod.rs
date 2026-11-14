//! Owner-controlled external notifications, independent of authentication mail.
mod api;
mod model;
mod providers;
#[cfg(test)]
mod tests;
use crate::{ApiError, AppState};
pub(crate) use api::routes;
use indexloom_storage::notifications::{Channel, Template};
pub(crate) use model::Event;
use model::{EVENTS, Message, render};
use std::time::{Duration, Instant};

fn message(event: &Event, channel: &str, templates: &[Template]) -> Message {
    let (_, title, body) = EVENTS
        .iter()
        .find(|e| e.0 == event.name)
        .copied()
        .unwrap_or((
            "test",
            "IndexLoom test",
            "Your notification channel is working.",
        ));
    let time = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let template = templates
        .iter()
        .find(|t| t.scope == channel && t.event == event.name)
        .or_else(|| {
            templates
                .iter()
                .find(|t| t.scope.is_empty() && t.event == event.name)
        });
    let vars = [
        ("title", title),
        ("message", body),
        ("event", event.name),
        ("time", time.as_str()),
        ("app", "IndexLoom"),
        ("indexer", event.indexer.as_str()),
        ("job", event.job.as_str()),
    ];
    Message {
        title: model::clip(
            &render(template.map_or("{{title}}", |t| t.title.as_str()), &vars),
            250,
        ),
        body: model::clip(
            &render(template.map_or("{{message}}", |t| t.body.as_str()), &vars),
            4000,
        ),
        event: event.name.into(),
        time,
    }
}
/// Producers only write the outbox; network I/O is isolated in the worker.
pub(crate) async fn emit(state: &AppState, event: Event) {
    if enqueue(state, &event).await.is_err() {
        tracing::warn!("notification outbox write failed");
    }
}
async fn enqueue(state: &AppState, event: &Event) -> Result<(), ApiError> {
    let channels = state.store.notification_channels(&state.vault).await?;
    let templates = state.store.notification_templates().await?;
    for c in channels {
        let subscribed: Vec<String> = serde_json::from_str(&c.events).unwrap_or_default();
        if c.enabled && subscribed.iter().any(|e| e == event.name) {
            let payload = serde_json::to_string(&message(event, &c.id, &templates))
                .map_err(|_| ApiError::internal("notification rendering failed"))?;
            state
                .store
                .enqueue_notification(&c, event.name, &event.key, &payload, false)
                .await?;
        }
    }
    Ok(())
}
fn backoff(attempt: i64) -> i64 {
    30 * (1_i64 << (attempt - 1).clamp(0, 4))
}
async fn deliver(
    state: &AppState,
    client: &reqwest::Client,
    pending: indexloom_storage::notifications::Pending,
    lease: &str,
    test: bool,
) -> Result<TestResult, ApiError> {
    let start = Instant::now();
    let channel = state
        .store
        .notification_channel(&pending.channel_id, &state.vault)
        .await?;
    let result = match channel {
        Some(c) if c.revision != pending.channel_revision => Err(providers::Failure {
            summary: "Channel configuration changed",
            retryable: false,
        }),
        Some(c) if c.enabled || test => match serde_json::from_str::<Message>(&pending.payload) {
            Ok(m) => providers::send(client, &c, &m).await,
            Err(_) => Err(providers::Failure {
                summary: "Invalid queued message",
                retryable: false,
            }),
        },
        _ => Err(providers::Failure {
            summary: "Channel disabled or deleted",
            retryable: false,
        }),
    };
    let duration_ms = i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX);
    let (status, summary) = match result {
        Ok(()) => ("sent", "Provider accepted notification"),
        Err(f) => (
            if f.retryable && pending.attempts < 5 && !test {
                "retry"
            } else {
                "failed"
            },
            f.summary,
        ),
    };
    state
        .store
        .finish_notification(
            &pending.id,
            lease,
            status,
            summary,
            duration_ms,
            backoff(pending.attempts),
        )
        .await?;
    Ok(TestResult {
        success: status == "sent",
        status: status.into(),
        summary: summary.into(),
        duration_ms,
    })
}
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TestResult {
    success: bool,
    status: String,
    summary: String,
    duration_ms: i64,
}
pub(super) async fn test_send(state: &AppState, c: &Channel) -> Result<TestResult, ApiError> {
    let v = model::view(c)?;
    let input = model::ChannelInput {
        name: v.name,
        provider: v.provider,
        enabled: v.enabled,
        config: v.config,
        events: v.events,
        secrets: std::collections::BTreeMap::default(),
        revision: v.revision,
    };
    model::validate(&input, &c.secrets, true)?;
    let event = Event::new("test", uuid::Uuid::now_v7().to_string());
    let payload = serde_json::to_string(&message(&event, &c.id, &[]))
        .map_err(|_| ApiError::internal("notification rendering failed"))?;
    let id = state
        .store
        .enqueue_notification(c, "test", &event.key, &payload, true)
        .await?
        .ok_or_else(|| ApiError::conflict("notification queue full or channel changed"))?;
    let lease = uuid::Uuid::now_v7().to_string();
    let pending = state
        .store
        .claim_notification(&lease, Some(&id))
        .await?
        .ok_or_else(|| ApiError::conflict("test already running"))?;
    let client = providers::client()
        .map_err(|_| ApiError::internal("notification transport unavailable"))?;
    deliver(state, &client, pending, &lease, true).await
}
/// Start once per runtime. Leases permit multiple processes sharing a database.
pub async fn run(state: AppState) {
    let Ok(client) = providers::client() else {
        tracing::error!("notification transport unavailable");
        return;
    };
    emit(
        &state,
        Event::new("system.startup", uuid::Uuid::now_v7().to_string()),
    )
    .await;
    let mut ticker = tokio::time::interval(Duration::from_secs(2));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_prune = None::<Instant>;
    loop {
        ticker.tick().await;
        if last_prune.is_none_or(|at| at.elapsed() >= Duration::from_secs(60)) {
            if state.store.prune_notifications().await.is_err() {
                tracing::warn!("notification retention failed");
            }
            last_prune = Some(Instant::now());
        }
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..4 {
            let lease = uuid::Uuid::now_v7().to_string();
            match state.store.claim_notification(&lease, None).await {
                Ok(Some(pending)) => {
                    let state = state.clone();
                    let client = client.clone();
                    tasks.spawn(async move {
                        if deliver(&state, &client, pending, &lease, false)
                            .await
                            .is_err()
                        {
                            tracing::warn!("notification delivery state update failed");
                        }
                    });
                }
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!("notification outbox unavailable");
                    break;
                }
            }
        }
        while let Some(result) = tasks.join_next().await {
            if result.is_err() {
                tracing::warn!("notification worker interrupted; lease will recover");
            }
        }
    }
}
