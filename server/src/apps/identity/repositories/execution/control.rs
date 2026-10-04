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
	RawRun, RunControlAction, RunInspection,
	identity::execution::{ExecutionGrant, ExecutionPrincipal},
	policy::Resource,
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
		Ok(Box::new(Scope {
			federation: self.federation,
			access: Access::begin(&self.federation.store, self.identity).await?,
		}))
	}
	fn notify(&self) {
		self.federation.notify.notify_waiters();
	}
}
#[async_trait]
impl RunControlScope for Scope<'_> {
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
	async fn lock_execution_grant(&mut self, id: Uuid) -> Result<Option<ExecutionGrant>> {
		let result: NativeResult<Option<Grant>> = async {
			Ok({
				let query_bind_1 = id;
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
			let query_bind_1 = id;
			let query_bind_2 = credential;
			sqlx::query(
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
	async fn control_run(&mut self, id: Uuid, action: RunControlAction) -> Result<RunInspection> {
		self.federation
			.store
			.control_in(&mut self.access.tx, id, action)
			.await
			.map_err(Into::into)
	}
	async fn finish(self: Box<Self>, result: Result<RunInspection>) -> Result<RunInspection> {
		self.access
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
}
