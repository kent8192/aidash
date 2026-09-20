use super::{LocalStatus, Manifest, Status, Vote, coordinator, participant};
use crate::{Error, Result, federation::Federation};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    middleware,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct TransactionHistory {
    pub sequence: i64,
    pub transaction_id: Uuid,
    pub role: String,
    pub phase: String,
    pub detail: String,
    pub created_at: DateTime<Utc>,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct TransactionDetails {
    pub transaction: Status,
    pub participants: Vec<Vote>,
    pub history: Vec<TransactionHistory>,
}
#[derive(Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TransactionTrust {
    pub node_id: String,
    pub enabled: bool,
}

pub fn routes() -> OpenApiRouter<Federation> {
    OpenApiRouter::new()
        .routes(routes!(submit, list))
        .routes(routes!(details))
        .routes(routes!(abort))
        .routes(routes!(trust, trust_list))
        .routes(routes!(participants))
        .route_layer(middleware::from_fn(crate::api::operator_only))
}
#[utoipa::path(post,path="/transactions",operation_id="transaction_submit",request_body=Manifest,responses((status=202,body=Status)),security(("bearer_auth"=[])))]
async fn submit(
    State(f): State<Federation>,
    Json(manifest): Json<Manifest>,
) -> Result<(StatusCode, Json<Status>)> {
    Ok((
        StatusCode::ACCEPTED,
        Json(coordinator::submit(&f, &manifest).await?),
    ))
}
#[utoipa::path(get,path="/transactions",operation_id="transactions",responses((status=200,body=[Status])),security(("bearer_auth"=[])))]
async fn list(State(f): State<Federation>) -> Result<Json<Vec<Status>>> {
    Ok(Json(
        sqlx::query_as("SELECT * FROM atomic_coordinators ORDER BY created_at DESC LIMIT 200")
            .fetch_all(&f.store.control_pool)
            .await?,
    ))
}
#[utoipa::path(get,path="/transactions/{id}",operation_id="transaction_details",params(("id"=Uuid,Path)),responses((status=200,body=TransactionDetails)),security(("bearer_auth"=[])))]
async fn details(
    State(f): State<Federation>,
    Path(id): Path<Uuid>,
) -> Result<Json<TransactionDetails>> {
    let transaction = coordinator::status(&f, id).await?;
    Ok(Json(TransactionDetails {
        transaction,
        participants: coordinator::votes(&f, id).await?,
        history: sqlx::query_as(
            "SELECT * FROM atomic_history WHERE transaction_id=$1 ORDER BY sequence",
        )
        .bind(id)
        .fetch_all(&f.store.control_pool)
        .await?,
    }))
}
#[utoipa::path(post,path="/transactions/{id}/abort",operation_id="transaction_abort",params(("id"=Uuid,Path)),responses((status=200,body=Status)),security(("bearer_auth"=[])))]
async fn abort(State(f): State<Federation>, Path(id): Path<Uuid>) -> Result<Json<Status>> {
    Ok(Json(coordinator::abort(&f, id).await?))
}
#[utoipa::path(get,path="/transactions/participants",operation_id="transaction_participants",responses((status=200,body=[LocalStatus])),security(("bearer_auth"=[])))]
async fn participants(State(f): State<Federation>) -> Result<Json<Vec<LocalStatus>>> {
    Ok(Json(
        sqlx::query_as("SELECT * FROM atomic_participants ORDER BY updated_at DESC LIMIT 200")
            .fetch_all(&f.store.control_pool)
            .await?,
    ))
}
#[utoipa::path(get,path="/transactions/trust",operation_id="transaction_trust_list",responses((status=200,body=[TransactionTrust])),security(("bearer_auth"=[])))]
async fn trust_list(State(f): State<Federation>) -> Result<Json<Vec<TransactionTrust>>> {
    Ok(Json(
        sqlx::query_as("SELECT node_id,enabled FROM atomic_peer_trust ORDER BY node_id")
            .fetch_all(&f.store.control_pool)
            .await?,
    ))
}
#[utoipa::path(post,path="/transactions/trust",operation_id="transaction_trust",request_body=TransactionTrust,responses((status=200,body=TransactionTrust)),security(("bearer_auth"=[])))]
async fn trust(
    State(f): State<Federation>,
    Json(input): Json<TransactionTrust>,
) -> Result<Json<TransactionTrust>> {
    f.peer(&input.node_id).await?;
    let mut tx = f.store.control_pool.begin().await?;
    sqlx::query("INSERT INTO atomic_peer_trust(node_id,enabled) VALUES($1,$2) ON CONFLICT(node_id) DO UPDATE SET enabled=EXCLUDED.enabled,updated_at=now()").bind(&input.node_id).bind(input.enabled).execute(&mut *tx).await?;
    super::history(
        &mut tx,
        Uuid::nil(),
        "trust",
        if input.enabled { "ENABLED" } else { "DISABLED" },
        &input.node_id,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(input))
}
pub fn peer_routes() -> Router<Federation> {
    Router::new()
        .route("/transactions/reserve", post(reserve))
        .route("/transactions/prepare", post(prepare))
        .route("/transactions/finish", post(finish))
        .route("/transactions/{id}/decision", get(decision))
}
async fn reserve(
    State(f): State<Federation>,
    headers: HeaderMap,
    Json(manifest): Json<Manifest>,
) -> Result<Json<LocalStatus>> {
    Ok(Json(
        participant::reserve(&f, crate::api::peer_node(&headers)?, &manifest).await?,
    ))
}
async fn prepare(
    State(f): State<Federation>,
    headers: HeaderMap,
    Json(manifest): Json<Manifest>,
) -> Result<Json<LocalStatus>> {
    Ok(Json(
        participant::prepare(&f, crate::api::peer_node(&headers)?, &manifest).await?,
    ))
}
async fn finish(
    State(f): State<Federation>,
    headers: HeaderMap,
    Json(manifest): Json<Manifest>,
) -> Result<Json<LocalStatus>> {
    Ok(Json(
        participant::finish(&f, crate::api::peer_node(&headers)?, &manifest).await?,
    ))
}
async fn decision(
    State(f): State<Federation>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Status>> {
    let state = coordinator::status(&f, id).await?;
    let manifest: Manifest = serde_json::from_value(state.manifest.clone())?;
    if !manifest
        .participants
        .iter()
        .any(|p| Some(p.node_id.as_str()) == crate::api::peer_node(&headers).ok())
    {
        return Err(Error::Forbidden);
    }
    Ok(Json(state))
}
