//! Generation projections and caller-owned visibility query adapters.
use crate::{Result, authorization::access::Access, database::Record};
use aidash_application::ports::generation::visibility::GenerationVisibility;
use aidash_domain::{
	entities::Task,
	generation::requests::{History, Request, Usage},
	policy::Resource,
};
use async_trait::async_trait;
use reinhardt::query::{Expr, QueryStatementBuilder, SimpleExpr};
use serde_json::Value;
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

impl Record for Request {
	fn decode(row: &PgRow) -> std::result::Result<Self, sqlx::Error> {
		Ok(Self {
			id: row.try_get("id")?,
			tenant: row.try_get("tenant")?,
			policy_id: row.try_get("policy_id")?,
			policy_revision: row.try_get("policy_revision")?,
			task_id: row.try_get("task_id")?,
			home_node: row.try_get("home_node")?,
			foreign_intent: row.try_get("foreign_intent")?,
			prepared: row.try_get("prepared")?,
			grant_id: row.try_get("grant_id")?,
			admission_id: row.try_get("admission_id")?,
			workspace_id: row.try_get("workspace_id")?,
			credential_id: row.try_get("credential_id")?,
			root_subject: row.try_get("root_subject")?,
			subject_chain: row.try_get("subject_chain")?,
			agent_id: row.try_get("agent_id")?,
			agent_version: row.try_get("agent_version")?,
			definition: row.try_get("definition")?,
			status: row.try_get("status")?,
			reason: row.try_get("reason")?,
			depth: row.try_get("depth")?,
			token_limit: row.try_get("token_limit")?,
			quota_released: row.try_get("quota_released")?,
			expires_at: row.try_get("expires_at")?,
			created_at: row.try_get("created_at")?,
		})
	}
}

impl Record for History {
	fn decode(row: &PgRow) -> std::result::Result<Self, sqlx::Error> {
		Ok(Self {
			sequence: row.try_get("sequence")?,
			request_id: row.try_get("request_id")?,
			status: row.try_get("status")?,
			actor: row.try_get("actor")?,
			reason: row.try_get("reason")?,
			created_at: row.try_get("created_at")?,
		})
	}
}

impl Record for Usage {
	fn decode(row: &PgRow) -> std::result::Result<Self, sqlx::Error> {
		Ok(Self {
			token_limit: row.try_get("token_limit")?,
			used_tokens: row.try_get("used_tokens")?,
			inference_attempts: row.try_get("inference_attempts")?,
			compaction_call_limit: row.try_get("compaction_call_limit")?,
			compaction_calls: row.try_get("compaction_calls")?,
			embedding_calls: row.try_get("embedding_calls")?,
			embedding_call_limit: row.try_get("embedding_call_limit")?,
		})
	}
}

pub(crate) struct NativeVisibility<'a> {
	pub access: &'a mut Access,
}
#[async_trait]
impl GenerationVisibility for NativeVisibility<'_> {
	fn inherited_lease(&self) -> bool {
		self.access.inherited_lease
	}
	fn context(&mut self, value: Value) {
		self.access.context = value;
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn workspace(&mut self, id: Uuid) -> aidash_application::Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn task(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> aidash_application::Result<Option<Task>> {
		let task: Option<crate::domain::Task> = {
			let query_bind_1 = id;
			let query_bind_2 = workspace;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND workspace_id = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.access.tx)
			.await
			.map_err(crate::Error::from)?
		};

		Ok(task)
	}
	async fn task_visible(&mut self, task: &Task) -> aidash_application::Result<bool> {
		self.access.task_visible(task).await.map_err(Into::into)
	}
	async fn decide(
		&mut self,
		resource: &Resource,
		action: &str,
	) -> aidash_application::Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
pub(crate) trait RequestAccess {
	async fn visible(&self, access: &mut Access) -> Result<bool>;
}
#[async_trait]
impl RequestAccess for Request {
	async fn visible(&self, access: &mut Access) -> Result<bool> {
		aidash_application::generation::visibility::visible(
			&mut crate::bootstrap::generation_visibility_scope(access),
			self,
		)
		.await
		.map_err(Into::into)
	}
}
