//! How many Nodes a Tenant has, for the Tier's Node limit.
use aidash_server::Result;
use async_trait::async_trait;
use std::collections::BTreeMap;

#[async_trait]
pub trait NodeCount: Send + Sync {
	async fn nodes(&self, tenant: &str) -> Result<u32>;
}

/// Current stored Nodes, including the default, for attachment and Tier changes.
pub struct StoredNodes(pub aidash_server::database::native::Pool);

#[async_trait]
impl NodeCount for StoredNodes {
	async fn nodes(&self, tenant: &str) -> Result<u32> {
		use reinhardt::query::{
			Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
		};
		let count: i64 = self
			.0
			.scoped(
				&aidash_application::tenancy::TenancyScope::tenant(tenant)?,
				async |tx| {
					aidash_server::database::native::query_scalar(
						&Query::select()
							.expr(reinhardt::query::Func::count(
								Expr::col(reinhardt::query::ColumnRef::Asterisk).into(),
							))
							.from(Alias::new("nodes"))
							.and_where(Expr::col(Alias::new("tenant")).eq(tenant))
							.to_string(PostgresQueryBuilder),
					)
					.scalar_one(&mut **tx)
					.await
				},
			)
			.await?;
		u32::try_from(count)
			.map_err(|_| aidash_server::Error::Invalid("Node count exceeds its range".into()))
	}
}

/// Fixed counts for tests, one Node for a Tenant not listed.
#[derive(Default)]
pub struct FixedNodes(pub BTreeMap<String, u32>);

#[async_trait]
impl NodeCount for FixedNodes {
	async fn nodes(&self, tenant: &str) -> Result<u32> {
		Ok(self.0.get(tenant).copied().unwrap_or(1))
	}
}
