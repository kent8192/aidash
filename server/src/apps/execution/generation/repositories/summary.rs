//! Atomic Summary Stage call charges and durable per-attempt usage records.
use crate::store::Store;
use aidash_application::{
	Result,
	ports::generation::summary::{GenerationSummaryRepository, GenerationSummarySession},
};
use aidash_domain::generation::summary::Attempt;
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder as _, SimpleExpr,
};
use uuid::Uuid;

pub(crate) struct NativeSummaryRepository {
	pub store: Store,
}
struct Session {
	transaction: crate::database::native::Transaction,
}
#[async_trait]
impl GenerationSummaryRepository for NativeSummaryRepository {
	fn node_id(&self) -> &str {
		&self.store.node_id
	}
	async fn begin(&self) -> Result<Box<dyn GenerationSummarySession>> {
		Ok(Box::new(Session {
			transaction: crate::database::native::begin(&self.store.pool).await?,
		}))
	}
}
#[async_trait]
impl GenerationSummarySession for Session {
	async fn charge(&mut self, id: Uuid) -> Result<bool> {
		let tx = &mut self.transaction;
		let reserved: Option<Uuid> = crate::database::native::query_scalar(
			&Query::update()
				.table(Alias::new("generation_budgets"))
				.value_expr(Alias::new("summary_calls"), Expr::cust("summary_calls + 1"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(request_id = ? AND summary_calls < summary_call_limit)".to_owned(),
					vec![Expr::value(id).into()],
				))
				.returning_exprs([SimpleExpr::from(Expr::col(Alias::new("request_id")))])
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **tx)
		.await?;
		Ok(reserved.is_some())
	}
	async fn reserve(&mut self, id: Uuid, attempt: &Attempt) -> Result<()> {
		let tx = &mut self.transaction;
		crate::database::native::query(
			&Query::insert()
				.into_table(Alias::new("generation_summary_usage"))
				.columns([
					Alias::new("request_id"),
					Alias::new("attempt_id"),
					Alias::new("run_id"),
					Alias::new("provider_id"),
					Alias::new("provider_version"),
					Alias::new("definition_digest"),
					Alias::new("request_bytes"),
				])
				.values_panic([
					Expr::value(id),
					Expr::value(attempt.id),
					Expr::value(attempt.run),
					Expr::value(attempt.provider.id.clone()),
					Expr::value(attempt.provider.version.clone()),
					Expr::value(attempt.definition_digest.clone()),
					Expr::value(attempt.request_bytes),
				])
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?;
		Ok(())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.transaction.commit().await?;
		Ok(())
	}
}
