//! Stable ancestor lookup and atomic compaction updates retain their original Query trees.
use crate::{Error, apps::identity::repositories::catalog::NativeCatalog, store::Store};
use aidash_application::{
	Result,
	ports::{
		catalog::CatalogScope,
		generation::{
			compaction::{
				GenerationCompactionAuthority, GenerationCompactionRepository,
				GenerationCompactionSession,
			},
			publication::GenerationLive,
		},
	},
};
use aidash_domain::{
	generation::{compaction::Attempt, requests::Request},
	registry::EntityRef,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{Expr, QueryStatementBuilder as _, SimpleExpr};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct NativeCompactionAuthority<'a> {
	pub catalog: NativeCatalog<'a>,
}
#[async_trait]
impl GenerationCompactionAuthority for NativeCompactionAuthority<'_> {
	async fn ancestors(&mut self, node: &str) -> Result<Vec<(Uuid, Value)>> {
		let access = &mut *self.catalog.0;
		let jobs: Vec<(Uuid, Value)> = {
			let query_bind_1 = access.identity.tenant.clone();
			let query_bind_2 = node;
			let query_bind_3 = access.subjects.clone();
			sqlx::query_as(&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col((
						reinhardt::query::Alias::new("r"),
						reinhardt::query::Alias::new("id"),
					)),
				))
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col((
						reinhardt::query::Alias::new("h"),
						reinhardt::query::Alias::new("spec"),
					)),
				))
				.from_as(
					reinhardt::query::Alias::new("generation_requests"),
					reinhardt::query::Alias::new("r"),
				).join(reinhardt::query::JoinType::InnerJoin, reinhardt::query::TableRef::table_alias(reinhardt::query::Alias::new("generation_policy_history"), reinhardt::query::Alias::new("h")), reinhardt::query::Expr::cust(
						"h.tenant = r.tenant AND h.policy_id = r.policy_id AND h.revision = r.policy_revision",
					))
				.and_where(SimpleExpr::CustomWithExpr("(r.tenant = ? AND (? || '/agents/' || r.agent_id || '@' || r.agent_version) = ANY(?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), crate::database::text_array(query_bind_3.to_owned())]))
				.order_by_expr(
					reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col((
						reinhardt::query::Alias::new("r"),
						reinhardt::query::Alias::new("id"),
					))),
					reinhardt::query::Order::Asc,
				)
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.fetch_all(&mut **access.tx)
		.await.map_err(Error::from)?
		};

		Ok(jobs)
	}
	fn live(&mut self) -> &mut dyn GenerationLive {
		self
	}
	fn catalog(&mut self) -> &mut dyn CatalogScope {
		&mut self.catalog
	}
}
#[async_trait]
impl GenerationLive for NativeCompactionAuthority<'_> {
	fn tenant(&self) -> &str {
		&self.catalog.0.identity.tenant
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn jobs(&mut self, node: &str, agent: &EntityRef) -> Result<Vec<Request>> {
		super::publication::NativeLive {
			access: &mut *self.catalog.0,
		}
		.jobs(node, agent)
		.await
	}
	async fn policy_enabled(&mut self, job: &Request) -> Result<bool> {
		super::publication::NativeLive {
			access: &mut *self.catalog.0,
		}
		.policy_enabled(job)
		.await
	}
}
pub(crate) struct NativeCompactionRepository {
	pub store: Store,
}
struct Session {
	transaction: sqlx::Transaction<'static, sqlx::Postgres>,
}
#[async_trait]
impl GenerationCompactionRepository for NativeCompactionRepository {
	fn node_id(&self) -> &str {
		&self.store.node_id
	}
	async fn begin(&self) -> Result<Box<dyn GenerationCompactionSession>> {
		Ok(Box::new(Session {
			transaction: self.store.pool.begin().await.map_err(Error::from)?,
		}))
	}
}
#[async_trait]
impl GenerationCompactionSession for Session {
	async fn charge(&mut self, id: Uuid) -> Result<bool> {
		let tx = &mut self.transaction;
		let reserved: Option<Uuid> = {
			let query_bind_1 = id;
			sqlx::query_scalar(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("generation_budgets"))
					.value_expr(
						reinhardt::query::Alias::new("compaction_calls"),
						reinhardt::query::Expr::cust("compaction_calls + 1"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(request_id = ? AND compaction_calls < compaction_call_limit)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.returning_exprs([reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::Alias::new("request_id")),
					)])
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await
			.map_err(Error::from)?
		};

		Ok(reserved.is_some())
	}
	async fn reserve(&mut self, id: Uuid, attempt: &Attempt) -> Result<()> {
		let tx = &mut self.transaction;
		{
			let query_bind_1 = id;
			let query_bind_2 = attempt.id;
			let query_bind_3 = attempt.run;
			let query_bind_4 = &attempt.provider.id;
			let query_bind_5 = &attempt.provider.version;
			let query_bind_6 = attempt.request_bytes;
			let query_bind_7 = attempt.questions;
			sqlx::query(
				&reinhardt::query::Query::insert()
					.into_table(reinhardt::query::Alias::new("generation_compaction_usage"))
					.columns([
						reinhardt::query::Alias::new("request_id"),
						reinhardt::query::Alias::new("attempt_id"),
						reinhardt::query::Alias::new("run_id"),
						reinhardt::query::Alias::new("provider_id"),
						reinhardt::query::Alias::new("provider_version"),
						reinhardt::query::Alias::new("request_bytes"),
						reinhardt::query::Alias::new("questions"),
					])
					.from_subquery(
						reinhardt::query::Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_5.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_6.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_7.to_owned()).into()],
							))
							.to_owned(),
					)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await
			.map_err(Error::from)?
		};
		Ok(())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.transaction.commit().await.map_err(Error::from)?;
		Ok(())
	}
}

pub(crate) mod remote;
