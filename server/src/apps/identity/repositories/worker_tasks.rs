//! Worker task persistence retains atomic output/origin writes in one inherited transaction.
use crate::{
	Result as NativeResult, apps::identity::repositories::execution::Grant,
	apps::identity::services::access::Access, federation::Federation,
};
use aidash_application::{Result, ports::authorization::worker_tasks::WorkerTaskScope};
use aidash_domain::{
	NewTask, RunMetadata, Task,
	federation::Delegation,
	generation::requests::Assignment,
	identity::execution::{CreatedTaskOrigin, ExecutionGrant, ExecutionPrincipal, TaskOrigin},
	policy::Resource,
	registry::EntityRef,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait as _, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use uuid::Uuid;
pub(crate) struct WorkerTasks<'a> {
	pub(crate) federation: &'a Federation,
	pub(crate) access: &'a mut Access,
}
#[async_trait]
impl WorkerTaskScope for WorkerTasks<'_> {
	fn node_id(&self) -> &str {
		&self.federation.config.node_id
	}
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
	async fn assign(&mut self, task: Uuid, policy: &str, reason: &str) -> Result<Assignment> {
		crate::generation::assign_in(self.federation, self.access, task, policy, reason)
			.await
			.map_err(Into::into)
	}
	async fn record_output(&mut self, source: &RunMetadata, kind: &str, id: Uuid) -> Result<()> {
		self.federation
			.store
			.record_output_in(
				&mut self.access.tx,
				Some(source.id),
				source.workspace_id,
				kind,
				id,
			)
			.await
			.map_err(Into::into)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn related_tasks(&mut self, workspace: Uuid, input: &NewTask) -> Result<()> {
		self.access
			.related_tasks(workspace, input)
			.await
			.map_err(Into::into)
	}
	async fn create_task(
		&mut self,
		workspace: Uuid,
		input: &NewTask,
		creator: &str,
		key: &str,
	) -> Result<Task> {
		self.federation
			.store
			.create_task_in(&mut self.access.tx, workspace, input, creator, Some(key))
			.await
			.map_err(Into::into)
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn task_read(&mut self, task: Uuid) -> Result<Task> {
		self.access.task_read(task).await.map_err(Into::into)
	}
	async fn delegate(&mut self, task: Uuid, agent: &EntityRef) -> Result<Delegation> {
		crate::apps::identity::services::execution::delegate_in(
			self.federation,
			self.access,
			task,
			agent,
		)
		.await
		.map_err(Into::into)
	}
	async fn task_workspace(&mut self, task: Uuid) -> Result<Option<Uuid>> {
		let result: NativeResult<Option<Uuid>> = async {
			Ok({
				let query_bind_1 = task;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("workspace_id"))
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
				.scalar_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn created_origin(&mut self, task_id: Uuid) -> Result<CreatedTaskOrigin> {
		let result: NativeResult<CreatedTaskOrigin> = async {
			let origin: (Uuid, String, String, Vec<String>) = {
				let query_bind_1 = task_id;
				crate::database::native::query_as(
					&Query::select()
						.column(Alias::new("source_run_id"))
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
				.columns(&["source_run_id", "tenant", "root_subject", "subject_chain"])
				.fetch_one(&mut **self.access.tx)
				.await?
			};
			Ok(CreatedTaskOrigin {
				source_run_id: origin.0,
				authority: TaskOrigin {
					tenant: origin.1,
					root_subject: origin.2,
					subject_chain: origin.3,
				},
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn execution_grant(&mut self, task: Uuid) -> Result<Option<ExecutionGrant>> {
		let result: NativeResult<Option<ExecutionGrant>> = async {
			let grant: Option<Grant> = {
				let query_bind_1 = task;
				crate::database::native::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("authorization_execution"))
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
			Ok(grant.map(Into::into))
		}
		.await;
		result.map_err(Into::into)
	}
	async fn insert_origin(&mut self, task: Uuid, source: Uuid) -> Result<()> {
		let result: NativeResult<()> = async {
			{
				let query_bind_1 = task;
				let query_bind_2 = source;
				let query_bind_3 = &self.access.identity.tenant;
				let query_bind_4 = &self.access.identity.subject;
				let query_bind_5 = &self.access.subjects;
				crate::database::native::query(
					&Query::insert()
						.into_table(Alias::new("authorization_task_origins"))
						.columns([
							Alias::new("task_id"),
							Alias::new("source_run_id"),
							Alias::new("tenant"),
							Alias::new("root_subject"),
							Alias::new("subject_chain"),
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
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![crate::database::text_array(query_bind_5.to_owned())],
								))
								.to_owned(),
						)
						.on_conflict(OnConflict::columns(["task_id"]).do_nothing().to_owned())
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **self.access.tx)
				.await?;
			}
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
}
