//! Provision the PostgreSQL roles for a self-hosted Deployment.
//! Keep the role and default-privilege definitions in sync with
//! `deploy/postgres/init-roles.sh`.
use reinhardt::db::{backends::DatabaseConnection, orm::execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use std::time::{Duration, Instant};

fn literal(value: &str) -> String {
	format!("E'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

fn connection_url(base: &reqwest::Url, role: &str, password: &str) -> Result<String, &'static str> {
	let mut url = base.clone();
	url.set_query(None);
	url.set_fragment(None);
	url.set_username(role).map_err(|_| "invalid database URL")?;
	url.set_password(Some(password))
		.map_err(|_| "invalid database URL")?;
	Ok(url.to_string())
}

pub struct Command;
#[async_trait::async_trait]
impl reinhardt::commands::CapabilityCommand for Command {
	fn cli(&self) -> clap::Command {
		clap::Command::new("setup-roles")
			.about("Create the PostgreSQL roles for a self-hosted Deployment")
			.arg(
				clap::Arg::new("superuser-url")
					.long("superuser-url")
					.value_name("URL"),
			)
			.arg(
				clap::Arg::new("owner-password")
					.long("owner-password")
					.value_name("PASSWORD"),
			)
			.arg(
				clap::Arg::new("server-password")
					.long("server-password")
					.value_name("PASSWORD"),
			)
	}
	fn requirements(
		&self,
		_: &clap::ArgMatches,
	) -> Vec<reinhardt::commands::CapabilityRequirement> {
		Vec::new()
	}
	// A superuser creates roles and extensions before any Tenant data exists,
	// as `local-dev-db` prepares a database; no tenancy scope applies.
	#[allow(clippy::disallowed_methods)]
	async fn execute(
		&self,
		args: &clap::ArgMatches,
		_: &reinhardt::commands::CapabilityContext,
	) -> reinhardt::commands::CommandResult<()> {
		use reinhardt::commands::CommandError;
		let fail = |message: &str| CommandError::ExecutionError(message.to_owned());
		let source = args
			.get_one::<String>("superuser-url")
			.cloned()
			.or_else(|| std::env::var("AIDASH_SUPERUSER_DATABASE_URL").ok())
			.ok_or_else(|| fail("provide --superuser-url or AIDASH_SUPERUSER_DATABASE_URL"))?;
		let url =
			reqwest::Url::parse(&source).map_err(|_| fail("invalid superuser database URL"))?;
		if !matches!(url.scheme(), "postgres" | "postgresql") || url.host_str().is_none() {
			return Err(fail("invalid PostgreSQL database URL"));
		}
		let connection = DatabaseConnection::connect_postgres(&source)
			.await
			.map_err(|_| fail("could not connect using the superuser database URL"))?;
		let mut transaction = connection
			.begin()
			.await
			.map_err(|_| fail("could not begin role setup"))?;
		let (sql, values) = Query::select()
			.expr_as(
				Expr::cust("current_database()"),
				Alias::new("database_name"),
			)
			.build(PostgresQueryBuilder);
		let row = transaction
			.fetch_one(&sql, convert_values(values))
			.await
			.map_err(|_| fail("could not read the database name"))?;
		let database: String = row
			.get("database_name")
			.map_err(|_| fail("could not read the database name"))?;
		let mut output = Vec::new();
		for (role, option) in [
			("aidash_owner", "owner-password"),
			("aidash_server", "server-password"),
		] {
			let (sql, values) = Query::select()
				.expr(Expr::val(1))
				.from(Alias::new("pg_roles"))
				.and_where(SimpleExpr::from(Expr::col(Alias::new("rolname"))).eq(role))
				.build(PostgresQueryBuilder);
			let exists = transaction
				.fetch_optional(&sql, convert_values(values))
				.await
				.map_err(|_| fail("could not inspect database roles"))?
				.is_some();
			let supplied = args.get_one::<String>(option);
			let password = supplied.cloned().or_else(|| {
				(!exists).then(|| {
					format!(
						"{}{}",
						uuid::Uuid::new_v4().simple(),
						uuid::Uuid::new_v4().simple()
					)
				})
			});
			if let Some(password) = &password {
				if password.contains('\0') {
					return Err(fail("passwords cannot contain NUL"));
				}
				// PostgreSQL role DDL is not supported by Reinhardt's schema builder.
				let sql = if exists {
					format!("ALTER ROLE {role} PASSWORD {}", literal(password))
				} else {
					format!(
						"CREATE ROLE {role} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS PASSWORD {}",
						literal(password)
					)
				};
				transaction
					.execute(&sql, vec![])
					.await
					.map_err(|_| fail("could not create or update a database role"))?;
				if exists {
					output.push(format!("{role} already exists; password updated."));
				}
				output.push(format!(
					"{} URL: {}",
					if role == "aidash_server" {
						"DATABASE_URL"
					} else {
						"Migration owner"
					},
					connection_url(&url, role, password).map_err(fail)?
				));
			} else {
				output.push(format!(
					"{role} already exists; password unchanged. Replace EXISTING_PASSWORD in {} URL: {}",
					if role == "aidash_server" {
						"DATABASE_URL"
					} else {
						"migration owner"
					},
					connection_url(&url, role, "EXISTING_PASSWORD").map_err(fail)?
				));
			}
		}
		// PostgreSQL role grants and database ownership DDL are not expressible by Reinhardt.
		let identifier = format!("\"{}\"", database.replace('"', "\"\""));
		let tables = concat!(
			"ALTER DEFAULT PRIVILEGES FOR ROLE aidash_owner ",
			"GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO aidash_server"
		);
		let sequences = concat!(
			"ALTER DEFAULT PRIVILEGES FOR ROLE aidash_owner ",
			"GRANT USAGE, SELECT ON SEQUENCES TO aidash_server"
		);
		for sql in [
			format!("ALTER DATABASE {identifier} OWNER TO aidash_owner"),
			tables.to_owned(),
			sequences.to_owned(),
		] {
			transaction.execute(&sql, vec![]).await.map_err(|_| {
				fail("could not configure database ownership or default privileges")
			})?;
		}
		transaction
			.commit()
			.await
			.map_err(|_| fail("could not commit role setup"))?;
		// Print credentials after commit, so an extension error cannot lose newly
		// generated passwords. A rerun retains them unless explicitly replaced.
		for line in output {
			println!("{line}");
		}
		// CREATE EXTENSION is PostgreSQL DDL not expressible by Reinhardt's schema builder.
		for (name, ddl) in [
			(
				"pg_jsonschema",
				"CREATE EXTENSION IF NOT EXISTS pg_jsonschema WITH SCHEMA public",
			),
			("vector", "CREATE EXTENSION IF NOT EXISTS vector"),
		] {
			connection.execute(ddl, vec![]).await.map_err(|_| fail(&format!(
				"could not create extension {name}; the PostgreSQL image or package must provide it (see deploy/postgres). local-dev-db and migrate check pg_jsonschema 0.3.4"
			)))?;
		}
		let started = Instant::now();
		loop {
			// A failed PGroonga initialization poisons its backend. Each attempt
			// owns a new connection pool and therefore uses a fresh backend.
			let fresh = DatabaseConnection::connect_postgres(&source)
				.await
				.map_err(|_| fail("could not connect using the superuser database URL"))?;
			let result = async {
				fresh
					.execute("CREATE EXTENSION IF NOT EXISTS pgroonga", vec![])
					.await
					.map_err(|error| error.to_string())?;
				let (sql, values) = Query::select()
					.expr(Expr::cust("pgroonga_command('io_flush')"))
					.build(PostgresQueryBuilder);
				fresh
					.fetch_one(&sql, convert_values(values))
					.await
					.map_err(|error| error.to_string())?;
				Ok::<(), String>(())
			}
			.await;
			match result {
				Ok(()) => break,
				Err(error)
					if error.contains("pgroonga_crash_safer is preparing")
						&& started.elapsed() < Duration::from_secs(30) =>
				{
					drop(fresh);
					tokio::time::sleep(Duration::from_millis(100)).await;
				}
				Err(_) => {
					return Err(fail(
						"could not create or initialize extension pgroonga; the PostgreSQL image or package must provide it (see deploy/postgres). local-dev-db and migrate check pg_jsonschema 0.3.4",
					));
				}
			}
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn quotes_and_encodes_credentials() {
		assert_eq!(literal("a'\\b"), "E'a''\\\\b'");
		let url = reqwest::Url::parse("postgres://admin:secret@localhost:5433/db").unwrap();
		assert_eq!(
			connection_url(&url, "aidash_server", "p@ss:/").unwrap(),
			"postgres://aidash_server:p%40ss%3A%2F@localhost:5433/db"
		);
	}
}
