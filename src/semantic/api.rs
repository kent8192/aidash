use super::*;
use crate::{authorization::identity::Actor, federation::Federation};
use axum::{
    Extension, Json,
    extract::{Path, State},
    middleware,
};
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn routes() -> OpenApiRouter<Federation> {
    OpenApiRouter::new()
        .merge(
            OpenApiRouter::new()
                .routes(routes!(configure))
                .routes(routes!(cleanup))
                .route_layer(middleware::from_fn(crate::api::operator_only)),
        )
        .routes(routes!(index))
        .routes(routes!(put, entries))
        .routes(routes!(delete))
        .routes(routes!(reindex))
        .routes(routes!(search))
        .routes(routes!(history))
}
#[utoipa::path(post,path="/workspaces/{workspace}/semantic/index",operation_id="semantic_configure",params(("workspace"=Uuid,Path)),request_body=ConfigureIndex,responses((status=200,body=Index)),security(("bearer_auth"=[])))]
async fn configure(
    State(f): State<Federation>,
    Path(workspace): Path<Uuid>,
    Json(input): Json<ConfigureIndex>,
) -> Result<Json<Index>> {
    Ok(Json(service::configure(&f.store, workspace, input).await?))
}
#[utoipa::path(get,path="/workspaces/{workspace}/semantic/index",operation_id="semantic_index",params(("workspace"=Uuid,Path)),responses((status=200,body=Index)),security(("bearer_auth"=[])))]
async fn index(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(workspace): Path<Uuid>,
) -> Result<Json<Index>> {
    Ok(Json(service::get_index(&f.store, &actor, workspace).await?))
}
#[utoipa::path(post,path="/workspaces/{workspace}/semantic/entries",operation_id="semantic_put",params(("workspace"=Uuid,Path)),request_body=PutEntry,responses((status=200,body=Entry)),security(("bearer_auth"=[])))]
async fn put(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(workspace): Path<Uuid>,
    Json(input): Json<PutEntry>,
) -> Result<Json<Entry>> {
    Ok(Json(
        service::put(&f.store, &actor, workspace, input).await?,
    ))
}
#[utoipa::path(get,path="/workspaces/{workspace}/semantic/entries",operation_id="semantic_entries",params(("workspace"=Uuid,Path)),responses((status=200,body=[Entry])),security(("bearer_auth"=[])))]
async fn entries(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(workspace): Path<Uuid>,
) -> Result<Json<Vec<Entry>>> {
    Ok(Json(service::entries(&f.store, &actor, workspace).await?))
}
#[utoipa::path(delete,path="/workspaces/{workspace}/semantic/entries/{id}",operation_id="semantic_delete",params(("workspace"=Uuid,Path),("id"=Uuid,Path)),request_body=Revision,responses((status=200,body=Entry)),security(("bearer_auth"=[])))]
async fn delete(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path((workspace, id)): Path<(Uuid, Uuid)>,
    Json(input): Json<Revision>,
) -> Result<Json<Entry>> {
    Ok(Json(
        service::change(
            &f.store,
            &actor,
            workspace,
            id,
            input.expected_revision,
            true,
        )
        .await?,
    ))
}
#[utoipa::path(post,path="/workspaces/{workspace}/semantic/entries/{id}/reindex",operation_id="semantic_reindex",params(("workspace"=Uuid,Path),("id"=Uuid,Path)),request_body=Revision,responses((status=200,body=Entry)),security(("bearer_auth"=[])))]
async fn reindex(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path((workspace, id)): Path<(Uuid, Uuid)>,
    Json(input): Json<Revision>,
) -> Result<Json<Entry>> {
    Ok(Json(
        service::change(
            &f.store,
            &actor,
            workspace,
            id,
            input.expected_revision,
            false,
        )
        .await?,
    ))
}
#[utoipa::path(post,path="/workspaces/{workspace}/semantic/search",operation_id="semantic_search",params(("workspace"=Uuid,Path)),request_body=Search,responses((status=200,body=SearchResult)),security(("bearer_auth"=[])))]
async fn search(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(workspace): Path<Uuid>,
    Json(input): Json<Search>,
) -> Result<Json<SearchResult>> {
    // Reserve effect/audit capacity independently from API revokers waiting on
    // this search's credential and policy lease.
    let worker = f.for_workers().await?;
    Ok(Json(
        service::search(&worker.store, &actor, workspace, input).await?,
    ))
}
#[utoipa::path(get,path="/workspaces/{workspace}/semantic/history",operation_id="semantic_history",params(("workspace"=Uuid,Path)),responses((status=200,body=[History])),security(("bearer_auth"=[])))]
async fn history(
    State(f): State<Federation>,
    Extension(actor): Extension<Actor>,
    Path(workspace): Path<Uuid>,
) -> Result<Json<Vec<History>>> {
    Ok(Json(
        service::history_list(&f.store, &actor, workspace).await?,
    ))
}

#[utoipa::path(get,path="/workspaces/{workspace}/semantic/cleanup",operation_id="semantic_cleanup",params(("workspace"=Uuid,Path)),responses((status=200,body=CleanupStatus)),security(("bearer_auth"=[])))]
async fn cleanup(
    State(f): State<Federation>,
    Path(workspace): Path<Uuid>,
) -> Result<Json<CleanupStatus>> {
    let points:CleanupCounts=sqlx::query_as("SELECT count(*) AS retired,count(*) FILTER (WHERE p.cleaned_at IS NULL) AS pending,count(*) FILTER (WHERE p.last_error IS NOT NULL) AS failed FROM semantic_points p JOIN semantic_collections c ON c.collection=p.collection WHERE c.workspace_id=$1 AND p.retired AND NOT c.retired").bind(workspace).fetch_one(&f.store.pool).await?;
    let collections:CleanupCounts=sqlx::query_as("SELECT count(*) AS retired,count(*) FILTER (WHERE cleaned_at IS NULL) AS pending,count(*) FILTER (WHERE last_error IS NOT NULL) AS failed FROM semantic_collections WHERE workspace_id=$1 AND retired").bind(workspace).fetch_one(&f.store.pool).await?;
    Ok(Json(CleanupStatus {
        points,
        collections,
    }))
}
