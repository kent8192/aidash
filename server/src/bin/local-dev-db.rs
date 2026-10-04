//! Install and validate the local PostgreSQL JSON Schema extension.
use reinhardt::db::{backends::DatabaseConnection, orm::execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait, JoinType, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr, TableRef,
};
use std::{env, error::Error, io};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
	let database_urls: Vec<String> = env::args().skip(1).collect();
	if database_urls.is_empty() {
		return Err(io::Error::new(
			io::ErrorKind::InvalidInput,
			"usage: local-dev-db <database-url> [database-url...]",
		)
		.into());
	}
	for (index, database_url) in database_urls.iter().enumerate() {
		prepare_database(database_url).await?;
		println!("Prepared local Aidash database {}.", index + 1);
	}
	Ok(())
}

async fn prepare_database(database_url: &str) -> Result<(), Box<dyn Error>> {
	let database = DatabaseConnection::connect_postgres(database_url).await?;
	// Query has no CREATE EXTENSION builder. This PostgreSQL extension DDL
	// remains outside schema history and matches the production prerequisite.
	database
		.execute(
			"CREATE EXTENSION IF NOT EXISTS pg_jsonschema WITH SCHEMA public",
			vec![],
		)
		.await?;
	let (sql, values) = Query::select()
		.expr_as(
			Expr::col((Alias::new("n"), Alias::new("nspname"))),
			Alias::new("schema_name"),
		)
		.expr_as(
			Expr::col((Alias::new("e"), Alias::new("extversion"))),
			Alias::new("version"),
		)
		.from_as(Alias::new("pg_extension"), Alias::new("e"))
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("pg_namespace"), Alias::new("n")),
			SimpleExpr::from(Expr::col((Alias::new("e"), Alias::new("extnamespace"))))
				.eq(Expr::col((Alias::new("n"), Alias::new("oid")))),
		)
		.and_where(
			SimpleExpr::from(Expr::col((Alias::new("e"), Alias::new("extname"))))
				.eq("pg_jsonschema"),
		)
		.build(PostgresQueryBuilder);
	let row = database.fetch_one(&sql, convert_values(values)).await?;
	let schema: String = row.get("schema_name")?;
	let version: String = row.get("version")?;
	if schema != "public" || version != "0.3.4" {
		return Err(io::Error::new(
			io::ErrorKind::InvalidData,
			format!("expected pg_jsonschema 0.3.4 in public; found {schema}:{version}"),
		)
		.into());
	}
	Ok(())
}
