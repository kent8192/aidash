//! Live generated-ancestor authority for Summary Stage requests.
use crate::apps::identity::repositories::catalog::NativeCatalog;
use aidash_application::{
	Result,
	ports::{
		catalog::CatalogScope,
		generation::{publication::GenerationLive, summary::GenerationSummaryAuthority},
	},
};
use aidash_domain::{generation::requests::Request, registry::EntityRef};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{Expr, QueryStatementBuilder as _, SimpleExpr};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct NativeSummaryAuthority<'a> {
	pub catalog: NativeCatalog<'a>,
}
#[async_trait]
impl GenerationSummaryAuthority for NativeSummaryAuthority<'_> {
	async fn ancestors(&mut self, node: &str) -> Result<Vec<(Uuid, Value)>> {
		let access = &mut *self.catalog.0;
		let jobs: Vec<(Uuid, Value)> = {
			let query_bind_1 = access.identity.tenant.clone();
			let query_bind_2 = node;
			let query_bind_3 = access.subjects.clone();
			crate::database::native::query_as(&reinhardt::query::Query::select()
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
				.and_where(SimpleExpr::CustomWithExpr("(r.tenant = ? AND (? || '/' || r.node || '/agents/' || r.agent_id || '@' || r.agent_version) = ANY(?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), crate::database::text_array(query_bind_3.to_owned())]))
				.order_by_expr(
					reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col((
						reinhardt::query::Alias::new("r"),
						reinhardt::query::Alias::new("id"),
					))),
					reinhardt::query::Order::Asc,
				)
				.to_string(reinhardt::query::PostgresQueryBuilder)).columns(&["id", "spec"])
		.fetch_all(&mut **access.tx)
		.await?
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
impl GenerationLive for NativeSummaryAuthority<'_> {
	fn tenant(&self) -> &str {
		&self.catalog.0.identity.tenant
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn jobs(&mut self, node: &str, agent: &EntityRef) -> Result<Vec<Request>> {
		super::super::publication::NativeLive {
			access: &mut *self.catalog.0,
		}
		.jobs(node, agent)
		.await
	}
	async fn policy_enabled(&mut self, job: &Request) -> Result<bool> {
		super::super::publication::NativeLive {
			access: &mut *self.catalog.0,
		}
		.policy_enabled(job)
		.await
	}
}
