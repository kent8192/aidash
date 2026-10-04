//! Check database availability without acquiring application locks.
use crate::Result;
use reinhardt::db::backends::DatabaseConnection;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{Expr, PostgresQueryBuilder, Query, QueryStatementBuilder};

pub(crate) async fn probe(connection: &DatabaseConnection) -> Result<()> {
	let (sql, values) = Query::select()
		.expr(Expr::value(1_i32))
		.build(PostgresQueryBuilder);
	connection.execute(&sql, convert_values(values)).await?;
	Ok(())
}
