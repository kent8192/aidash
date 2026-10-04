//! Native Home ports retain original SQL, lock order, cancellation fences and audited transactions.
use super::remote_grants::{Grant as GrantRow, HomeBinding as BindingRow};
use crate::{
	Error as NativeError, Result as NativeResult,
	authorization::{access::Access, identity::Actor},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::authorization::home::{HomeRepository, HomeScope},
};
use aidash_domain::{
	NewTask, Task, TaskStatus,
	federation::execution::{
		Description, PrepareInput, Prepared,
		admission::Admission,
		home::{Grant, HomeBinding},
	},
	identity::execution::ExecutionPrincipal,
	policy::Resource,
	semantic::{
		Failure,
		remote::{
			Binding,
			status::{Provenance, Status},
		},
	},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, ColumnRef::Asterisk, Expr, ExprTrait as _, LockType, PostgresQueryBuilder,
	Query, QueryStatementBuilder as _, SimpleExpr,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use uuid::Uuid;
pub(crate) struct Repository {
	pub(crate) runtime: Federation,
	pub(crate) actor: Actor,
}
pub(crate) struct Scope {
	pub(crate) runtime: Federation,
	pub(crate) access: Box<Access>,
}
pub(crate) async fn binding(access: &mut Access, grant: Uuid) -> NativeResult<Option<BindingRow>> {
	Ok({
		let query_bind_1 = grant;
		sqlx::query_as(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("authorization_remote_execution"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("grant_id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	})
}
#[async_trait]
impl HomeScope for Scope {
	fn identity(&self) -> ExecutionPrincipal {
		let id = &self.access.identity;
		ExecutionPrincipal {
			tenant: id.tenant.clone(),
			subject: id.subject.clone(),
			credential_id: id.credential_id,
		}
	}
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn task_read(&mut self, id: Uuid) -> Result<Task> {
		self.access.task_read(id).await.map_err(Into::into)
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
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
	async fn binding(&mut self, grant: Uuid) -> Result<Option<HomeBinding>> {
		binding(&mut self.access, grant)
			.await
			.map(|r| r.map(Into::into))
			.map_err(Into::into)
	}
	async fn control_task(&mut self, id: Uuid) -> Result<Option<Task>> {
		let result: NativeResult<Option<Task>> = async {
			let access = &mut *self.access;
			Ok({
				let query_bind_1 = id;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("tasks"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn requester_grant(&mut self, task: Uuid, id: Uuid) -> Result<Option<Grant>> {
		let result: NativeResult<Option<GrantRow>> = async {
			let identity = self.access.identity.clone();
			let access = &mut *self.access;
			Ok({
				let query_bind_1 = id;
				let query_bind_2 = task;
				let query_bind_3 = &identity.tenant;
				let query_bind_4 = &identity.subject;
				sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("authorization_remote_grants"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("task_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
								"root_subject",
							)))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							)),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			})
		}
		.await;
		result.map(|r| r.map(Into::into)).map_err(Into::into)
	}
	async fn grants(&mut self, task: Uuid) -> Result<Vec<Grant>> {
		let result: NativeResult<Vec<GrantRow>> = async {
			let identity = self.access.identity.clone();
			let access = &mut *self.access;
			Ok({
				let query_bind_1 = task;
				let query_bind_2 = &identity.tenant;
				let query_bind_3 = &identity.subject;
				sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("authorization_remote_grants"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(task_id=? AND tenant=? AND root_subject=?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
								Expr::value(query_bind_3.to_owned()).into(),
							],
						))
						.order_by(Alias::new("expires_at"), reinhardt::query::Order::Desc)
						.limit(100)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **access.tx)
				.await?
			})
		}
		.await;
		result
			.map(|r| r.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
	async fn delegation_grants(&mut self, task: &Task, node: &str) -> Result<Vec<Grant>> {
		let result:NativeResult<Vec<GrantRow>>=async {let identity=self.access.identity.clone();let access=&mut *self.access;Ok({ let query_bind_1 = task.id; let query_bind_2 = node; let query_bind_3 = &identity.tenant; let query_bind_4 = &identity.subject; sqlx::query_as(&Query::select().column(Asterisk).from(Alias::new("authorization_remote_grants"))
            .and_where(SimpleExpr::CustomWithExpr("(task_id=? AND node_id=? AND tenant=? AND root_subject=? AND NOT revoked AND expires_at>CLOCK_TIMESTAMP())".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.to_owned()).into()]))
            .order_by(Alias::new("expires_at"),reinhardt::query::Order::Desc).limit(100).to_string(PostgresQueryBuilder)).fetch_all(&mut **access.tx).await? })}.await;
		result
			.map(|r| r.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
	async fn insert_binding(
		&mut self,
		id: Uuid,
		task: Uuid,
		admission: &Admission,
		description: &Description,
	) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = id;
				let query_bind_2 = admission.id;
				let query_bind_3 = task;
				let query_bind_4 = description.task.revision;
				let query_bind_5 = json!(description.task);
				sqlx::query(&format!(
					"{} ON CONFLICT DO NOTHING",
					Query::insert()
						.into_table(Alias::new("authorization_remote_execution"))
						.columns(
							[
								"grant_id",
								"admission_id",
								"task_id",
								"task_revision",
								"initial_task",
							]
							.map(Alias::new),
						)
						.from_subquery(
							reinhardt::query::Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()]
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_5.to_owned()).into()]
								))
								.to_owned()
						)
						.to_owned()
						.to_string(PostgresQueryBuilder)
				))
				.execute(&mut **access.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn revoke(&mut self, id: Uuid) -> Result<()> {
		let result: NativeResult<()> = async {
			let identity = self.access.identity.clone();
			let access = &mut *self.access;
			{
				let query_bind_1 = id;
				let query_bind_2 = &identity.tenant;
				let query_bind_3 = &identity.subject;
				sqlx::query(
					&Query::update()
						.table(Alias::new("authorization_remote_grants"))
						.value(Alias::new("revoked"), true)
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=? AND tenant=? AND root_subject=?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
								Expr::value(query_bind_3.to_owned()).into(),
							],
						))
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
	async fn delivered_inputs(&mut self, task: Uuid, admission: Uuid) -> Result<Vec<String>> {
		let result: NativeResult<Vec<String>> = async {
			let access = &mut *self.access;
			Ok({
				let query_bind_1 = task;
				let query_bind_2 = admission;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("idempotency_key"))
						.from(Alias::new("remote_run_message_fences"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(task_id=? AND run_id=?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn binding_revision(&mut self, id: Uuid, revision: i64) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = id;
				let query_bind_2 = revision;
				sqlx::query(
					&Query::update()
						.table(Alias::new("authorization_remote_execution"))
						.value_expr(
							Alias::new("task_revision"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(grant_id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
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
	async fn receipt(&mut self, id: Uuid) -> Result<Option<Value>> {
		let result: NativeResult<Option<Value>> = async {
			let access = &mut *self.access;
			Ok({
				let query_bind_1 = id;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("receipt"))
						.from(Alias::new("semantic_remote_operations"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(grant_id=? AND receipt IS NOT NULL)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.order_by(Alias::new("created_at"), reinhardt::query::Order::Desc)
						.order_by(Alias::new("id"), reinhardt::query::Order::Desc)
						.limit(1)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn cancel_task(
		&mut self,
		task: Uuid,
		revision: i64,
		owner: &str,
		admission: Uuid,
		keys: &[String],
	) -> Result<Task> {
		self.runtime
			.store
			.transition_remote_terminal_in(
				&mut self.access.tx,
				task,
				revision,
				owner,
				TaskStatus::Cancelled,
				admission,
				crate::store::TerminalRunMessageInputs::Keys(keys),
			)
			.await
			.map_err(Into::into)
	}
	async fn resume_semantic(
		&mut self,
		grant: Uuid,
		admission: Uuid,
		workspace: Uuid,
		subject: &str,
	) -> Result<()> {
		crate::semantic::remote::journal::resume_in(
			&self.runtime.store,
			&mut self.access.tx,
			grant,
			admission,
			workspace,
			subject,
		)
		.await
		.map_err(Into::into)
	}
	async fn remote_semantic_sources(&mut self, grant: Uuid) -> Result<()> {
		self.access
			.remote_semantic_sources(grant)
			.await
			.map_err(Into::into)
	}
	async fn grant_output_visible(&mut self, grant: Uuid) -> Result<bool> {
		self.access
			.grant_output_visible(grant)
			.await
			.map_err(Into::into)
	}
	async fn provenance(&mut self, value: Option<Value>) -> Result<Option<Provenance>> {
		crate::semantic::remote::status::provenance(
			&mut self.access,
			&self.runtime.config.node_id,
			value,
		)
		.await
		.map_err(Into::into)
	}
	async fn create_task(
		&mut self,
		workspace: Uuid,
		input: &NewTask,
		subject: &str,
		key: &str,
	) -> Result<Task> {
		self.runtime
			.store
			.create_task_in(&mut self.access.tx, workspace, input, subject, Some(key))
			.await
			.map_err(Into::into)
	}
	async fn finish(self, result: Result<()>) -> Result<()> {
		(*self.access)
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl HomeRepository for Repository {
	type Scope = Scope;
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	fn identity(&self) -> Option<ExecutionPrincipal> {
		match &self.actor {
			Actor::Subject(id) => Some(ExecutionPrincipal {
				tenant: id.tenant.clone(),
				subject: id.subject.clone(),
				credential_id: id.credential_id,
			}),
			Actor::Operator => None,
		}
	}
	async fn begin(&self) -> Result<Scope> {
		let Actor::Subject(identity) = &self.actor else {
			return Err(NativeError::Forbidden.into());
		};
		Ok(Scope {
			runtime: self.runtime.clone(),
			access: Box::new(Access::begin(&self.runtime.store, identity).await?),
		})
	}
	async fn description(&self, node: &str, grant: Uuid) -> Result<(Scope, Description)> {
		let (access, description) =
			crate::authorization::remote::description_lease(&self.runtime, node, grant).await?;
		Ok((
			Scope {
				runtime: self.runtime.clone(),
				access: Box::new(access),
			},
			description,
		))
	}
	async fn request<T: DeserializeOwned + Send>(
		&self,
		node: &str,
		path: &str,
		input: &Value,
	) -> Result<T> {
		crate::authorization::peer::authority_request(&self.runtime, node, path, input)
			.await
			.map_err(Into::into)
	}
	async fn prepare(&self, task: Uuid, input: PrepareInput) -> Result<Prepared> {
		crate::authorization::remote::RemoteGrants {
			runtime: self.runtime.clone(),
		}
		.prepare(self.actor.clone(), task, input)
		.await
		.map_err(Into::into)
	}
	async fn semantic_status(
		&self,
		grant: Uuid,
		binding: &Binding,
		reason: Option<Failure>,
	) -> Result<Status> {
		crate::semantic::remote::status::load(&self.runtime.store, grant, binding, reason)
			.await
			.map_err(Into::into)
	}
}

impl Scope {
	pub(crate) fn into_access(self) -> Access {
		*self.access
	}
}
