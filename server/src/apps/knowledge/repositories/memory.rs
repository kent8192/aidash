//! Native query adapters keep memory writes and read checks on the inherited lease.
use super::mutations::Entries;
use crate::{Result as NativeResult, apps::knowledge::services::core as native};
use aidash_application::{
	Result,
	ports::semantic::{memory::SemanticMemoryReadSession, mutations::SemanticEntriesSession},
};
use aidash_domain::semantic::{Source, mutations::Entry};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Expr, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use uuid::Uuid;

pub(crate) struct Memory<'a, 'scope> {
	pub entries: Entries<'a, 'scope>,
}

#[async_trait]
impl SemanticMemoryReadSession for Memory<'_, '_> {
	async fn native_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		super::memory_reads::visible(self.entries.lease, run)
			.await
			.map_err(Into::into)
	}
	async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool> {
		self.entries.permits(entry, action).await
	}
	async fn source(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>> {
		self.entries.source(workspace, source).await
	}

	async fn dependencies(&mut self, run: Uuid) -> Result<Vec<(Uuid, i64)>> {
		let result: NativeResult<Vec<(Uuid, i64)>> = async {
			let dependencies: Vec<(Uuid, i64)> = {
				let query_bind_1 = run;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(Alias::new("entry_id"))))
						.expr(SimpleExpr::from(Expr::col(Alias::new("revision"))))
						.from(Alias::new("semantic_run_reads"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(run_id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.order_by_expr(
							SimpleExpr::from(Expr::col(Alias::new("entry_id"))),
							Order::Asc,
						)
						.order_by_expr(
							SimpleExpr::from(Expr::col(Alias::new("revision"))),
							Order::Asc,
						)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["entry_id", "revision"])
				.fetch_all(&mut **self.entries.lease.tx())
				.await?
			};
			Ok(dependencies)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn entry(&mut self, id: Uuid) -> Result<Option<Entry>> {
		let result: NativeResult<native::Entry> = async {
			let entry: native::Entry = {
				let query_bind_1 = id;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **self.entries.lease.tx())
				.await?
			};
			Ok(entry)
		}
		.await;
		result.map(|entry| Some(entry.into())).map_err(Into::into)
	}
	async fn point_digest(&mut self, point: Uuid) -> Result<Option<String>> {
		let result: NativeResult<Option<String>> = async {
			let digest: Option<String> = {
				let query_bind_1 = point;
				crate::database::native::query_scalar(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(Alias::new("content_digest"))))
						.from(Alias::new("semantic_points"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&mut **self.entries.lease.tx())
				.await?
			};
			Ok(digest)
		}
		.await;
		result.map_err(Into::into)
	}
}
