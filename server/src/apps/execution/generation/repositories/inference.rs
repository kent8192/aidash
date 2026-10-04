//! Local inference retains the original ORM methods and their transaction executor.
use crate::apps::execution::generation::models::{
	GenerationBudget, GenerationPolicy, GenerationRequest, GenerationUsage,
};
use crate::{Error, authorization::access::Access};
use aidash_application::{
	Result,
	ports::generation::inference::{
		GenerationInferenceAuthority, GenerationInferenceRepository, GenerationInferenceSession,
	},
};
use async_trait::async_trait;
use reinhardt::db::backends::{DatabaseConnection, TransactionExecutor};
use reinhardt::query::{
	Alias, Expr, Order, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

pub(crate) struct NativeInferenceAuthority<'a> {
	pub access: &'a mut Access,
}
#[async_trait]
impl GenerationInferenceAuthority for NativeInferenceAuthority<'_> {
	async fn requests(&mut self, node: &str) -> Result<Vec<Uuid>> {
		let access = &mut *self.access;
		let requests: Vec<Uuid> = {
			let query_bind_1 = &access.identity.tenant;
			let query_bind_2 = node;
			let query_bind_3 = &access.subjects;
			sqlx::query_scalar(&Query::select()
			.expr(SimpleExpr::from(Expr::col(Alias::new("id"))))
			.from(Alias::new("generation_requests"))
			.and_where(SimpleExpr::CustomWithExpr("(tenant = ? AND (? || '/agents/' || agent_id || '@' || agent_version) = ANY(?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), crate::database::text_array(query_bind_3.to_owned())]))
			.order_by_expr(SimpleExpr::from(Expr::col(Alias::new("id"))), Order::Asc)
			.to_string(PostgresQueryBuilder))
	.fetch_all(&mut **access.tx)
	.await.map_err(Error::from)?
		};
		Ok(requests)
	}
}
pub(crate) struct NativeInferenceRepository {
	pub database: DatabaseConnection,
	pub node_id: String,
}
struct Session {
	transaction: Box<dyn TransactionExecutor>,
}
#[async_trait]
impl GenerationInferenceRepository for NativeInferenceRepository {
	fn node_id(&self) -> &str {
		&self.node_id
	}
	async fn begin(&self) -> Result<Box<dyn GenerationInferenceSession>> {
		Ok(Box::new(Session {
			transaction: self.database.begin().await.map_err(Error::from)?,
		}))
	}
}
#[async_trait]
impl GenerationInferenceSession for Session {
	async fn charge(&mut self, request: Uuid, amount: i64) -> Result<()> {
		GenerationBudget::charge_inference(self.transaction.as_mut(), request, amount)
			.await
			.map_err(Into::into)
	}
	async fn reserve(
		&mut self,
		request: Uuid,
		attempt: Uuid,
		run: Uuid,
		amount: i64,
	) -> Result<()> {
		GenerationUsage::reserve(self.transaction.as_mut(), request, attempt, run, amount)
			.await
			.map_err(Into::into)
	}
	async fn refund(&mut self, request: Uuid, amount: i64) -> Result<()> {
		GenerationBudget::refund(self.transaction.as_mut(), request, amount)
			.await
			.map_err(Into::into)
	}
	async fn released_policy(&mut self, request: Uuid) -> Result<Option<(String, String)>> {
		GenerationRequest::released_policy(self.transaction.as_mut(), request)
			.await
			.map_err(Into::into)
	}
	async fn refund_allocated(&mut self, tenant: &str, policy: &str, amount: i64) -> Result<()> {
		GenerationPolicy::refund_allocated(self.transaction.as_mut(), tenant, policy, amount)
			.await
			.map_err(Into::into)
	}
	async fn report(&mut self, request: Uuid, attempt: Uuid, reported: Option<i64>) -> Result<()> {
		GenerationUsage::report(self.transaction.as_mut(), request, attempt, reported)
			.await
			.map_err(Into::into)
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.transaction.commit().await.map_err(Error::from)?;
		Ok(())
	}
}
