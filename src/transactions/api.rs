use super::{LocalStatus, Manifest, Status, Vote, authority, coordinator, participant};
use crate::authorization::identity::Actor;
use crate::{Error, Result, federation::Federation};
use axum::{
	Extension, Json, Router,
	extract::{Path, State},
	http::{HeaderMap, StatusCode},
	middleware,
	routing::{get, post},
};
use chrono::{DateTime, Utc};
use futures_util::{StreamExt, stream};
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
		.merge(
			OpenApiRouter::new()
				.routes(routes!(trust, trust_list))
				.routes(routes!(participants))
				.routes(routes!(restore_peer))
				.route_layer(middleware::from_fn(crate::api::operator_only)),
		)
}
#[utoipa::path(post,path="/transactions",operation_id="transaction_submit",request_body=Manifest,responses((status=202,body=Status)),security(("bearer_auth"=[])))]
async fn submit(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Json(manifest): Json<Manifest>,
) -> Result<(StatusCode, Json<Status>)> {
	Ok((
		StatusCode::ACCEPTED,
		Json(match actor {
			Actor::Operator => coordinator::submit(&f, &manifest).await?,
			Actor::Subject(identity) => authority::submit(&f, &identity, &manifest).await?,
		}),
	))
}
#[utoipa::path(get,path="/transactions",operation_id="transactions",responses((status=200,body=[Status])),security(("bearer_auth"=[])))]
async fn list(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
) -> Result<Json<Vec<Status>>> {
	use sea_orm::sea_query::{Alias, Asterisk, Expr, Order, PostgresQueryBuilder, Query};
	let mut select = Query::select();
	select
		.column((Alias::new("c"), Asterisk))
		.from_as(Alias::new("atomic_coordinators"), Alias::new("c"))
		.order_by((Alias::new("c"), Alias::new("created_at")), Order::Desc)
		.limit(200);
	if matches!(actor, Actor::Subject(_)) {
		select.and_where(Expr::exists(
			Query::select()
				.expr(Expr::val(1))
				.from_as(Alias::new("atomic_subjects"), Alias::new("s"))
				.and_where(Expr::cust(
					"s.id=c.id AND s.binding->>'tenant'=$1 AND s.binding->>'subject'=$2",
				))
				.to_owned(),
		));
	}
	// Scan bounded pages in a stable order until the visible page is full.
	select.order_by((Alias::new("c"), Alias::new("id")), Order::Desc);
	let mut visible = Vec::new();
	let mut cursor: Option<(DateTime<Utc>, Uuid)> = None;
	loop {
		let mut page = select.clone();
		if cursor.is_some() {
			page.and_where(Expr::cust(if matches!(actor, Actor::Subject(_)) {
				"(c.created_at < $3 OR (c.created_at = $3 AND c.id < $4))"
			} else {
				"(c.created_at < $1 OR (c.created_at = $1 AND c.id < $2))"
			}));
		}
		let sql = page.to_string(PostgresQueryBuilder);
		let mut query = sqlx::query_as(&sql);
		if let Actor::Subject(identity) = &actor {
			query = query.bind(&identity.tenant).bind(&identity.subject);
		}
		if let Some((created, id)) = cursor {
			query = query.bind(created).bind(id);
		}
		let rows: Vec<Status> = query.fetch_all(&f.store.control_pool).await?;
		let exhausted = rows.len() < 200;
		// Bound concurrent live checks (and their retained authority transactions),
		// while yielding in database order so visibility filtering preserves the cursor.
		let mut checked = stream::iter(rows.into_iter().map(|row| {
			let f = &f;
			let actor = &actor;
			async move {
				let result = if let Actor::Subject(identity) = &actor {
					authority::manage(f, identity, &row, "transaction.read").await
				} else {
					Ok(())
				};
				(row, result)
			}
		}))
		.buffered(8);
		while let Some((row, result)) = checked.next().await {
			cursor = Some((row.created_at, row.id));
			match result {
				Ok(()) => {}
				Err(Error::Forbidden | Error::Unauthorized | Error::NotFound(_)) => continue,
				Err(error) => return Err(error),
			}
			visible.push(row);
			if visible.len() == 200 {
				break;
			}
		}
		if exhausted || visible.len() == 200 {
			break;
		}
	}

	Ok(Json(visible))
}
#[utoipa::path(get,path="/transactions/{id}",operation_id="transaction_details",params(("id"=Uuid,Path)),responses((status=200,body=TransactionDetails)),security(("bearer_auth"=[])))]
async fn details(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<TransactionDetails>> {
	if let Actor::Subject(identity) = &actor {
		authority::require_owner(&f, identity, id).await?;
	}
	let transaction = coordinator::status(&f, id).await?;
	if let Actor::Subject(identity) = actor {
		authority::manage(&f, &identity, &transaction, "transaction.read").await?;
	}
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
async fn abort(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<Status>> {
	if let Actor::Subject(identity) = actor {
		authority::require_owner(&f, &identity, id).await?;
		let state = coordinator::status(&f, id).await?;
		authority::manage(&f, &identity, &state, "transaction.abort").await?;
		return Ok(Json(coordinator::status(&f, id).await?));
	}
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
#[utoipa::path(get,path="/transactions/trust",operation_id="transaction_trust_list",responses((status=200,body=[TrustChange])),security(("bearer_auth"=[])))]
async fn trust_list(State(f): State<Federation>) -> Result<Json<Vec<TrustChange>>> {
	let rows: Vec<TransactionTrust> = sqlx::query_as(
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
	.await?;
	let mut result = Vec::with_capacity(rows.len());
	for trust in rows {
		let pending_transactions = if trust.enabled {
			Vec::new()
		} else {
			authority::pending_peer(&f, &trust.node_id).await?
		};
		result.push(TrustChange {
			trust,
			pending_transactions,
		});
	}
	Ok(Json(result))
}
#[derive(Serialize, utoipa::ToSchema)]
struct TrustChange {
	#[serde(flatten)]
	trust: TransactionTrust,
	pending_transactions: Vec<Uuid>,
}
#[utoipa::path(post,path="/transactions/trust",operation_id="transaction_trust",request_body=TransactionTrust,responses((status=200,body=TrustChange),(status=202,body=TrustChange)),security(("bearer_auth"=[])))]
async fn trust(
	State(f): State<Federation>,
	Json(input): Json<TransactionTrust>,
) -> Result<(StatusCode, Json<TrustChange>)> {
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
	let pending_transactions = if input.enabled {
		Vec::new()
	} else {
		authority::pending_peer(&f, &input.node_id).await?
	};
	let status = if pending_transactions.is_empty() {
		StatusCode::OK
	} else {
		StatusCode::ACCEPTED
	};
	Ok((
		status,
		Json(TrustChange {
			trust: input,
			pending_transactions,
		}),
	))
}
pub fn peer_routes() -> Router<Federation> {
	Router::new()
		.route("/transactions/preflight", post(preflight))
		.route("/transactions/access", post(read_access))
		.route("/transactions/{id}/authority", get(authority_ticket))
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

async fn preflight(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<authority::Preflight>,
) -> Result<Json<serde_json::Value>> {
	authority::preflight(&f, crate::api::peer_node(&headers)?, &input).await?;
	Ok(Json(serde_json::json!({"authorized":true})))
}
async fn read_access(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<authority::Preflight>,
) -> Result<Json<serde_json::Value>> {
	authority::read_access(&f, crate::api::peer_node(&headers)?, &input).await?;
	Ok(Json(serde_json::json!({"authorized":true})))
}
async fn authority_ticket(
	State(f): State<Federation>,
	headers: HeaderMap,
	Path(id): Path<Uuid>,
) -> Result<Json<authority::Preflight>> {
	Ok(Json(
		authority::ticket(&f, id, crate::api::peer_node(&headers)?).await?,
	))
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct PeerRecovery {
	node_id: String,
	credential_env: String,
}
#[utoipa::path(post,path="/transactions/peer-recovery",operation_id="transaction_peer_recovery",request_body=PeerRecovery,responses((status=200,body=crate::federation::Peer)),security(("bearer_auth"=[])))]
async fn restore_peer(
	State(f): State<Federation>,
	Json(input): Json<PeerRecovery>,
) -> Result<Json<crate::federation::Peer>> {
	let credential = crate::config::peer_secret(&input.credential_env)?;
	let mut tx = f.store.control_pool.begin().await?;
	authority::control(&mut tx).await?;
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	sqlx::query(
		&Query::select()
			.expr(Expr::cust("pg_advisory_xact_lock(71003203)"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	let others: Vec<String> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("credential_env"))
			.from(Alias::new("peers"))
			.and_where(Expr::cust("enabled AND node_id<>$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&input.node_id)
	.fetch_all(&mut *tx)
	.await?;
	for other in others {
		if crate::config::peer_secret(&other)? == credential {
			return Err(Error::Invalid(
				"enabled peers must use distinct credentials for each node identity".into(),
			));
		}
	}

	// Existing endpoint and Node identity are immutable here. This restores
	// authenticated communication without restoring subject mapping or trust.
	let peer = sqlx::query_as(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("peers"))
			.value(
				sea_orm::sea_query::Alias::new("credential_env"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.value(sea_orm::sea_query::Alias::new("enabled"), true)
			.and_where(sea_orm::sea_query::Expr::cust("node_id=$1"))
			.returning_all()
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(&input.node_id)
	.bind(input.credential_env)
	.fetch_optional(&mut *tx)
	.await?
	.ok_or_else(|| Error::NotFound("existing peer".into()))?;
	super::history(
		&mut tx,
		Uuid::nil(),
		"trust",
		"AUTHENTICATION_RESTORED",
		&input.node_id,
	)
	.await?;
	tx.commit().await?;
	Ok(Json(peer))
}
