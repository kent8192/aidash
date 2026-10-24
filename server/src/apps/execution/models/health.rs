//! Check database availability without acquiring application locks.
use crate::{Result, database::native};
use aidash_application::tenancy::TenancyScope;
use reinhardt::query::{Expr, PostgresQueryBuilder, Query, QueryStatementBuilder};

/// The probe reads no table, so it opens the no-Tenant scope.
pub(crate) async fn probe(pool: &native::Pool) -> Result<()> {
	native::query(
		&Query::select()
			.expr(Expr::value(1_i32))
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool.in_scope(&TenancyScope::NoTenant))
	.await?;
	Ok(())
}
