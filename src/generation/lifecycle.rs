//! Lifecycle mutations take the tenant policy lock exclusively before any job
//! lock. Workers hold a shared lease while making protected decisions, release
//! it during external provider waits, then reacquire it and recheck authority
//! before accepting output or starting another effect.
use super::Request;
use crate::{Error, Result, authorization::access::Access, federation::Federation};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = GenerationAction)]
pub enum Action {
	Approve,
	Deny,
	Stop,
	Delete,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = GenerationControl)]
pub struct Control {
	pub action: Action,
	pub reason: String,
}
#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
#[schema(as = GenerationHistory)]
pub struct History {
	pub sequence: i64,
	pub request_id: Uuid,
	pub status: String,
	pub actor: String,
	pub reason: String,
	pub created_at: DateTime<Utc>,
}

pub(crate) async fn load(
	tx: &mut Transaction<'_, Postgres>,
	tenant: &str,
	id: Uuid,
) -> Result<Request> {
	sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("generation_requests"))
			.and_where(sea_orm::sea_query::Expr::cust("tenant = $1 AND id = $2"))
			.lock(sea_orm::sea_query::LockType::Update)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(tenant)
	.bind(id)
	.fetch_optional(&mut **tx)
	.await?
	.ok_or(Error::Forbidden)
}
impl Request {
	pub(crate) async fn visible(&self, access: &mut Access) -> Result<bool> {
		if !access.inherited_lease {
			access.context = json!({});
		}
		let workspace = access.workspace(self.workspace_id).await?;
		let task: Option<crate::domain::Task> = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND workspace_id = $2",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(self.task_id)
		.bind(self.workspace_id)
		.fetch_optional(&mut **access.tx)
		.await?;
		let Some(task) = task else {
			return Ok(false);
		};
		if !access.task_visible(&task).await? {
			return Ok(false);
		}

		access.context = workspace.attributes.clone();
		Ok(access.decide(&workspace, "workspace.read").await?
			&& access
				.decide(&self.resource(access), "generation.read")
				.await?)
	}
	pub(crate) fn resource(&self, access: &Access) -> crate::authorization::policy::Resource {
		access.resource(
			"generation",
			self.id,
			json!({"policy_id":self.policy_id,"root_subject":self.root_subject,"task_id":self.task_id}),
		)
	}
}
pub(crate) async fn transition(
	f: &Federation,
	tx: &mut Transaction<'_, Postgres>,
	job: &Request,
	status: &str,
	actor: &str,
	reason: &str,
) -> Result<Request> {
	if matches!(
		status,
		"COMPLETED" | "DENIED" | "STOPPED" | "EXPIRED" | "FAILED" | "DELETED"
	) && !job.quota_released
	{
		let (unused, unused_calls, unused_embeddings): (i64, i64, i64) = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust("token_limit - used_tokens"))
				.expr(sea_orm::sea_query::Expr::cust(
					"compaction_call_limit - compaction_calls",
				))
				.expr(sea_orm::sea_query::Expr::cust(
					"embedding_call_limit - embedding_calls",
				))
				.from(sea_orm::sea_query::Alias::new("generation_budgets"))
				.and_where(sea_orm::sea_query::Expr::cust("request_id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(job.id)
		.fetch_one(&mut **tx)
		.await?;
		sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("generation_policies"))
				.value(
					sea_orm::sea_query::Alias::new("allocated_tokens"),
					sea_orm::sea_query::Expr::cust("allocated_tokens - $3"),
				)
				.value(
					sea_orm::sea_query::Alias::new("allocated_compaction_calls"),
					sea_orm::sea_query::Expr::cust("allocated_compaction_calls - $4"),
				)
				.value(
					sea_orm::sea_query::Alias::new("allocated_embedding_calls"),
					sea_orm::sea_query::Expr::cust("allocated_embedding_calls - $5"),
				)
				.and_where(sea_orm::sea_query::Expr::cust("tenant = $1 AND id = $2"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(&job.tenant)
		.bind(&job.policy_id)
		.bind(unused)
		.bind(unused_calls)
		.bind(unused_embeddings)
		.execute(&mut **tx)
		.await?;
		sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("generation_requests"))
				.value(
					sea_orm::sea_query::Alias::new("quota_released"),
					sea_orm::sea_query::Expr::cust("TRUE"),
				)
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(job.id)
		.execute(&mut **tx)
		.await?;
	}
	if matches!(status, "STOPPED" | "EXPIRED" | "DELETED") {
		sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("runs"))
				.value(
					sea_orm::sea_query::Alias::new("control"),
					sea_orm::sea_query::Expr::cust("'CANCELLED'"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"task_id = $1 AND NOT phase IN ('COMPLETED', 'FAILED', 'CANCELLED')",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(job.task_id)
		.execute(&mut **tx)
		.await?;
	}
	if matches!(
		status,
		"COMPLETED" | "DENIED" | "STOPPED" | "EXPIRED" | "FAILED" | "DELETED"
	) {
		let mut snapshot =
			crate::authorization::Authorization::load_with_mode(tx, &job.tenant, true).await?;
		let subject =
			crate::domain::qualified_agent(&f.config.node_id, &job.agent_id, &job.agent_version);
		if let Some(subject) = snapshot.bundle.subjects.get_mut(&subject)
			&& subject.enabled
		{
			subject.enabled = false;
			snapshot.revision = snapshot
				.revision
				.checked_add(1)
				.ok_or_else(|| Error::Invalid("authorization revision exhausted".into()))?;
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("authorization_bundles"))
					.value(
						sea_orm::sea_query::Alias::new("revision"),
						sea_orm::sea_query::Expr::cust("$2"),
					)
					.value(
						sea_orm::sea_query::Alias::new("document"),
						sea_orm::sea_query::Expr::cust("$3"),
					)
					.value(
						sea_orm::sea_query::Alias::new("updated_at"),
						sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP"),
					)
					.and_where(sea_orm::sea_query::Expr::cust("tenant = $1"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(&job.tenant)
			.bind(snapshot.revision)
			.bind(json!(snapshot.bundle))
			.execute(&mut **tx)
			.await?;
			sqlx::query(
				&sea_orm::sea_query::Query::insert()
					.into_table(sea_orm::sea_query::Alias::new("authorization_revisions"))
					.columns([
						sea_orm::sea_query::Alias::new("tenant"),
						sea_orm::sea_query::Alias::new("revision"),
						sea_orm::sea_query::Alias::new("document"),
						sea_orm::sea_query::Alias::new("actor"),
					])
					.values_panic([
						sea_orm::sea_query::Expr::cust("$1"),
						sea_orm::sea_query::Expr::cust("$2"),
						sea_orm::sea_query::Expr::cust("$3"),
						sea_orm::sea_query::Expr::cust("$4"),
					])
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(&job.tenant)
			.bind(snapshot.revision)
			.bind(json!(snapshot.bundle))
			.bind(actor)
			.execute(&mut **tx)
			.await?;
		}
		let revision: Option<i64> = sqlx::query_scalar(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("authorization_catalog"))
				.value(
					sea_orm::sea_query::Alias::new("enabled"),
					sea_orm::sea_query::Expr::cust("FALSE"),
				)
				.value(
					sea_orm::sea_query::Alias::new("revision"),
					sea_orm::sea_query::Expr::cust("revision + 1"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"tenant = $1 AND entry_id = $2 AND entry_version = $3 AND enabled",
				))
				.returning(sea_orm::sea_query::Query::returning().exprs([
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("revision"),
					)),
				]))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(&job.tenant)
		.bind(&job.agent_id)
		.bind(&job.agent_version)
		.fetch_optional(&mut **tx)
		.await?;
		if let Some(revision) = revision {
			sqlx::query(
				&sea_orm::sea_query::Query::insert()
					.into_table(sea_orm::sea_query::Alias::new(
						"authorization_catalog_history",
					))
					.columns([
						sea_orm::sea_query::Alias::new("tenant"),
						sea_orm::sea_query::Alias::new("entry_id"),
						sea_orm::sea_query::Alias::new("entry_version"),
						sea_orm::sea_query::Alias::new("revision"),
						sea_orm::sea_query::Alias::new("enabled"),
						sea_orm::sea_query::Alias::new("actor"),
					])
					.values_panic([
						sea_orm::sea_query::Expr::cust("$1"),
						sea_orm::sea_query::Expr::cust("$2"),
						sea_orm::sea_query::Expr::cust("$3"),
						sea_orm::sea_query::Expr::cust("$4"),
						sea_orm::sea_query::Expr::cust("FALSE"),
						sea_orm::sea_query::Expr::cust("$5"),
					])
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(&job.tenant)
			.bind(&job.agent_id)
			.bind(&job.agent_version)
			.bind(revision)
			.bind(actor)
			.execute(&mut **tx)
			.await?;
		}
	}
	let updated = sqlx::query_as(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("generation_requests"))
			.value(
				sea_orm::sea_query::Alias::new("status"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.returning_all()
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(job.id)
	.bind(status)
	.fetch_one(&mut **tx)
	.await?;
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("generation_history"))
			.columns([
				sea_orm::sea_query::Alias::new("request_id"),
				sea_orm::sea_query::Alias::new("status"),
				sea_orm::sea_query::Alias::new("actor"),
				sea_orm::sea_query::Alias::new("reason"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("$3"),
				sea_orm::sea_query::Expr::cust("$4"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(job.id)
	.bind(status)
	.bind(actor)
	.bind(reason)
	.execute(&mut **tx)
	.await?;
	f.store
		.event(
			tx,
			Some(job.workspace_id),
			"generation.changed",
			json!({"id":job.id,"task_id":job.task_id,"policy_id":job.policy_id,"status":status}),
		)
		.await?;
	Ok(updated)
}
pub(crate) async fn control(
	f: &Federation,
	tx: &mut Transaction<'_, Postgres>,
	job: &Request,
	input: &Control,
	actor: &str,
) -> Result<Request> {
	crate::domain::nonempty(&input.reason, "control reason")?;
	if input.reason.len() > 4096 {
		return Err(Error::Invalid("control reason exceeds 4096 bytes".into()));
	}
	let status = match input.action {
		Action::Approve => "QUEUED",
		Action::Deny => "DENIED",
		Action::Stop => "STOPPED",
		Action::Delete => "DELETED",
	};
	let replay:bool=sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("EXISTS(SELECT 1 FROM generation_history WHERE request_id = $1 AND status = $2 AND actor = $3 AND reason = $4)")).to_string(sea_orm::sea_query::PostgresQueryBuilder))
        .bind(job.id).bind(status).bind(actor).bind(&input.reason).fetch_one(&mut **tx).await?;
	if replay {
		return load(tx, &job.tenant, job.id).await;
	}
	if job.status == status {
		return Err(Error::Conflict(
			"generation decision already recorded".into(),
		));
	}
	let active = matches!(
		job.status.as_str(),
		"PENDING_APPROVAL" | "QUEUED" | "ACTIVE"
	);
	let valid = match input.action {
		Action::Approve | Action::Deny => job.status == "PENDING_APPROVAL",
		Action::Stop => active,
		Action::Delete => !active,
	};
	if !valid {
		return Err(Error::Conflict(
			"invalid generation state transition".into(),
		));
	}
	if matches!(input.action, Action::Approve) && job.expires_at <= Utc::now() {
		return Err(Error::Conflict("generation request expired".into()));
	}
	transition(f, tx, job, status, actor, &input.reason).await
}
