//! Exact Run/grant queries retain the original subject transaction and resume row lock.
use crate::{
	Result as NativeResult,
	apps::identity::{
		repositories::execution::Grant,
		services::{access::Access, identity::SubjectIdentity},
	},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::authorization::runs::{RunControlRepository, RunControlScope},
};
use aidash_domain::{
	RawRun, RunControlAction, RunInspection, RunMetadata,
	identity::execution::{ExecutionGrant, ExecutionPrincipal},
	policy::Resource,
	run_state::RunReadsBlock,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Controls<'a> {
	pub(crate) federation: &'a Federation,
	pub(crate) identity: &'a SubjectIdentity,
}
struct Scope<'a> {
	federation: &'a Federation,
	access: Access,
}
#[async_trait]
impl RunControlRepository for Controls<'_> {
	async fn begin(&self) -> Result<Box<dyn RunControlScope + '_>> {
		let access = Access::begin(&self.federation.store, self.identity).await?;
		Ok(Box::new(Scope {
			federation: self.federation,
			access,
		}))
	}
	fn notify(&self) {
		self.federation.notify.notify_waiters();
	}
}
#[async_trait]
impl RunControlScope for Scope<'_> {
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
	fn set_context(&mut self, context: Value) {
		self.access.context = context;
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn run(&mut self, id: Uuid) -> Result<Option<RawRun>> {
		let result: NativeResult<Option<RawRun>> = async {
			Ok({
				let query_bind_1 = id;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("runs"))
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
				.fetch_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
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
	async fn run_visible(&mut self, run: &RunInspection) -> Result<bool> {
		self.access.run_visible(run).await.map_err(Into::into)
	}
	async fn run_reads_block(&mut self, run: &RunInspection) -> Result<Option<RunReadsBlock>> {
		self.access.run_reads_block(run).await.map_err(Into::into)
	}
	async fn run_reads_stop(&mut self, run: &RunInspection) -> Result<Option<RunReadsBlock>> {
		self.access.run_reads_stop(run).await.map_err(Into::into)
	}
	async fn use_block(&mut self, run: &RunInspection) -> Result<Option<RunReadsBlock>> {
		let metadata = RunMetadata::from(run);
		// The Run uses its definitions as its own subject chain, not as the person who resumes
		// it: a dependency restricted to the Agent that uses it, or an Agent version restricted to
		// its delegating parent, would otherwise refuse the person (ADR 0095). The grant is
		// already locked by the resume's ownership check.
		let grant = self
			.lock_execution_grant(run.id)
			.await?
			.ok_or(aidash_application::Error::Forbidden)?;
		let person = std::mem::replace(&mut self.access.subjects, grant.subject_chain);
		let blocked = super::guard::use_block(&mut self.access, &metadata, true).await;
		// Who may be told its name is decided as the person, so restore their subjects first.
		self.access.subjects = person;
		let Some(reference) = blocked? else {
			return Ok(None);
		};
		let entry = match crate::apps::registry::models::Definition::read_in(
			&mut **self.access.tx,
			&reference.id,
			&reference.version,
		)
		.await
		{
			Ok(entry) => Some(entry),
			Err(crate::Error::NotFound(_)) => None,
			Err(error) => return Err(error.into()),
		};
		aidash_application::authorization::catalog::use_block(
			&mut crate::bootstrap::catalog_scope(&mut self.access),
			&reference,
			entry.as_ref(),
		)
		.await
		.map(Some)
	}
	async fn lock_execution_grant(&mut self, id: Uuid) -> Result<Option<ExecutionGrant>> {
		let result: NativeResult<Option<Grant>> = async {
			Ok({
				let query_bind_1 = id;
				crate::database::native::query_as(
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
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result
			.map(|grant| grant.map(Into::into))
			.map_err(Into::into)
	}
	async fn update_credential(&mut self, id: Uuid, credential: Uuid) -> Result<()> {
		let result: NativeResult<()> = async {
			let policy = self.access.tx.pool().dashboard_policy();
			crate::apps::identity::models::DashboardExecutionOrigin::renew_manual(
				&mut **self.access.tx,
				id,
				&self.access.identity,
				policy.as_ref(),
			)
			.await?;
			let query_bind_1 = id;
			let query_bind_2 = credential;
			crate::database::native::query(
				&Query::update()
					.table(Alias::new("authorization_execution"))
					.value_expr(
						Alias::new("credential_id"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
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
			.execute(&mut **self.access.tx)
			.await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn resume_run(&mut self, id: Uuid) -> Result<RunInspection> {
		self.federation
			.store
			.control_in(&mut self.access.tx, id, RunControlAction::Resume)
			.await
			.map_err(Into::into)
	}
	async fn decline_call(&mut self, id: Uuid, call: &str) -> Result<RunInspection> {
		let by = self.access.identity.subject.clone();
		self.federation
			.store
			.decline_call_in(&mut self.access.tx, id, call, &by)
			.await
			.map(|declined| declined.run)
			.map_err(Into::into)
	}
	async fn finish(self: Box<Self>, result: Result<RunInspection>) -> Result<RunInspection> {
		let run = self.access.finish(result.map_err(Into::into)).await?;
		self.federation
			.store
			.run_control_committed(run.id, run.control);
		Ok(run)
	}
}
