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
use reinhardt::db::orm::{Model, QueryRow, execution::convert_values};
use reinhardt::query::LockType;
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
	async fn subnode_members(&mut self, thread: Uuid) -> Result<Option<Vec<EntityRef>>> {
		use crate::apps::execution::models::subnodes::{SubnodeMemberRecord, SubnodeRecord};
		let tenant = self.access.identity.tenant.clone();
		let result: NativeResult<Option<Vec<EntityRef>>> = async {
			// The same exclusive row lock must guard membership changes and ending.
			// It keeps a target a member until delegated admission commits.
			let (sql, values) = Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("subnodes"))
				.and_where(Expr::col("tenant").eq(tenant.clone()))
				.and_where(Expr::col("thread_id").eq(Expr::value(thread)))
				.lock(LockType::Update)
				.build(PostgresQueryBuilder);
			let row = self
				.access
				.tx
				.native_mut()
				.fetch_optional(&sql, convert_values(values))
				.await?;
			let Some(row) = row else {
				return Ok(None);
			};
			let record: SubnodeRecord =
				serde_json::from_value(QueryRow::from_backend_row(row).data)?;
			if record.state != "active" {
				return Ok(Some(vec![]));
			}
			let members = SubnodeMemberRecord::objects()
				.filter(SubnodeMemberRecord::field_tenant().eq(tenant))
				.filter(SubnodeMemberRecord::field_subnode_id().eq(record.id))
				.all_with_executor(self.access.tx.native_mut())
				.await
				.map_err(reinhardt::core::exception::Error::from)?;
			Ok(Some(
				members
					.into_iter()
					.map(|member| EntityRef {
						id: member.agent_id,
						version: member.agent_version,
					})
					.collect(),
			))
		}
		.await;
		result.map_err(Into::into)
	}
	fn node_id(&self) -> &str {
		self.federation.config.deployment.as_str()
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
	async fn assign(
		&mut self,
		task: Uuid,
		policy: &str,
		reason: &str,
		members: Option<&[EntityRef]>,
		draft: Option<&aidash_domain::generation::policy::Draft>,
	) -> Result<Assignment> {
		crate::generation::assign_in_restricted(
			self.federation,
			self.access,
			task,
			policy,
			reason,
			draft,
			members,
		)
		.await
		.map_err(Into::into)
	}
	async fn record_output(&mut self, source: &RunMetadata, kind: &str, id: Uuid) -> Result<()> {
		self.federation
			.store
			.record_output_in(
				&mut self.access.tx,
				Some(source.id),
				source.thread_id,
				kind,
				id,
			)
			.await
			.map_err(Into::into)
	}
	async fn thread(&mut self, id: Uuid) -> Result<Resource> {
		self.access.thread(id).await.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn related_tasks(&mut self, thread: Uuid, input: &NewTask) -> Result<()> {
		self.access
			.related_tasks(thread, input)
			.await
			.map_err(Into::into)
	}
	async fn create_task(
		&mut self,
		thread: Uuid,
		input: &NewTask,
		creator: &str,
		key: &str,
	) -> Result<Task> {
		self.federation
			.store
			.create_task_in(&mut self.access.tx, thread, input, creator, Some(key))
			.await
			.map_err(Into::into)
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn task_read(&mut self, task: Uuid) -> Result<Task> {
		self.access.task_read(task).await.map_err(Into::into)
	}
	async fn task_source_run(&mut self, task: Uuid) -> Result<Option<Uuid>> {
		self.access.task_source_run(task).await.map_err(Into::into)
	}
	async fn own_task_read(
		&mut self,
		task: Uuid,
	) -> Result<
		std::result::Result<Task, aidash_application::authorization::worker_tasks::OwnTaskRefusal>,
	> {
		self.access.own_task_read(task).await.map_err(Into::into)
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
	async fn task_thread(&mut self, task: Uuid) -> Result<Option<Uuid>> {
		let result: NativeResult<Option<Uuid>> = async {
			Ok({
				let query_bind_1 = task;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("thread_id"))
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
			// Retain insertion-time source existence, not a perpetual Run lifetime.
			let source_row = crate::database::native::query(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("task_executions"))
					.and_where(Expr::col("id").eq(Expr::value(source)))
					.and_where(
						Expr::col("tenant")
							.eq(crate::database::scope::tenant_of("tasks", "id", task)),
					)
					.lock(reinhardt::query::LockType::KeyShare)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.access.tx)
			.await?;
			if source_row.is_none() {
				return Err(crate::Error::Forbidden);
			}

			crate::database::native::query(
				&Query::insert()
					.into_table(Alias::new("authorization_task_origins"))
					.columns(
						[
							"task_id",
							"source_run_id",
							"tenant",
							"root_subject",
							"subject_chain",
							"federated",
						]
						.map(Alias::new),
					)
					.from_subquery(
						Query::select()
							.expr(Expr::value(task))
							.expr(Expr::value(source))
							.expr(Expr::value(&self.access.identity.tenant))
							.expr(Expr::value(&self.access.identity.subject))
							.expr(crate::database::text_array(self.access.subjects.clone()))
							.expr(super::execution::federated_source(
								source,
								&self.access.identity.tenant,
							))
							.to_owned(),
					)
					.on_conflict(OnConflict::columns(["task_id"]).do_nothing().to_owned())
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **self.access.tx)
			.await?;
			// A Task a Duty's Run creates works for that Duty too (#1012).
			crate::database::native::query(
				&Query::update()
					.table(Alias::new("tasks"))
					.value_expr(
						Alias::new("duty_id"),
						SimpleExpr::CustomWithExpr(
							"(SELECT s.duty_id FROM authorization_execution e JOIN tasks s ON s.id = e.task_id AND s.tenant = e.tenant WHERE e.run_id = ? AND e.tenant = ?)".to_owned(),
							vec![
								Expr::value(source).into(),
								Expr::value(self.access.identity.tenant.clone()).into(),
							],
						),
					)
					.and_where(Expr::col("id").eq(Expr::value(task)))
					.and_where(Expr::col("tenant").eq(Expr::value(&self.access.identity.tenant)))
					.and_where(Expr::col("duty_id").is_null())
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **self.access.tx)
			.await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
}
