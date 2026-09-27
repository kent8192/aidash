use super::*;
use crate::{authorization::identity::Actor, federation::Federation};
use axum::{
	Extension, Json,
	extract::{Path, State},
	middleware,
};
use utoipa_axum::{router::OpenApiRouter, routes};

// Keep the process-wide capacity across router construction, clones and nodes.
static SEARCH_ADMISSION: std::sync::LazyLock<tower::limit::GlobalConcurrencyLimitLayer> =
	std::sync::LazyLock::new(|| tower::limit::GlobalConcurrencyLimitLayer::new(2));

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
		.merge(
			OpenApiRouter::new()
				.routes(routes!(search))
				.route_layer(SEARCH_ADMISSION.clone()),
		)
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

#[cfg(test)]
mod admission_tests {
	use super::SEARCH_ADMISSION;
	use axum::{Router, body::Body, http::Request, routing::get};
	use std::{sync::Arc, time::Duration};
	use tower::ServiceExt;

	#[tokio::test]
	async fn search_routes_share_two_slots_wait_and_release_on_cancellation() {
		let started = Arc::new(tokio::sync::Semaphore::new(0));
		let make_router = || {
			let started = started.clone();
			Router::new()
				.route(
					"/search",
					get(move || {
						let started = started.clone();
						async move {
							started.add_permits(1);
							std::future::pending::<()>().await;
							"never"
						}
					}),
				)
				.route_layer(SEARCH_ADMISSION.clone())
		};
		let request = || {
			Request::builder()
				.uri("/search")
				.body(Body::empty())
				.unwrap()
		};
		let first = tokio::spawn(make_router().oneshot(request()));
		let second = tokio::spawn(make_router().oneshot(request()));
		started.acquire_many(2).await.unwrap().forget();
		let independent = Router::new()
			.route("/search", get(|| async { "ready" }))
			.route_layer(SEARCH_ADMISSION.clone())
			.route("/other", get(|| async { "unlimited" }));
		assert_eq!(
			independent
				.clone()
				.oneshot(
					Request::builder()
						.uri("/other")
						.body(Body::empty())
						.unwrap()
				)
				.await
				.unwrap()
				.status(),
			200
		);
		let waiting = independent.oneshot(request());
		tokio::pin!(waiting);
		assert!(
			tokio::time::timeout(Duration::from_millis(30), &mut waiting)
				.await
				.is_err()
		);
		first.abort();
		assert!(first.await.unwrap_err().is_cancelled());
		assert_eq!(
			tokio::time::timeout(Duration::from_secs(1), waiting)
				.await
				.unwrap()
				.unwrap()
				.status(),
			200
		);
		second.abort();
		assert!(second.await.unwrap_err().is_cancelled());
	}
}
