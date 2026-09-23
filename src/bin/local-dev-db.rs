use sea_orm::sea_query::{Alias, Expr, JoinType, Query};
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
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
	let database = Database::connect(database_url).await?;
	// SeaQuery has no PostgreSQL CREATE EXTENSION builder; this is the one
	// unavoidable raw DDL statement in the local database preflight.
	database
		.execute(Statement::from_string(
			DbBackend::Postgres,
			"CREATE EXTENSION IF NOT EXISTS pg_jsonschema WITH SCHEMA public".to_owned(),
		))
		.await?;

	let query = Query::select()
		.expr_as(
			Expr::col((Alias::new("n"), Alias::new("nspname"))),
			Alias::new("schema_name"),
		)
		.expr_as(
			Expr::col((Alias::new("e"), Alias::new("extversion"))),
			Alias::new("version"),
		)
		.from_as(Alias::new("pg_extension"), Alias::new("e"))
		.join_as(
			JoinType::InnerJoin,
			Alias::new("pg_namespace"),
			Alias::new("n"),
			Expr::col((Alias::new("e"), Alias::new("extnamespace")))
				.equals((Alias::new("n"), Alias::new("oid"))),
		)
		.and_where(Expr::col((Alias::new("e"), Alias::new("extname"))).eq("pg_jsonschema"))
		.to_owned();
	let row = database
		.query_one(DbBackend::Postgres.build(&query))
		.await?
		.ok_or_else(|| {
			io::Error::new(
				io::ErrorKind::NotFound,
				"pg_jsonschema was not installed in the local database",
			)
		})?;
	let schema: String = row.try_get("", "schema_name")?;
	let version: String = row.try_get("", "version")?;
	if schema != "public" || version != "0.3.4" {
		return Err(io::Error::new(
			io::ErrorKind::InvalidData,
			format!("expected pg_jsonschema 0.3.4 in public; found {schema}:{version}"),
		)
		.into());
	}

	database.close().await?;
	Ok(())
}
