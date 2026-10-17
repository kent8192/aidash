//! Fixtures create Tenants under fixed names, which the Tenant creation API
//! never does: it assigns them (ADR 0050). These write the root record a
//! Tenant needs before its first policy, as the API's creation steps would.
use aidash_server::federation::Federation;

#[allow(dead_code)]
pub async fn tenant_root(f: &Federation, name: &str) {
	tenant_root_in(f.store.pool.driver(), name).await;
}

#[allow(dead_code)]
pub async fn tenant_root_in(pool: &sqlx::PgPool, name: &str) {
	use reinhardt::query::{
		Alias, Expr, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	let sql = Query::insert()
		.into_table(Alias::new("tenants"))
		.columns(["name", "label", "personal", "created_at", "created_by_kind"].map(Alias::new))
		.from_subquery(
			Query::select()
				.expr(Expr::cust("$1"))
				.expr(Expr::cust("$1"))
				.expr(Expr::cust("false"))
				.expr(Expr::cust("clock_timestamp()"))
				.expr(Expr::cust("'deployment_administrator'"))
				.to_owned(),
		)
		.on_conflict(
			OnConflict::column(Alias::new("name"))
				.do_nothing()
				.to_owned(),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&sql)
		.bind(name)
		.execute(pool)
		.await
		.expect("the Tenant root record is written");
}
