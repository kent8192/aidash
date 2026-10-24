//! Current-source reads adapt a leased native transaction to retrieval ports.
use super::mutations::Entries;
use crate::apps::knowledge::services::core::{self as native, service};
use crate::{Result as NativeResult, store::Store};
use aidash_application::{
	Result,
	ports::semantic::{mutations::SemanticEntriesSession, retrieval::SemanticRetrievalSession},
};
use aidash_domain::semantic::{
	EmbeddingConfig, Source,
	mutations::{Entry, Index},
	retrieval::Search,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Expr, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use uuid::Uuid;

pub(crate) struct Retrieval<'a, 'scope> {
	pub entries: Entries<'a, 'scope>,
	pub store: Store,
}
#[async_trait]
impl SemanticRetrievalSession for Retrieval<'_, '_> {
	async fn workspace(&mut self, workspace: Uuid, action: &str) -> Result<()> {
		self.entries.workspace(workspace, action).await
	}
	async fn index(&mut self, workspace: Uuid) -> Result<Index> {
		self.entries.index(workspace, false).await
	}
	async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool> {
		self.entries.permits(entry, action).await
	}
	async fn source(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>> {
		self.entries.source(workspace, source).await
	}
	async fn configured(&mut self, workspace: Uuid) -> Result<Option<Index>> {
		let lease = &mut *self.entries.lease;
		let result: NativeResult<Option<native::Index>> = async {
			let configured: Option<native::Index> = {
				let query_bind_1 = workspace;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("semantic_indexes"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(workspace_id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **lease.tx())
				.await?
			};

			Ok(configured)
		}
		.await;
		result
			.map(|index| index.map(Into::into))
			.map_err(Into::into)
	}
	async fn candidates(&mut self, workspace: Uuid) -> Result<Vec<Entry>> {
		let lease = &mut *self.entries.lease;
		let result: NativeResult<Vec<native::Entry>> = async {
			let rows: Vec<native::Entry> = {
				let query_bind_1 = workspace;
				crate::database::native::query_as(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
						))
						.from(reinhardt::query::Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(workspace_id = ? AND NOT deleted)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.order_by_expr(
							reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
								reinhardt::query::Alias::new("id"),
							)),
							reinhardt::query::Order::Asc,
						)
						.lock(reinhardt::query::LockType::Share)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_all(&mut **lease.tx())
				.await?
			};

			Ok(rows)
		}
		.await;
		result
			.map(|rows| rows.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
	async fn digest(&mut self, point: Uuid) -> Result<String> {
		let lease = &mut *self.entries.lease;
		let result: NativeResult<String> = async {
			let digest: String = {
				let query_bind_1 = point;
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::Expr::cust("COALESCE(content_digest, '')"))
						.from(reinhardt::query::Alias::new("semantic_points"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ? AND NOT retired)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_optional(&mut **lease.tx())
				.await?
			}
			.unwrap_or_default();

			Ok(digest)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn embed(
		&mut self,
		workspace: Uuid,
		config: &EmbeddingConfig,
		text: &str,
		run: Option<Uuid>,
	) -> Result<Vec<f32>> {
		service::embed(
			&self.store,
			self.entries.lease,
			workspace,
			config,
			text,
			crate::generation::embedding::Origin::Query(run),
		)
		.await
		.map_err(Into::into)
	}
}
impl From<&native::Search> for Search {
	fn from(input: &native::Search) -> Self {
		Self {
			query: input.query.clone(),
			agent: input.agent.clone(),
			metadata: input.metadata.clone(),
			limit: input.limit,
			max_tokens: input.max_tokens,
		}
	}
}
