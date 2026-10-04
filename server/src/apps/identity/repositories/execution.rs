//! Native grant reads retain the existing SHARE locks and policy transaction.
use crate::{
	Result as NativeResult,
	apps::identity::services::{access::Access, identity::SubjectIdentity},
	store::Store,
};
use aidash_application::{
	Result,
	ports::authorization::{ExecutionGrantRepository, ExecutionGrantSession},
};
use aidash_domain::{
	identity::execution::{ExecutionGrant, ExecutionPrincipal, TaskOrigin},
	policy::Resource,
};
use async_trait::async_trait;
use reinhardt::query::ColumnRef::Asterisk;
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, sqlx::FromRow)]
pub(crate) struct Grant {
	pub(crate) run_id: Uuid,
	pub(crate) task_id: Uuid,
	pub(crate) workspace_id: Uuid,
	pub(crate) tenant: String,
	pub(crate) credential_id: Uuid,
	pub(crate) root_subject: String,
	pub(crate) subject_chain: Vec<String>,
}
impl Grant {
	pub(crate) fn identity(&self) -> SubjectIdentity {
		SubjectIdentity {
			http_session: None,
			credential_id: self.credential_id,
			tenant: self.tenant.clone(),
			subject: self.root_subject.clone(),
		}
	}
}

impl From<Grant> for ExecutionGrant {
	fn from(value: Grant) -> Self {
		Self {
			run_id: value.run_id,
			task_id: value.task_id,
			workspace_id: value.workspace_id,
			tenant: value.tenant,
			credential_id: value.credential_id,
			root_subject: value.root_subject,
			subject_chain: value.subject_chain,
		}
	}
}
impl From<ExecutionGrant> for Grant {
	fn from(value: ExecutionGrant) -> Self {
		Self {
			run_id: value.run_id,
			task_id: value.task_id,
			workspace_id: value.workspace_id,
			tenant: value.tenant,
			credential_id: value.credential_id,
			root_subject: value.root_subject,
			subject_chain: value.subject_chain,
		}
	}
}
pub(crate) struct Grants<'a> {
	pub store: &'a Store,
}
pub(crate) struct GrantScope<'a> {
	pub access: &'a mut Access,
}

#[async_trait]
impl ExecutionGrantRepository for Grants<'_> {
	fn node_id(&self) -> &str {
		&self.store.node_id
	}
	async fn grant(&self, run: Uuid) -> Result<Option<ExecutionGrant>> {
		let result: NativeResult<Option<Grant>> = async {
			let grant: Option<Grant> = {
				let query_bind_1 = run;
				sqlx::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("authorization_execution"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("run_id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&self.store.pool)
				.await?
			};
			Ok(grant)
		}
		.await;
		result
			.map(|grant| grant.map(Into::into))
			.map_err(Into::into)
	}
	async fn require_legacy_remote_task(&self, home: &str, task: Uuid) -> Result<()> {
		self.store
			.require_legacy_remote_task(home, task)
			.await
			.map_err(Into::into)
	}
	async fn require_legacy_execution(&self, workspace: Uuid) -> Result<()> {
		self.store
			.require_legacy_execution(workspace)
			.await
			.map_err(Into::into)
	}
	async fn require_legacy_agent(&self, agent: &str, version: &str) -> Result<()> {
		self.store
			.require_legacy_agent(agent, version)
			.await
			.map_err(Into::into)
	}
}

#[async_trait]
impl ExecutionGrantSession for GrantScope<'_> {
	fn identity(&self) -> ExecutionPrincipal {
		ExecutionPrincipal {
			tenant: self.access.identity.tenant.clone(),
			subject: self.access.identity.subject.clone(),
			credential_id: self.access.identity.credential_id,
		}
	}
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn set_subjects(&mut self, subjects: Vec<String>) {
		self.access.subjects = subjects;
	}
	fn select_worker(&mut self, run: Uuid, durable_audit: Option<bool>) {
		if let Some(durable) = durable_audit {
			self.access.durable_audit = durable;
		}
		self.access.read_run = Some(run);
		self.access.worker();
	}
	async fn refresh(&mut self, run: Uuid) -> Result<()> {
		self.access.refresh_execution(run).await.map_err(Into::into)
	}
	async fn required_grant(&mut self, run: Uuid) -> Result<ExecutionGrant> {
		let result: NativeResult<Grant> = async {
			let current: Grant = {
				let query_bind_1 = run;
				sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("authorization_execution"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("run_id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **self.access.tx)
				.await?
			};
			Ok(current)
		}
		.await;
		result.map(Into::into).map_err(Into::into)
	}
	async fn optional_grant(&mut self, run: Uuid) -> Result<Option<ExecutionGrant>> {
		let result: NativeResult<Option<Grant>> = async {
			let grant: Option<Grant> = {
				let query_bind_1 = run;
				sqlx::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("authorization_execution"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("run_id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			};
			Ok(grant)
		}
		.await;
		result
			.map(|grant| grant.map(Into::into))
			.map_err(Into::into)
	}
	async fn local_origin(&mut self, task: Uuid) -> Result<Option<TaskOrigin>> {
		let result: NativeResult<Option<(String, String, Vec<String>)>> = async {
			let origin: Option<(String, String, Vec<String>)> = {
				let query_bind_1 = task;
				sqlx::query_as(
					&Query::select()
						.column(Alias::new("tenant"))
						.column(Alias::new("root_subject"))
						.column(Alias::new("subject_chain"))
						.from(Alias::new("authorization_task_origins"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("task_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								)),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			};
			Ok(origin)
		}
		.await;
		result
			.map(|origin| origin.map(Into::into))
			.map_err(Into::into)
	}
	async fn remote_origin(&mut self, task: Uuid) -> Result<Option<TaskOrigin>> {
		let result: NativeResult<Option<(String, String, Vec<String>)>> = async {
			let origin = {
				let query_bind_1 = task;
				sqlx::query_as(
					&Query::select()
						.columns([
							(Alias::new("g"), Alias::new("tenant")),
							(Alias::new("g"), Alias::new("root_subject")),
							(Alias::new("g"), Alias::new("subject_chain")),
						])
						.from_as(Alias::new("authorization_remote_outputs"), Alias::new("o"))
						.join(
							reinhardt::query::JoinType::InnerJoin,
							reinhardt::query::TableRef::table_alias(
								Alias::new("authorization_remote_grants"),
								Alias::new("g"),
							),
							Expr::cust("g.id=o.grant_id"),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(o.resource_kind='task' AND o.resource_id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			};
			Ok(origin)
		}
		.await;
		result
			.map(|origin| origin.map(Into::into))
			.map_err(Into::into)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	fn set_context(&mut self, context: Value) {
		self.access.context = context;
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
}

pub(crate) mod guard;

pub(crate) mod control;

pub(crate) mod details;

pub(crate) mod semantic;

pub(crate) mod boundaries;

pub(crate) mod resume;

pub(crate) mod admission;

pub(crate) mod entry;
