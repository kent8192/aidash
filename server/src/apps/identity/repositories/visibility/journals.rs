//! Membership commits and source queries keep their existing transaction boundaries.
use super::Reads;
use crate::Result as NativeResult;
use crate::apps::{
	federation::remote::models::AuthorizationRemoteGrantRead,
	identity::models::AuthorizationRunRead,
};
use aidash_application::{
	Result,
	ports::authorization::journals::{GrantJournalScope, ReadJournalScope, ReadMembership},
};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait as _, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use uuid::Uuid;
#[async_trait]
impl ReadJournalScope for Reads<'_> {
	fn membership(&self) -> (Option<Uuid>, Option<Uuid>) {
		(self.access.read_run, self.access.read_grant)
	}
	async fn legacy_messages(
		&mut self,
		workspace: Option<Uuid>,
		sender: Option<&str>,
		content: Option<&str>,
	) -> Result<Vec<Uuid>> {
		let result: NativeResult<Vec<Uuid>> = async {
			let ids: Vec<Uuid> = {
				let query_bind_1 = workspace;
				let query_bind_2 = sender;
				let query_bind_3 = content;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("messages"))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_1.to_owned()).into()],
									)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"sender",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"content",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_3.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_all(&mut **self.access.tx)
				.await?
			};
			Ok(ids)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn record_sources(
		&mut self,
		membership: ReadMembership,
		workspace: Uuid,
		sources: &[(String, Uuid)],
	) -> Result<()> {
		let result: NativeResult<()> = async {
			let mut tx: Box<dyn TransactionExecutor> =
				Box::new(crate::database::native::begin(self.access.journal_pool()).await?);
			if let ReadMembership::RemoteGrant(scope) = membership {
				AuthorizationRemoteGrantRead::record_sources(
					tx.as_mut(),
					scope,
					workspace,
					sources,
				)
				.await?;
			} else if let ReadMembership::Run(scope) = membership {
				AuthorizationRunRead::record_sources(tx.as_mut(), scope, workspace, sources)
					.await?;
			}
			tx.commit().await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn record_registry(
		&mut self,
		run: Uuid,
		ids: Vec<String>,
		versions: Vec<String>,
	) -> Result<()> {
		let result: NativeResult<()> = async {
			{
				let query_bind_1 = run;
				let query_bind_2 = ids;
				let query_bind_3 = versions;
				crate::database::native::query(
					&Query::insert()
						.into_table(Alias::new("authorization_run_registry_reads"))
						.columns([
							Alias::new("run_id"),
							Alias::new("entry_id"),
							Alias::new("entry_version"),
						])
						.from_subquery(
							Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(unnest(?::text[]))".to_owned(),
									vec![crate::database::text_array(query_bind_2.to_owned())],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(unnest(?::text[]))".to_owned(),
									vec![crate::database::text_array(query_bind_3.to_owned())],
								))
								.to_owned(),
						)
						.on_conflict(
							OnConflict::columns(["run_id", "entry_id", "entry_version"])
								.do_nothing()
								.to_owned(),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(self.access.journal_pool())
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl GrantJournalScope for Reads<'_> {
	async fn remote_semantic_sources(&mut self, grant: Uuid) -> Result<()> {
		Box::pin(self.access.remote_semantic_sources(grant))
			.await
			.map_err(Into::into)
	}
	async fn source_visible(
		&mut self,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
		pending: &mut Vec<Uuid>,
	) -> Result<bool> {
		Box::pin(
			aidash_application::authorization::visibility::provenance::source_visible(
				self, workspace, kind, id, pending,
			),
		)
		.await
	}
	async fn run_reads(&mut self, run: Uuid) -> Result<bool> {
		Box::pin(self.access.run_reads_visible(run))
			.await
			.map_err(Into::into)
	}
	async fn sources(&mut self, grant: Uuid) -> Result<Vec<(Uuid, String, Uuid)>> {
		let result: NativeResult<Vec<(Uuid, String, Uuid)>> = async {
			let sources: Vec<(Uuid, String, Uuid)> = {
				let query_bind_1 = grant;
				crate::database::native::query_as(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new(
								"workspace_id",
							)),
						))
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new(
								"resource_kind",
							)),
						))
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new(
								"resource_id",
							)),
						))
						.from(reinhardt::query::Alias::new(
							"authorization_remote_grant_reads",
						))
						.and_where(SimpleExpr::CustomWithExpr(
							"(grant_id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.order_by_expr(
							reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
								reinhardt::query::Alias::new("resource_kind"),
							)),
							reinhardt::query::Order::Asc,
						)
						.order_by_expr(
							reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
								reinhardt::query::Alias::new("resource_id"),
							)),
							reinhardt::query::Order::Asc,
						)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.columns(&["workspace_id", "resource_kind", "resource_id"])
				.fetch_all(&mut **self.access.tx)
				.await?
			};
			Ok(sources)
		}
		.await;
		result.map_err(Into::into)
	}
}
