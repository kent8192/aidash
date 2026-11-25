//! Retry committed Subnode retirement work outside the originating transaction.
use crate::{
	Result,
	apps::identity::services::creator_removal,
	database::native::{self, Transaction},
	store::Store,
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::{nodes::NodeName, registry::EntityRef};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use uuid::Uuid;

pub(super) async fn enqueue(tx: &mut Transaction, id: Uuid) -> Result<()> {
	native::query(
		&Query::update()
			.table(Alias::new("generation_requests"))
			.value_expr(
				Alias::new("subnode_cleanup_at"),
				Expr::cust("CURRENT_TIMESTAMP"),
			)
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	Ok(())
}

pub(in crate::apps::execution::generation::repositories) async fn reconcile(
	store: &Store,
) -> Result<()> {
	let deployment_scope = TenancyScope::Deployment(DeploymentScopeReason::GenerationReconcile);
	let columns = ["id", "tenant", "node", "agent_id", "agent_version"];
	let pending: Vec<(Uuid, String, String, String, String)> = native::query_as(
		&Query::select()
			.columns(columns.map(Alias::new))
			.from(Alias::new("generation_requests"))
			.and_where(Expr::col("subnode_cleanup_at").lte(Expr::cust("CURRENT_TIMESTAMP")))
			.order_by(Alias::new("subnode_cleanup_at"), Order::Asc)
			.order_by(Alias::new("id"), Order::Asc)
			.limit(16)
			.to_string(PostgresQueryBuilder),
	)
	.columns(&columns)
	.fetch_all(store.pool.in_scope(&deployment_scope))
	.await?;
	for (id, tenant, node, agent_id, agent_version) in pending {
		let node = store.deployment.node(NodeName::parse(&node)?);
		let agent = EntityRef {
			id: agent_id,
			version: agent_version,
		};
		// Any partial cleanup is safe to replay: the sweep only ends active Subnodes.
		let next = match creator_removal::end_retired_creators(store, &tenant, &node, &agent).await
		{
			Ok(()) => Expr::cust("NULL"),
			Err(error) => {
				tracing::warn!(%id, %error, "generated Agent Subnode cleanup failed");
				Expr::cust("CURRENT_TIMESTAMP + INTERVAL '5 seconds'")
			}
		};
		// Never requeue a row that another worker has already acknowledged.
		native::query(
			&Query::update()
				.table(Alias::new("generation_requests"))
				.value_expr(Alias::new("subnode_cleanup_at"), next)
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.and_where(Expr::col("subnode_cleanup_at").is_not_null())
				.to_string(PostgresQueryBuilder),
		)
		.execute(store.pool.in_scope(&TenancyScope::tenant(&tenant)?))
		.await?;
	}
	Ok(())
}
