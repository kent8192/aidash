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
    static SEARCH_CAPACITY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let _permit = SEARCH_CAPACITY
        .acquire()
        .await
        .expect("search capacity stays open");
    let worker = f.for_workers().await?;
    let result = service::search(&worker.store, &actor, workspace, input).await;
    worker.store.pool.close().await;
    worker.store.control_pool.close().await;
    Ok(Json(result?))
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
    let points: CleanupCounts = sqlx::query_as(
        &sea_orm::sea_query::Query::select()
            .expr_as(
                sea_orm::sea_query::Expr::cust("COUNT(*)"),
                sea_orm::sea_query::Alias::new("retired"),
            )
            .expr_as(
                sea_orm::sea_query::Expr::cust("COUNT(*) FILTER(WHERE p.cleaned_at IS NULL)"),
                sea_orm::sea_query::Alias::new("pending"),
            )
            .expr_as(
                sea_orm::sea_query::Expr::cust("COUNT(*) FILTER(WHERE p.last_error IS NOT NULL)"),
                sea_orm::sea_query::Alias::new("failed"),
            )
            .from_as(
                sea_orm::sea_query::Alias::new("semantic_points"),
                sea_orm::sea_query::Alias::new("p"),
            )
            .join_as(
                sea_orm::sea_query::JoinType::InnerJoin,
                sea_orm::sea_query::Alias::new("semantic_collections"),
                sea_orm::sea_query::Alias::new("c"),
                sea_orm::sea_query::Expr::cust("c.collection = p.collection"),
            )
            .and_where(sea_orm::sea_query::Expr::cust(
                "c.workspace_id = $1 AND p.retired AND NOT c.retired",
            ))
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(workspace)
    .fetch_one(&f.store.pool)
    .await?;
    let collections: CleanupCounts = sqlx::query_as(
        &sea_orm::sea_query::Query::select()
            .expr_as(
                sea_orm::sea_query::Expr::cust("COUNT(*)"),
                sea_orm::sea_query::Alias::new("retired"),
            )
            .expr_as(
                sea_orm::sea_query::Expr::cust("COUNT(*) FILTER(WHERE cleaned_at IS NULL)"),
                sea_orm::sea_query::Alias::new("pending"),
            )
            .expr_as(
                sea_orm::sea_query::Expr::cust("COUNT(*) FILTER(WHERE last_error IS NOT NULL)"),
                sea_orm::sea_query::Alias::new("failed"),
            )
            .from(sea_orm::sea_query::Alias::new("semantic_collections"))
            .and_where(sea_orm::sea_query::Expr::cust(
                "workspace_id = $1 AND retired",
            ))
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(workspace)
    .fetch_one(&f.store.pool)
    .await?;
    Ok(Json(CleanupStatus {
        points,
        collections,
    }))
}
