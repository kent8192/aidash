//! One owned native transaction journals semantic dependencies before worker context disclosure.
use crate::{Result as NativeResult, store::Store};
use aidash_application::{Result, ports::semantic::run_context::RunSemanticJournal};
use async_trait::async_trait;
use reinhardt::query::{Expr, QueryStatementBuilder as _, SimpleExpr};

use uuid::Uuid;
pub(crate) struct Journal {
	tx: crate::database::native::Transaction,
	run: Uuid,
}
impl Journal {
	pub(crate) async fn begin(store: &Store, run: Uuid) -> NativeResult<Self> {
		Ok(Self {
			tx: crate::database::native::begin(&store.pool).await?,
			run,
		})
	}
}
#[async_trait]
impl RunSemanticJournal for Journal {
	async fn record(&mut self, entry: Uuid, revision: i64) -> Result<()> {
		let result: NativeResult<()> = async {
			let query_bind_1 = self.run;
			let query_bind_2 = entry;
			let query_bind_3 = revision;
			crate::database::native::query(&format!(
				"{} ON CONFLICT DO NOTHING",
				reinhardt::query::Query::insert()
					.into_table(reinhardt::query::Alias::new("semantic_run_reads"))
					.columns([
						reinhardt::query::Alias::new("run_id"),
						reinhardt::query::Alias::new("entry_id"),
						reinhardt::query::Alias::new("revision"),
					])
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
							.to_owned()
					)
					.to_owned()
					.to_string(reinhardt::query::PostgresQueryBuilder)
			))
			.execute(&mut *self.tx)
			.await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.tx.commit().await.map_err(|error| error.into())
	}
}
