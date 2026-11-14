use super::{
    message,
    model::{
        ChannelInput, ChannelView, EVENTS, Event, Message, known_event, validate,
        validate_template, view,
    },
    test_send,
};
use crate::{ApiError, AppState};
use axum::{
    Json, Router,
    extract::{Path, Query, State, rejection::JsonRejection},
    routing::{get, put},
};
use indexloom_storage::notifications::{Channel, Delivery, Template};
use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/notifications/channels", get(channels).post(create))
        .route(
            "/api/v1/notifications/channels/{id}",
            put(update).delete(delete),
        )
        .route(
            "/api/v1/notifications/channels/{id}/test",
            axum::routing::post(test),
        )
        .route("/api/v1/notifications/templates", get(templates))
        .route(
            "/api/v1/notifications/templates/{event}",
            put(save_template).delete(reset_template),
        )
        .route(
            "/api/v1/notifications/preview",
            axum::routing::post(preview),
        )
        .route("/api/v1/notifications/deliveries", get(deliveries))
        .route("/api/v1/notifications/events", get(events))
}
async fn channels(State(s): State<AppState>) -> Result<Json<Vec<ChannelView>>, ApiError> {
    let channels = s.store.notification_channels(&s.vault).await?;
    Ok(Json(channels.iter().map(view).collect::<Result<_, _>>()?))
}
async fn load(s: &AppState, id: &str) -> Result<Channel, ApiError> {
    s.store
        .notification_channel(id, &s.vault)
        .await?
        .ok_or_else(|| ApiError::not_found("notification channel not found"))
}
async fn create(
    State(s): State<AppState>,
    input: Result<Json<ChannelInput>, JsonRejection>,
) -> Result<Json<ChannelView>, ApiError> {
    let Json(input) =
        input.map_err(|_| ApiError::bad_request("invalid notification request body"))?;
    save(&s, uuid::Uuid::now_v7().to_string(), input, None).await
}
async fn update(
    State(s): State<AppState>,
    Path(id): Path<String>,
    input: Result<Json<ChannelInput>, JsonRejection>,
) -> Result<Json<ChannelView>, ApiError> {
    let Json(input) =
        input.map_err(|_| ApiError::bad_request("invalid notification request body"))?;
    let old = load(&s, &id).await?;
    if input.revision != old.revision {
        return Err(ApiError::conflict(
            "notification channel changed; reload before saving",
        ));
    }
    save(&s, id, input, Some(old)).await
}
async fn save(
    s: &AppState,
    id: String,
    input: ChannelInput,
    old: Option<Channel>,
) -> Result<Json<ChannelView>, ApiError> {
    let mut secrets = old
        .as_ref()
        .filter(|c| c.provider == input.provider)
        .map(|c| c.secrets.clone())
        .unwrap_or_default();
    if input.secrets.len() > 3
        || input
            .secrets
            .values()
            .flatten()
            .any(|v| v.is_empty() || v.len() > 2048 || v.chars().any(char::is_control))
    {
        return Err(ApiError::bad_request(
            "notification credentials must be 1..2048 bytes without control characters; use null to clear",
        ));
    }
    for (key, value) in &input.secrets {
        match value {
            Some(v) => {
                secrets.insert(key.clone(), v.clone());
            }
            None => {
                secrets.remove(key);
            }
        }
    }
    validate(&input, &secrets, false)?;
    let revision = old.as_ref().map_or(0, |c| c.revision);
    let c = Channel {
        id: id.clone(),
        name: input.name.trim().into(),
        provider: input.provider,
        enabled: input.enabled,
        config: serde_json::to_string(&input.config)
            .map_err(|_| ApiError::internal("notification serialization failed"))?,
        events: serde_json::to_string(&input.events)
            .map_err(|_| ApiError::internal("notification serialization failed"))?,
        secrets,
        revision,
    };
    if !s.store.save_notification_channel(&c, &s.vault).await? {
        return Err(ApiError::conflict(
            "notification channel changed or channel limit reached",
        ));
    }
    Ok(Json(view(&load(s, &id).await?)?))
}
async fn delete(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if !s.store.delete_notification_channel(&id).await? {
        return Err(ApiError::not_found("notification channel not found"));
    }
    Ok(Json(json!({"deleted":true})))
}
async fn test(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<super::TestResult>, ApiError> {
    // A short per-channel cooldown also serializes concurrent owner clicks across processes.
    let c = load(&s, &id).await?;
    if !s.store.claim_notification_test(&id).await? {
        return Err(ApiError::too_many_requests(
            "wait 15 seconds before testing this channel again",
        ));
    }
    Ok(Json(test_send(&s, &c).await?))
}
async fn events() -> Json<Value> {
    Json(json!(
        EVENTS
            .iter()
            .map(|(id, title, body)| json!({"id":id,"title":title,"message":body}))
            .collect::<Vec<_>>()
    ))
}
async fn templates(State(s): State<AppState>) -> Result<Json<Vec<Template>>, ApiError> {
    let mut stored = s.store.notification_templates().await?;
    for (event, _, _) in EVENTS {
        if !stored
            .iter()
            .any(|t| t.scope.is_empty() && t.event == *event)
        {
            stored.push(Template {
                scope: String::new(),
                event: (*event).into(),
                title: "{{title}}".into(),
                body: "{{message}}".into(),
            });
        }
    }
    Ok(Json(stored))
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Scope {
    #[serde(default)]
    channel_id: Option<String>,
}
async fn check_scope(s: &AppState, event: &str, scope: &Scope) -> Result<String, ApiError> {
    if !known_event(event) {
        return Err(ApiError::bad_request("unknown notification event"));
    }
    if let Some(id) = &scope.channel_id {
        load(s, id).await?;
    }
    Ok(scope.channel_id.clone().unwrap_or_default())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TemplateInput {
    title: String,
    body: String,
}
async fn save_template(
    State(s): State<AppState>,
    Path(event): Path<String>,
    Query(q): Query<Scope>,
    input: Result<Json<TemplateInput>, JsonRejection>,
) -> Result<Json<Template>, ApiError> {
    let Json(input) =
        input.map_err(|_| ApiError::bad_request("invalid notification request body"))?;
    let scope = check_scope(&s, &event, &q).await?;
    validate_template(&input.title, &input.body)?;
    let template = Template {
        scope,
        event,
        title: input.title,
        body: input.body,
    };
    s.store.save_notification_template(&template).await?;
    Ok(Json(template))
}
async fn reset_template(
    State(s): State<AppState>,
    Path(event): Path<String>,
    Query(q): Query<Scope>,
) -> Result<Json<Value>, ApiError> {
    let scope = check_scope(&s, &event, &q).await?;
    s.store.reset_notification_template(&scope, &event).await?;
    Ok(Json(json!({"reset":true})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Preview {
    event: String,
    title: String,
    body: String,
}
async fn preview(input: Result<Json<Preview>, JsonRejection>) -> Result<Json<Message>, ApiError> {
    let Json(p) = input.map_err(|_| ApiError::bad_request("invalid notification request body"))?;
    let name = EVENTS
        .iter()
        .find(|e| e.0 == p.event)
        .map(|e| e.0)
        .ok_or_else(|| ApiError::bad_request("unknown notification event"))?;
    validate_template(&p.title, &p.body)?;
    let mut e = Event::new(name, "preview");
    e.indexer = "Example indexer".into();
    e.job = "Example job".into();
    Ok(Json(message(
        &e,
        "",
        &[Template {
            scope: String::new(),
            event: p.event,
            title: p.title,
            body: p.body,
        }],
    )))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Page {
    before: Option<String>,
    limit: Option<i64>,
}
async fn deliveries(
    State(s): State<AppState>,
    Query(p): Query<Page>,
) -> Result<Json<Vec<Delivery>>, ApiError> {
    Ok(Json(
        s.store
            .notification_deliveries(p.before.as_deref(), p.limit.unwrap_or(100))
            .await?,
    ))
}
