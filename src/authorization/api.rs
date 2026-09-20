use super::{
    Authorization, Snapshot,
    policy::{Decision, Evaluation, PolicyBundle},
};
use crate::{Result, federation::Federation};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::Value;
use utoipa_axum::{router::OpenApiRouter, routes};

/// Mounted inside the management router, behind the existing operator token.
/// Evaluation bodies are simulations of authority, never execution credentials.
pub fn routes() -> OpenApiRouter<Federation> {
    OpenApiRouter::new()
        .routes(routes!(snapshot))
        .routes(routes!(replace))
        .routes(routes!(evaluate))
        .routes(routes!(simulate))
        .routes(routes!(revisions))
        .routes(routes!(decisions))
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct AuthorizationUpdate {
    expected_revision: i64,
    bundle: PolicyBundle,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
struct AuthorizationPage {
    #[serde(default)]
    after: i64,
    #[serde(default = "page_size")]
    limit: i64,
}
fn page_size() -> i64 {
    100
}
fn service(f: Federation) -> Authorization {
    Authorization { pool: f.store.pool }
}

#[utoipa::path(get, path = "/authorization/{tenant}", operation_id = "authorization_snapshot", params(("tenant" = String, Path)), responses((status = 200, body = Snapshot)), security(("bearer_auth" = [])))]
async fn snapshot(
    State(f): State<Federation>,
    Path(tenant): Path<String>,
) -> Result<Json<Snapshot>> {
    Ok(Json(service(f).snapshot(&tenant).await?))
}
#[utoipa::path(post, path = "/authorization/{tenant}", operation_id = "authorization_replace", params(("tenant" = String, Path)), request_body = AuthorizationUpdate, responses((status = 200, body = Snapshot)), security(("bearer_auth" = [])))]
async fn replace(
    State(f): State<Federation>,
    Path(tenant): Path<String>,
    Json(input): Json<AuthorizationUpdate>,
) -> Result<Json<Snapshot>> {
    Ok(Json(
        service(f)
            .replace(&tenant, input.expected_revision, input.bundle, "operator")
            .await?,
    ))
}
#[utoipa::path(post, path = "/authorization/{tenant}/evaluate", operation_id = "authorization_evaluate", params(("tenant" = String, Path)), request_body = Evaluation, responses((status = 200, body = Decision)), security(("bearer_auth" = [])))]
async fn evaluate(
    State(f): State<Federation>,
    Path(tenant): Path<String>,
    Json(input): Json<Evaluation>,
) -> Result<Json<Decision>> {
    Ok(Json(service(f).evaluate(&tenant, &input).await?))
}
#[utoipa::path(post, path = "/authorization/{tenant}/simulate", operation_id = "authorization_simulate", params(("tenant" = String, Path)), request_body = Evaluation, responses((status = 200, body = Decision)), security(("bearer_auth" = [])))]
async fn simulate(
    State(f): State<Federation>,
    Path(tenant): Path<String>,
    Json(input): Json<Evaluation>,
) -> Result<Json<Decision>> {
    Ok(Json(service(f).simulate(&tenant, &input).await?))
}
#[utoipa::path(get, path = "/authorization/{tenant}/revisions", operation_id = "authorization_revisions", params(("tenant" = String, Path), AuthorizationPage), responses((status = 200, body = [Value])), security(("bearer_auth" = [])))]
async fn revisions(
    State(f): State<Federation>,
    Path(tenant): Path<String>,
    Query(page): Query<AuthorizationPage>,
) -> Result<Json<Vec<Value>>> {
    Ok(Json(
        service(f)
            .revisions(&tenant, page.after, page.limit)
            .await?,
    ))
}
#[utoipa::path(get, path = "/authorization/{tenant}/decisions", operation_id = "authorization_decisions", params(("tenant" = String, Path), AuthorizationPage), responses((status = 200, body = [Value])), security(("bearer_auth" = [])))]
async fn decisions(
    State(f): State<Federation>,
    Path(tenant): Path<String>,
    Query(page): Query<AuthorizationPage>,
) -> Result<Json<Vec<Value>>> {
    Ok(Json(
        service(f)
            .decisions(&tenant, page.after, page.limit)
            .await?,
    ))
}
