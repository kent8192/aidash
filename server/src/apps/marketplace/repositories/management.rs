//! Tenant-filtered and bounded candidate queries under the shared Access transaction.
use super::{definitions::NativeDefinitions, storage::documents_page};
use aidash_application::ports::marketplace::MarketplaceRead;
use aidash_domain::{marketplace::Version, registry::EntityRef};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait, JoinType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr, TableRef,
};
#[async_trait]
impl<Events: Send> MarketplaceRead for NativeDefinitions<'_, Events> {
	async fn version_page(
		&mut self,
		after: &str,
		limit: u64,
	) -> aidash_application::Result<Vec<(String, Version)>> {
		documents_page(&mut self.access.tx, "marketplace_versions", after, limit)
			.await
			.map_err(Into::into)
	}
	async fn source_references(
		&mut self,
		offset: usize,
		limit: u64,
	) -> aidash_application::Result<Vec<EntityRef>> {
		let candidates: Vec<(String, String)> = crate::database::native::query_as(
			&Query::select()
				.columns(
					["entry_id", "entry_version"].map(|name| (Alias::new("c"), Alias::new(name))),
				)
				.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
				.join(
					JoinType::InnerJoin,
					TableRef::table_alias(Alias::new("registry"), Alias::new("r")),
					Expr::cust("r.id=c.entry_id AND r.version=c.entry_version"),
				)
				.and_where(
					Expr::col((Alias::new("c"), Alias::new("tenant")))
						.eq(Expr::value(&self.access.identity.tenant)),
				)
				.and_where(Expr::cust("c.enabled"))
				.and_where(Expr::cust("r.metadata->>'kind'").is_in(["agent", "tool", "skill"]))
				.order_by((Alias::new("c"), Alias::new("entry_id")), Order::Asc)
				.order_by((Alias::new("c"), Alias::new("entry_version")), Order::Asc)
				.limit(limit)
				.offset(offset as u64)
				.to_string(PostgresQueryBuilder),
		)
		.columns(&["entry_id", "entry_version"])
		.fetch_all(&mut **self.access.tx)
		.await?;
		Ok(candidates
			.into_iter()
			.map(|(id, version)| EntityRef { id, version })
			.collect())
	}
	async fn installation_documents(
		&mut self,
	) -> aidash_application::Result<Vec<serde_json::Value>> {
		let documents: Vec<serde_json::Value> = {
			let query_bind_1 = &self.access.identity.tenant;
			crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("document"))
					.from(Alias::new("marketplace_installations"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.order_by(Alias::new("key"), Order::Asc)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_all(&mut **self.access.tx)
			.await?
		};
		Ok(documents)
	}
}
