//! Admission rows retain their native query trees and the caller's exact transaction.
use crate::{
	Result as NativeResult,
	apps::identity::{
		repositories::execution::GrantScope,
		services::{access::Access, catalog},
	},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::authorization::{ExecutionGrantSession, admission::ExecutionAdmissionSession},
};
use aidash_domain::{
	Task,
	federation::Delegation,
	identity::execution::{ExecutionGrant, ExecutionPrincipal, TaskOrigin},
	policy::{PolicyBundle, Resource},
	registry::{AgentConfig, EntityRef, Entry},
};
use async_trait::async_trait;
use reinhardt::query::ColumnRef::Asterisk;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait as _, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::{Value, json};
use uuid::Uuid;
pub(crate) struct Admission<'a> {
	pub f: &'a Federation,
	pub access: &'a mut Access,
}
#[async_trait]
impl ExecutionGrantSession for Admission<'_> {
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
	fn select_worker(&mut self, run: Uuid, durable: Option<bool>) {
		GrantScope {
			access: self.access,
		}
		.select_worker(run, durable);
	}
	async fn refresh(&mut self, run: Uuid) -> Result<()> {
		GrantScope {
			access: self.access,
		}
		.refresh(run)
		.await
	}
	async fn required_grant(&mut self, run: Uuid) -> Result<ExecutionGrant> {
		GrantScope {
			access: self.access,
		}
		.required_grant(run)
		.await
	}
	async fn optional_grant(&mut self, run: Uuid) -> Result<Option<ExecutionGrant>> {
		GrantScope {
			access: self.access,
		}
		.optional_grant(run)
		.await
	}
	async fn local_origin(&mut self, task: Uuid) -> Result<Option<TaskOrigin>> {
		GrantScope {
			access: self.access,
		}
		.local_origin(task)
		.await
	}
	async fn remote_origin(&mut self, task: Uuid) -> Result<Option<TaskOrigin>> {
		GrantScope {
			access: self.access,
		}
		.remote_origin(task)
		.await
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
#[async_trait]
impl ExecutionAdmissionSession for Admission<'_> {
	fn node_id(&self) -> &str {
		&self.f.config.node_id
	}
	fn bundle(&self) -> &PolicyBundle {
		&self.access.snapshot.bundle
	}
	async fn task(&mut self, id: Uuid) -> Result<Option<Task>> {
		let result: NativeResult<Option<Task>> = async {
			let task_id = id;
			let access = &mut *self.access;
			let task: Option<Task> = {
				let query_bind_1 = task_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("tasks"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(task)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn claimed_run(&mut self, task: Uuid) -> Result<Uuid> {
		let result: NativeResult<Uuid> = async {
			let access = &mut *self.access;
			let f = self.f;
			let run_id: Uuid = {
				let query_bind_1 = &f.config.node_id;
				let query_bind_2 = task;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("runs"))
						.cond_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"home_node",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_1.to_owned()).into()],
									)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"task_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **access.tx)
				.await?
			};
			Ok(run_id)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn persist_grant(&mut self, grant: &ExecutionGrant) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			let run_id = grant.run_id;
			{
				let query_bind_1 = run_id;
				let query_bind_2 = grant.task_id;
				let query_bind_3 = grant.workspace_id;
				let query_bind_4 = &grant.tenant;
				let query_bind_5 = grant.credential_id;
				let query_bind_6 = &grant.root_subject;
				let query_bind_7 = &grant.subject_chain;
				sqlx::query(
					&Query::insert()
						.into_table(Alias::new("authorization_execution"))
						.columns([
							Alias::new("run_id"),
							Alias::new("task_id"),
							Alias::new("workspace_id"),
							Alias::new("tenant"),
							Alias::new("credential_id"),
							Alias::new("root_subject"),
							Alias::new("subject_chain"),
						])
						.from_subquery(
							reinhardt::query::Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_5.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_6.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![crate::database::text_array(query_bind_7.to_owned())],
								))
								.to_owned(),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **access.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn dashboard_origin(&mut self, credential: Uuid) -> Result<Option<(Uuid, Uuid)>> {
		let result: NativeResult<Option<(Uuid, Uuid)>> = async {
			let access = &mut *self.access;
			let origin: Option<(Uuid, Uuid)> = {
				let query_bind_1 = credential;
				sqlx::query_as(
					&Query::select()
						.columns([Alias::new("identity_id"), Alias::new("id")])
						.from(Alias::new("dashboard_mappings"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
								"credential_id",
							)))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							)),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(origin)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn persist_dashboard_origin(
		&mut self,
		run: Uuid,
		identity: Uuid,
		mapping: Uuid,
	) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			let run_id = run;
			let identity_id = identity;
			let mapping_id = mapping;
			{
				let query_bind_1 = run_id;
				let query_bind_2 = identity_id;
				let query_bind_3 = mapping_id;
				sqlx::query(
					&Query::insert()
						.into_table(Alias::new("dashboard_execution_origins"))
						.columns([
							Alias::new("run_id"),
							Alias::new("identity_id"),
							Alias::new("mapping_id"),
						])
						.from_subquery(
							reinhardt::query::Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								))
								.to_owned(),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **access.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn local_delegation(&mut self, task: Uuid, agent: &EntityRef) -> Result<Delegation> {
		let result: NativeResult<Delegation> = async {
			let access = &mut *self.access;
			let f = self.f;
			let delegation: Delegation = {
				let query_bind_1 = task;
				let query_bind_2 = &f.config.node_id;
				let query_bind_3 = &agent.id;
				let query_bind_4 = &agent.version;
				crate::database::query_as(
					&Query::insert()
						.into_table(Alias::new("delegations"))
						.columns([
							Alias::new("task_id"),
							Alias::new("node_id"),
							Alias::new("agent_id"),
							Alias::new("agent_version"),
							Alias::new("delivered"),
						])
						.from_subquery(
							Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()],
								))
								.expr(Expr::cust("true"))
								.to_owned(),
						)
						.returning([
							Alias::new("task_id"),
							Alias::new("node_id"),
							Alias::new("agent_id"),
							Alias::new("agent_version"),
							Alias::new("delivered"),
						])
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **access.tx)
				.await?
			};
			Ok(delegation)
		}
		.await;
		result.map_err(Into::into)
	}

	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn require_live_agent(&mut self, task: Uuid, agent: &EntityRef) -> Result<()> {
		crate::generation::provision::require_live(self.access, &self.f.config.node_id, task, agent)
			.await
			.map_err(Into::into)
	}
	async fn executable_entry(&mut self, agent: &EntityRef) -> Result<Entry> {
		catalog::entry(self.access, agent, "agent.execute")
			.await
			.map_err(Into::into)
	}
	async fn active_installation(&mut self, entry: &Entry) -> Result<bool> {
		crate::marketplace::active(self.access, entry)
			.await
			.map_err(Into::into)
	}
	async fn check_pinned_installation(&mut self, entry: &Entry) -> Result<()> {
		crate::marketplace::check_pinned(self.access, entry)
			.await
			.map_err(Into::into)
	}
	async fn prepare_thread(
		&mut self,
		task: &Task,
		config: &AgentConfig,
		agent: &str,
	) -> Result<Option<Uuid>> {
		crate::capabilities::sessions::prepare_admission(
			&self.f.store,
			self.access,
			task,
			config,
			agent,
		)
		.await
		.map_err(Into::into)
	}
	async fn claim(
		&mut self,
		task: &Task,
		revision: i64,
		subject: &str,
		entry: &Entry,
	) -> Result<Task> {
		self.f
			.store
			.claim_in(&mut self.access.tx, task, revision, subject, entry)
			.await
			.map_err(Into::into)
	}
	async fn admit_thread(
		&mut self,
		task: &Task,
		run: Uuid,
		thread: Option<Uuid>,
		config: &AgentConfig,
		agent: &str,
	) -> Result<()> {
		crate::capabilities::sessions::admit(
			&self.f.store,
			self.access,
			task,
			run,
			thread,
			config,
			agent,
		)
		.await
		.map_err(Into::into)
	}
	async fn delegation_event(&mut self, workspace: Uuid, delegation: &Delegation) -> Result<()> {
		self.f
			.store
			.event(
				&mut self.access.tx,
				Some(workspace),
				"task.delegated",
				json!(delegation),
			)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
