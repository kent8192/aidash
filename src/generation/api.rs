use super::{
	Assignment, Request,
	policy::{self, Policy, Spec},
};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	federation::Federation,
};
use axum::{
	Extension, Json,
	extract::{Path, State},
};
use serde::Deserialize;

use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(set_policy))
		.routes(routes!(policies))
		.routes(routes!(assign))
		.routes(routes!(requests))
		.routes(routes!(control))
		.routes(routes!(history))
		.routes(routes!(usage))
		.routes(routes!(spec))
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = GenerationPolicyUpdate)]
pub struct PolicyUpdate {
	expected_revision: i64,
	spec: Spec,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = GenerationAssignInput)]
pub struct AssignInput {
	policy_id: String,
	reason: String,
}

#[utoipa::path(post,path="/generation/{tenant}/policies/{id}",operation_id="generation_set_policy",request_body=PolicyUpdate,params(("tenant"=String,Path),("id"=String,Path)),responses((status=200,body=Policy)),security(("bearer_auth"=[])))]
async fn set_policy(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((tenant, id)): Path<(String, String)>,
	Json(input): Json<PolicyUpdate>,
) -> Result<Json<Policy>> {
	match actor {
		Actor::Operator => {
			let mut tx = f.store.pool.begin().await?;
			let policy = policy::write(
				&mut tx,
				&tenant,
				&id,
				input.expected_revision,
				&input.spec,
				"operator",
			)
			.await?;
			tx.commit().await?;
			Ok(Json(policy))
		}
		Actor::Subject(identity) => {
			if tenant != identity.tenant {
				return Err(Error::Forbidden);
			}
			let mut access = Access::begin_exclusive(&f.store, &identity).await?;
			let result = async {
				access
					.require(&super::resource(&access, &id), "generation.manage")
					.await?;
				policy::write(
					&mut access.tx,
					&tenant,
					&id,
					input.expected_revision,
					&input.spec,
					&identity.subject,
				)
				.await
			}
			.await;
			Ok(Json(access.finish(result).await?))
		}
	}
}
#[utoipa::path(get,path="/generation/{tenant}/policies",operation_id="generation_policies",params(("tenant"=String,Path)),responses((status=200,body=[Policy])),security(("bearer_auth"=[])))]
async fn policies(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(tenant): Path<String>,
) -> Result<Json<Vec<Policy>>> {
	let ids: Vec<String> = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id")),
			))
			.from(sea_orm::sea_query::Alias::new("generation_policies"))
			.and_where(sea_orm::sea_query::Expr::cust("tenant = $1"))
			.order_by_expr(
				sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
					sea_orm::sea_query::Alias::new("id"),
				)),
				sea_orm::sea_query::Order::Asc,
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(&tenant)
	.fetch_all(&f.store.pool)
	.await?;
	match actor {
		Actor::Operator => {
			let mut tx = f.store.pool.begin().await?;
			let mut result = vec![];
			for id in ids {
				result.push(policy::load(&mut tx, &tenant, &id, false).await?);
			}
			tx.commit().await?;
			Ok(Json(result))
		}
		Actor::Subject(identity) => {
			if tenant != identity.tenant {
				return Err(Error::Forbidden);
			}
			let mut access = Access::begin(&f.store, &identity).await?;
			let result = async {
				let mut result = vec![];
				for id in ids {
					if access
						.decide(&super::resource(&access, &id), "generation.read")
						.await?
					{
						result.push(policy::load(&mut access.tx, &tenant, &id, false).await?);
					}
				}
				Ok(result)
			}
			.await;
			Ok(Json(access.finish(result).await?))
		}
	}
}
#[utoipa::path(post,path="/generation/{tenant}/tasks/{id}/assign",operation_id="generation_assign",params(("tenant"=String,Path),("id"=Uuid,Path)),request_body=AssignInput,responses((status=200,body=Assignment)),security(("bearer_auth"=[])))]
async fn assign(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((tenant, id)): Path<(String, Uuid)>,
	Json(input): Json<AssignInput>,
) -> Result<Json<Assignment>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	if tenant != identity.tenant {
		return Err(Error::Forbidden);
	}
	Ok(Json(
		super::assign(&f, &identity, id, &input.policy_id, &input.reason).await?,
	))
}
#[utoipa::path(get,path="/generation/{tenant}/requests",operation_id="generation_requests",params(("tenant"=String,Path)),responses((status=200,body=[Request])),security(("bearer_auth"=[])))]
async fn requests(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(tenant): Path<String>,
) -> Result<Json<Vec<Request>>> {
	let Actor::Subject(identity) = actor else {
		return Ok(Json(
			sqlx::query_as(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
					))
					.from(sea_orm::sea_query::Alias::new("generation_requests"))
					.and_where(sea_orm::sea_query::Expr::cust("tenant = $1"))
					.order_by_expr(
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
							sea_orm::sea_query::Alias::new("created_at"),
						)),
						sea_orm::sea_query::Order::Desc,
					)
					.order_by_expr(
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
							sea_orm::sea_query::Alias::new("id"),
						)),
						sea_orm::sea_query::Order::Asc,
					)
					.limit(200)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(tenant)
			.fetch_all(&f.store.pool)
			.await?,
		));
	};
	if tenant != identity.tenant {
		return Err(Error::Forbidden);
	}
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let mut visible = vec![];
		let mut offset = 0_i64;
		loop {
			let requests: Vec<Request> = sqlx::query_as(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
					))
					.from(sea_orm::sea_query::Alias::new("generation_requests"))
					.and_where(sea_orm::sea_query::Expr::cust("tenant = $1"))
					.order_by_expr(
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
							sea_orm::sea_query::Alias::new("created_at"),
						)),
						sea_orm::sea_query::Order::Desc,
					)
					.order_by_expr(
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
							sea_orm::sea_query::Alias::new("id"),
						)),
						sea_orm::sea_query::Order::Asc,
					)
					.limit(200)
					.offset(offset as u64)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(&tenant)
			.fetch_all(&mut *access.tx)
			.await?;
			let exhausted = requests.len() < 200;
			for request in requests {
				if request.visible(&mut access).await? {
					visible.push(request);
				}
				if visible.len() == 200 {
					break;
				}
			}
			if exhausted || visible.len() == 200 {
				break;
			}
			offset += 200;
		}
		Ok(visible)
	}
	.await;
	Ok(Json(access.finish(result).await?))
}

#[utoipa::path(post,path="/generation/{tenant}/requests/{id}/control",operation_id="generation_control",params(("tenant"=String,Path),("id"=Uuid,Path)),request_body=super::lifecycle::Control,responses((status=200,body=Request)),security(("bearer_auth"=[])))]
async fn control(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((tenant, id)): Path<(String, Uuid)>,
	Json(input): Json<super::lifecycle::Control>,
) -> Result<Json<Request>> {
	use super::lifecycle::{self, Action};
	let result = match actor {
		Actor::Operator => {
			let mut tx = f.store.pool.begin().await?;
			crate::authorization::Authorization::load_with_mode(&mut tx, &tenant, true).await?;
			let job = lifecycle::load(&mut tx, &tenant, id).await?;
			let job = lifecycle::control(&f, &mut tx, &job, &input, "operator").await?;
			tx.commit().await?;
			job
		}
		Actor::Subject(identity) => {
			if tenant != identity.tenant {
				return Err(Error::Forbidden);
			}
			let mut access = Access::begin_exclusive(&f.store, &identity).await?;
			let result = async {
				let job = lifecycle::load(&mut access.tx, &tenant, id).await?;
				if !job.visible(&mut access).await? {
					return Err(Error::Forbidden);
				}
				let action = match input.action {
					Action::Approve | Action::Deny => "generation.approve",
					Action::Stop => "generation.stop",
					Action::Delete => "generation.delete",
				};
				access.require(&job.resource(&access), action).await?;
				lifecycle::control(&f, &mut access.tx, &job, &input, &identity.subject).await
			}
			.await;
			access.finish(result).await?
		}
	};
	f.notify.notify_waiters();
	Ok(Json(result))
}
#[utoipa::path(get,path="/generation/{tenant}/requests/{id}/history",operation_id="generation_history",params(("tenant"=String,Path),("id"=Uuid,Path)),responses((status=200,body=[super::lifecycle::History])),security(("bearer_auth"=[])))]
async fn history(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((tenant, id)): Path<(String, Uuid)>,
) -> Result<Json<Vec<super::lifecycle::History>>> {
	let query = sea_orm::sea_query::Query::select()
		.expr(sea_orm::sea_query::SimpleExpr::from(
			sea_orm::sea_query::Expr::col((
				sea_orm::sea_query::Alias::new("h"),
				sea_orm::sea_query::Asterisk,
			)),
		))
		.from_as(
			sea_orm::sea_query::Alias::new("generation_history"),
			sea_orm::sea_query::Alias::new("h"),
		)
		.join_as(
			sea_orm::sea_query::JoinType::InnerJoin,
			sea_orm::sea_query::Alias::new("generation_requests"),
			sea_orm::sea_query::Alias::new("r"),
			sea_orm::sea_query::Expr::cust("r.id = h.request_id"),
		)
		.and_where(sea_orm::sea_query::Expr::cust(
			"r.tenant = $1 AND r.id = $2",
		))
		.order_by_expr(
			sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
				sea_orm::sea_query::Alias::new("h"),
				sea_orm::sea_query::Alias::new("sequence"),
			))),
			sea_orm::sea_query::Order::Asc,
		)
		.to_string(sea_orm::sea_query::PostgresQueryBuilder);
	match actor {
		Actor::Operator => Ok(Json(
			sqlx::query_as(&query)
				.bind(tenant)
				.bind(id)
				.fetch_all(&f.store.pool)
				.await?,
		)),
		Actor::Subject(identity) => {
			if tenant != identity.tenant {
				return Err(Error::Forbidden);
			}
			let mut access = Access::begin(&f.store, &identity).await?;
			let result = async {
				let job: Request = sqlx::query_as(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
						))
						.from(sea_orm::sea_query::Alias::new("generation_requests"))
						.and_where(sea_orm::sea_query::Expr::cust("tenant = $1 AND id = $2"))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(&tenant)
				.bind(id)
				.fetch_optional(&mut *access.tx)
				.await?
				.ok_or(Error::Forbidden)?;
				if !job.visible(&mut access).await? {
					return Err(Error::Forbidden);
				}
				Ok(sqlx::query_as(&query)
					.bind(tenant)
					.bind(id)
					.fetch_all(&mut *access.tx)
					.await?)
			}
			.await;
			Ok(Json(access.finish(result).await?))
		}
	}
}

#[derive(serde::Serialize, sqlx::FromRow, utoipa::ToSchema)]
#[schema(as=GenerationUsage)]
pub struct Usage {
	pub token_limit: i64,
	pub used_tokens: i64,
	pub inference_attempts: i64,
	pub compaction_call_limit: i64,
	pub compaction_calls: i64,
	pub embedding_calls: i64,
	pub embedding_call_limit: i64,
}
#[utoipa::path(get,path="/generation/{tenant}/requests/{id}/usage",operation_id="generation_usage",params(("tenant"=String,Path),("id"=Uuid,Path)),responses((status=200,body=Usage)),security(("bearer_auth"=[])))]
async fn usage(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((tenant, id)): Path<(String, Uuid)>,
) -> Result<Json<Usage>> {
	// One statement observes the counters and committed attempt ledger at the
	// same PostgreSQL snapshot while a worker reserves or settles its call.
	let query = sea_orm::sea_query::Query::select()
		.expr(sea_orm::sea_query::SimpleExpr::from(
			sea_orm::sea_query::Expr::col((
				sea_orm::sea_query::Alias::new("b"),
				sea_orm::sea_query::Alias::new("token_limit"),
			)),
		))
		.expr(sea_orm::sea_query::SimpleExpr::from(
			sea_orm::sea_query::Expr::col((
				sea_orm::sea_query::Alias::new("b"),
				sea_orm::sea_query::Alias::new("used_tokens"),
			)),
		))
		.expr(sea_orm::sea_query::SimpleExpr::from(
			sea_orm::sea_query::Expr::col((
				sea_orm::sea_query::Alias::new("b"),
				sea_orm::sea_query::Alias::new("compaction_call_limit"),
			)),
		))
		.expr(sea_orm::sea_query::SimpleExpr::from(
			sea_orm::sea_query::Expr::col((
				sea_orm::sea_query::Alias::new("b"),
				sea_orm::sea_query::Alias::new("compaction_calls"),
			)),
		))
		.expr(sea_orm::sea_query::SimpleExpr::from(
			sea_orm::sea_query::Expr::col((
				sea_orm::sea_query::Alias::new("b"),
				sea_orm::sea_query::Alias::new("embedding_calls"),
			)),
		))
		.expr(sea_orm::sea_query::SimpleExpr::from(
			sea_orm::sea_query::Expr::col((
				sea_orm::sea_query::Alias::new("b"),
				sea_orm::sea_query::Alias::new("embedding_call_limit"),
			)),
		))
		.expr_as(
			sea_orm::sea_query::Expr::cust(
				"(SELECT COUNT(*) FROM generation_usage AS u WHERE u.request_id = r.id)",
			),
			sea_orm::sea_query::Alias::new("inference_attempts"),
		)
		.from_as(
			sea_orm::sea_query::Alias::new("generation_requests"),
			sea_orm::sea_query::Alias::new("r"),
		)
		.join_as(
			sea_orm::sea_query::JoinType::InnerJoin,
			sea_orm::sea_query::Alias::new("generation_budgets"),
			sea_orm::sea_query::Alias::new("b"),
			sea_orm::sea_query::Expr::cust("b.request_id = r.id"),
		)
		.and_where(sea_orm::sea_query::Expr::cust(
			"r.tenant = $1 AND r.id = $2",
		))
		.to_string(sea_orm::sea_query::PostgresQueryBuilder);
	match actor {
		Actor::Operator => Ok(Json(
			sqlx::query_as(&query)
				.bind(tenant)
				.bind(id)
				.fetch_optional(&f.store.pool)
				.await?
				.ok_or(Error::Forbidden)?,
		)),
		Actor::Subject(identity) => {
			if tenant != identity.tenant {
				return Err(Error::Forbidden);
			}
			let mut access = Access::begin(&f.store, &identity).await?;
			let result = async {
				let job: Request = sqlx::query_as(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
						))
						.from(sea_orm::sea_query::Alias::new("generation_requests"))
						.and_where(sea_orm::sea_query::Expr::cust("tenant = $1 AND id = $2"))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(&tenant)
				.bind(id)
				.fetch_optional(&mut *access.tx)
				.await?
				.ok_or(Error::Forbidden)?;
				if !job.visible(&mut access).await? {
					return Err(Error::Forbidden);
				}
				Ok(sqlx::query_as(&query)
					.bind(tenant)
					.bind(id)
					.fetch_one(&mut *access.tx)
					.await?)
			}
			.await;
			Ok(Json(access.finish(result).await?))
		}
	}
}

#[utoipa::path(get,path="/generation/{tenant}/requests/{id}/spec",operation_id="generation_spec",params(("tenant"=String,Path),("id"=Uuid,Path)),responses((status=200,body=Spec)),security(("bearer_auth"=[])))]
async fn spec(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((tenant, id)): Path<(String, Uuid)>,
) -> Result<Json<Spec>> {
	let query = sea_orm::sea_query::Query::select()
		.expr(sea_orm::sea_query::SimpleExpr::from(
			sea_orm::sea_query::Expr::col((
				sea_orm::sea_query::Alias::new("h"),
				sea_orm::sea_query::Alias::new("spec"),
			)),
		))
		.from_as(
			sea_orm::sea_query::Alias::new("generation_requests"),
			sea_orm::sea_query::Alias::new("r"),
		)
		.join_as(
			sea_orm::sea_query::JoinType::InnerJoin,
			sea_orm::sea_query::Alias::new("generation_policy_history"),
			sea_orm::sea_query::Alias::new("h"),
			sea_orm::sea_query::Expr::cust(
				"h.tenant = r.tenant AND h.policy_id = r.policy_id AND h.revision = r.policy_revision",
			),
		)
		.and_where(sea_orm::sea_query::Expr::cust(
			"r.tenant = $1 AND r.id = $2",
		))
		.to_string(sea_orm::sea_query::PostgresQueryBuilder);
	match actor {
		Actor::Operator => {
			let document: serde_json::Value = sqlx::query_scalar(&query)
				.bind(tenant)
				.bind(id)
				.fetch_optional(&f.store.pool)
				.await?
				.ok_or(Error::Forbidden)?;
			Ok(Json(serde_json::from_value(document)?))
		}
		Actor::Subject(identity) => {
			if tenant != identity.tenant {
				return Err(Error::Forbidden);
			}
			let mut access = Access::begin(&f.store, &identity).await?;
			let result = async {
				let job: Request = sqlx::query_as(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
						))
						.from(sea_orm::sea_query::Alias::new("generation_requests"))
						.and_where(sea_orm::sea_query::Expr::cust("tenant = $1 AND id = $2"))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(&tenant)
				.bind(id)
				.fetch_optional(&mut *access.tx)
				.await?
				.ok_or(Error::Forbidden)?;
				if !job.visible(&mut access).await? {
					return Err(Error::Forbidden);
				}
				let document: serde_json::Value = sqlx::query_scalar(&query)
					.bind(tenant)
					.bind(id)
					.fetch_one(&mut *access.tx)
					.await?;
				Ok(serde_json::from_value(document)?)
			}
			.await;
			Ok(Json(access.finish(result).await?))
		}
	}
}
