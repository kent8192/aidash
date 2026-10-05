//! Both original source queries run on the caller's existing credential transaction.
use crate::{Result as NativeResult, authorization::access::Access};
use aidash_application::{
	Result, authorization::visits::ReadVisit,
	ports::authorization::source::provenance::SemanticSourceScope,
};
use aidash_domain::semantic::{Source, mutations::Entry, remote::SourceRead};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) access: &'a mut Access,
}
#[async_trait]
impl SemanticSourceScope for Scope<'_> {
	fn source_visit(&self, grant: Uuid) -> Option<ReadVisit> {
		self.access.authority_read_visit("semantic", grant)
	}
	async fn disclosed_sources(&mut self, grant: Uuid) -> Result<Vec<SourceRead>> {
		let result: NativeResult<Vec<(Uuid, i64, String)>> = async {
			let sources: Vec<(Uuid, i64, String)> = {
				let query_bind_1 = grant;
				crate::database::native::query_as(
					&Query::select()
						.columns(["entry_id", "revision", "content_digest"].map(Alias::new))
						.from(Alias::new("semantic_remote_reads"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("grant_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								)),
						)
						.order_by(Alias::new("entry_id"), reinhardt::query::Order::Asc)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["entry_id", "revision", "content_digest"])
				.fetch_all(&mut **self.access.tx)
				.await?
			};

			Ok(sources)
		}
		.await;
		result
			.map(|rows| {
				rows.into_iter()
					.map(|(entry_id, revision, content_digest)| SourceRead {
						entry_id,
						revision,
						content_digest,
					})
					.collect()
			})
			.map_err(Into::into)
	}
	async fn disclosed_entry(&mut self, id: Uuid) -> Result<Option<Entry>> {
		let result: NativeResult<Option<crate::semantic::Entry>> = async {
			let entry: Option<crate::semantic::Entry> = {
				let query_bind_1 = id;
				crate::database::native::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("semantic_entries"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			};
			Ok(entry)
		}
		.await;
		result.map(|row| row.map(Into::into)).map_err(Into::into)
	}
	async fn source_permitted(&mut self, entry: &Entry) -> Result<bool> {
		crate::semantic::service::Lease::Inherited(&mut *self.access)
			.permits(&entry.clone().into(), "semantic.read")
			.await
			.map_err(Into::into)
	}
	async fn source_text(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>> {
		crate::semantic::service::Lease::Inherited(&mut *self.access)
			.source(workspace, source)
			.await
			.map_err(Into::into)
	}
}
