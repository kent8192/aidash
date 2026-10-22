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
	access: Held<'a>,
}
/// The authorization transaction a scope reads through: its own, which it
/// finishes, or a caller's, which the caller goes on using and finishes.
enum Held<'a> {
	Own(Box<Access>),
	Borrowed(&'a mut Access),
}
impl Held<'_> {
	fn get(&mut self) -> &mut Access {
		match self {
			Self::Own(access) => access,
			Self::Borrowed(access) => access,
		}
	}
	async fn finish<T>(self, result: NativeResult<T>) -> NativeResult<T> {
		match self {
			Self::Own(access) => access.finish(result).await,
			Self::Borrowed(_) => result,
		}
	}
}
#[async_trait]
impl RunDetailsRepository for Details<'_> {
	async fn begin(&self) -> Result<Box<dyn RunDetailsScope + '_>> {
		let mut access = Access::begin(&self.federation.store, self.identity).await?;
		access.wire = Some(crate::bootstrap::peer_transport(self.federation));
		Ok(Box::new(Scope {
			federation: self.federation,
			access: Held::Own(Box::new(access)),
		}))
	}
}
/// Whether the subject of the caller's authorization transaction may see the
/// Run, decided exactly as `GET /api/runs/{id}` decides it but inside that
/// transaction, so the locks the decision takes also cover the caller's own
/// changes. The transaction's policy context is left as it was.
pub(crate) async fn visible_within(
	federation: &Federation,
	access: &mut Access,
	run: Uuid,
) -> Result<RunInspection> {
	let context = access.context.clone();
	let wire = access
		.wire
		.replace(crate::bootstrap::peer_transport(federation));
	let mut scope = Scope {
		federation,
		access: Held::Borrowed(&mut *access),
	};
	let result = aidash_application::authorization::run_details::visible_in(&mut scope, run).await;
	scope.access.get().context = context;
	scope.access.get().wire = wire;
	result
}
#[async_trait]
impl RunDetailsScope for Scope<'_> {
	fn node_id(&self) -> &str {
		self.federation.config.deployment.as_str()
	}
	fn set_context(&mut self, context: Value) {
		self.access.get().context = context;
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
				.fetch_optional(&mut **self.access.get().tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.get().workspace(id).await.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.get()
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn run_visible(&mut self, run: &RunInspection) -> Result<bool> {
		self.access.get().run_visible(run).await.map_err(Into::into)
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
				.fetch_all(&mut **self.access.get().tx)
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
		crate::semantic::repositories::bindings::load(&mut **self.access.get().tx, run)
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
	async fn finish_visible(
		self: Box<Self>,
		result: Result<RunInspection>,
	) -> Result<RunInspection> {
		self.access
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
}
