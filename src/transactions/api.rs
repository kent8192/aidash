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
        sqlx::query_as(
            &sea_orm::sea_query::Query::select()
                .expr(sea_orm::sea_query::SimpleExpr::from(
                    sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
                ))
                .from(sea_orm::sea_query::Alias::new("atomic_coordinators"))
                .order_by_expr(
                    sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
                        sea_orm::sea_query::Alias::new("created_at"),
                    )),
                    sea_orm::sea_query::Order::Desc,
                )
                .limit(200)
                .to_string(sea_orm::sea_query::PostgresQueryBuilder),
        )
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
            &sea_orm::sea_query::Query::select()
                .expr(sea_orm::sea_query::SimpleExpr::from(
                    sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
                ))
                .from(sea_orm::sea_query::Alias::new("atomic_history"))
                .and_where(sea_orm::sea_query::Expr::cust("transaction_id = $1"))
                .order_by_expr(
                    sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
                        sea_orm::sea_query::Alias::new("sequence"),
                    )),
                    sea_orm::sea_query::Order::Asc,
                )
                .to_string(sea_orm::sea_query::PostgresQueryBuilder),
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
        sqlx::query_as(
            &sea_orm::sea_query::Query::select()
                .expr(sea_orm::sea_query::SimpleExpr::from(
                    sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
                ))
                .from(sea_orm::sea_query::Alias::new("atomic_participants"))
                .order_by_expr(
                    sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
                        sea_orm::sea_query::Alias::new("updated_at"),
                    )),
                    sea_orm::sea_query::Order::Desc,
                )
                .limit(200)
                .to_string(sea_orm::sea_query::PostgresQueryBuilder),
        )
        .fetch_all(&f.store.control_pool)
        .await?,
    ))
}
#[utoipa::path(get,path="/transactions/trust",operation_id="transaction_trust_list",responses((status=200,body=[TransactionTrust])),security(("bearer_auth"=[])))]
async fn trust_list(State(f): State<Federation>) -> Result<Json<Vec<TransactionTrust>>> {
    Ok(Json(
        sqlx::query_as(
            &sea_orm::sea_query::Query::select()
                .expr(sea_orm::sea_query::SimpleExpr::from(
                    sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("node_id")),
                ))
                .expr(sea_orm::sea_query::SimpleExpr::from(
                    sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("enabled")),
                ))
                .from(sea_orm::sea_query::Alias::new("atomic_peer_trust"))
                .order_by_expr(
                    sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
                        sea_orm::sea_query::Alias::new("node_id"),
                    )),
                    sea_orm::sea_query::Order::Asc,
                )
                .to_string(sea_orm::sea_query::PostgresQueryBuilder),
        )
        .fetch_all(&f.store.control_pool)
        .await?,
    ))
}
#[utoipa::path(post,path="/transactions/trust",operation_id="transaction_trust",request_body=TransactionTrust,responses((status=200,body=TransactionTrust)),security(("bearer_auth"=[])))]
async fn trust(
    State(f): State<Federation>,
    Json(input): Json<TransactionTrust>,
) -> Result<Json<TransactionTrust>> {
    if input.enabled {
        f.peer(&input.node_id).await?;
    }
    let mut tx = f.store.control_pool.begin().await?;
    sqlx::query(
        &sea_orm::sea_query::Query::insert()
            .into_table(sea_orm::sea_query::Alias::new("atomic_peer_trust"))
            .columns([
                sea_orm::sea_query::Alias::new("node_id"),
                sea_orm::sea_query::Alias::new("enabled"),
            ])
            .values_panic([
                sea_orm::sea_query::Expr::cust("$1"),
                sea_orm::sea_query::Expr::cust("$2"),
            ])
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([sea_orm::sea_query::Alias::new(
                    "node_id",
                )])
                .value(
                    sea_orm::sea_query::Alias::new("enabled"),
                    sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
                        sea_orm::sea_query::Alias::new("excluded"),
                        sea_orm::sea_query::Alias::new("enabled"),
                    ))),
                )
                .value(
                    sea_orm::sea_query::Alias::new("updated_at"),
                    sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP"),
                )
                .to_owned(),
            )
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(&input.node_id)
    .bind(input.enabled)
    .execute(&mut *tx)
    .await?;
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
