//! Native Run inspection preserves its exact ordered SQL projection and borrowed transaction.
use crate::{
	Result as NativeResult,
	apps::identity::services::{access::Access, identity::SubjectIdentity},
	federation::Federation,
	store::Invocation,
};
use aidash_application::{
	Result,
	ports::authorization::run_details::{RunDetails, RunDetailsRepository, RunDetailsScope},
};
use aidash_domain::{
	RawRun, RunInspection, RunMetadata, invocation::InvocationSummary, policy::Resource,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Details<'a> {
	pub(crate) federation: &'a Federation,
	pub(crate) identity: &'a SubjectIdentity,
}
struct Scope<'a> {
	federation: &'a Federation,
	access: Access,
}
#[async_trait]
impl RunDetailsRepository for Details<'_> {
	async fn begin(&self) -> Result<Box<dyn RunDetailsScope + '_>> {
		Ok(Box::new(Scope {
			federation: self.federation,
			access: Access::begin(&self.federation.store, self.identity).await?,
		}))
	}
}
#[async_trait]
impl RunDetailsScope for Scope<'_> {
	fn node_id(&self) -> &str {
		&self.federation.config.node_id
	}
	fn set_context(&mut self, context: Value) {
		self.access.context = context;
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
	async fn invocations(&mut self, id: Uuid, offset: u64) -> Result<Vec<InvocationSummary>> {
		let result: NativeResult<Vec<Invocation>> = async {
			Ok({
				let query_bind_1 = id;
				crate::database::native::query_as(
					&crate::store::invocation_summary(None)
						.from(Alias::new("invocations"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("run_id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.order_by(Alias::new("created_at"), Order::Asc)
						.order_by(Alias::new("idempotency_key"), Order::Asc)
						.limit(100)
						.offset(offset)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result
			.map(|rows| rows.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
	async fn memory(
		&mut self,
		run: &RunMetadata,
	) -> Result<Option<aidash_domain::memory::Binding>> {
		crate::semantic::repositories::bindings::load(&mut **self.access.tx, run)
			.await
			.map_err(Into::into)
	}
	async fn media_input_routes(&mut self, run: &RunMetadata) -> Result<Vec<Vec<String>>> {
		self.federation
			.run_media_input_routes(run)
			.await
			.map_err(Into::into)
	}
	async fn finish(self: Box<Self>, result: Result<RunDetails>) -> Result<RunDetails> {
		self.access
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
}
