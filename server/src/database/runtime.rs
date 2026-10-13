//! The long-running server's PostgreSQL login and pool.
//!
//! Migrations run as the role that owns every table. Servers and workers log in
//! as a separate role that row-level security binds (#223), so they refuse a
//! login that would bypass it. Management commands, including the migration
//! Job's maintenance steps, keep Reinhardt's ordinary connection.
use crate::{Error, Result};
use reinhardt::db::backends::{DatabaseConnection, PostgresBackend};
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, ExprTrait, JoinType, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr, TableRef,
};
use sqlx::Executor as _;
use std::{sync::Arc, time::Duration};

/// Open the server's pool, refusing a login that row-level security would not bind.
///
/// The pool settings mirror Reinhardt's PostgreSQL defaults. Every connection
/// is reset when it returns to the pool, so a session-level setting made by
/// mistake cannot reach the next borrower; derived pools clone these options
/// and inherit the reset.
pub async fn connect(url: &str, max_connections: u32) -> Result<DatabaseConnection> {
	let pool = sqlx::postgres::PgPoolOptions::new()
		.max_connections(max_connections)
		.min_connections(1)
		.acquire_timeout(Duration::from_secs(10))
		.idle_timeout(Some(Duration::from_secs(10)))
		.max_lifetime(Some(Duration::from_secs(30 * 60)))
		.after_release(|connection, _| {
			Box::pin(async move {
				// RESET ALL, unlike DISCARD ALL, keeps SQLx's cached prepared
				// statements valid. A failed reset closes the connection.
				connection.execute("RESET ALL").await?;
				Ok(true)
			})
		})
		.connect(url)
		.await?;
	let connection = DatabaseConnection::new(Arc::new(PostgresBackend::new(pool)));
	require_unprivileged(&connection).await?;
	Ok(connection)
}

/// Refuse a superuser, a role with `BYPASSRLS`, or the owner of any relation
/// outside PostgreSQL's own schemas. Role attributes are never inherited
/// through membership, so the session's own role decides.
pub async fn require_unprivileged(connection: &DatabaseConnection) -> Result<()> {
	let role = Alias::new("r");
	let class = Alias::new("c");
	let namespace = Alias::new("n");
	let owned = Query::select()
		.expr(Expr::val(1))
		.from_as(Alias::new("pg_class"), class.clone())
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("pg_namespace"), namespace.clone()),
			SimpleExpr::from(Expr::col((namespace.clone(), Alias::new("oid"))))
				.eq(Expr::col((class.clone(), Alias::new("relnamespace")))),
		)
		.and_where(
			SimpleExpr::from(Expr::col((class, Alias::new("relowner"))))
				.eq(Expr::col((role.clone(), Alias::new("oid")))),
		)
		// Names starting with `pg_` are reserved for PostgreSQL, including the
		// TOAST and per-session temporary schemas.
		.and_where(
			SimpleExpr::from(Expr::col((namespace.clone(), Alias::new("nspname"))))
				.not_like("pg\\_%"),
		)
		.and_where(
			SimpleExpr::from(Expr::col((namespace, Alias::new("nspname"))))
				.ne("information_schema"),
		)
		.to_owned();
	let (sql, values) = Query::select()
		.expr_as(
			Expr::col((role.clone(), Alias::new("rolsuper"))),
			Alias::new("superuser"),
		)
		.expr_as(
			Expr::col((role.clone(), Alias::new("rolbypassrls"))),
			Alias::new("bypass"),
		)
		.expr_as(Expr::exists(owned), Alias::new("owner"))
		.from_as(Alias::new("pg_roles"), role.clone())
		.and_where(
			SimpleExpr::from(Expr::col((role, Alias::new("rolname"))))
				.eq(Expr::cust("current_user")),
		)
		.build(PostgresQueryBuilder);
	let row = connection.fetch_one(&sql, convert_values(values)).await?;
	let flag = |name: &str| {
		row.get::<bool>(name)
			.map_err(|error| Error::External(format!("database login check: {error}")))
	};
	let (superuser, bypass, owner) = (flag("superuser")?, flag("bypass")?, flag("owner")?);
	let reason = if superuser {
		"is a superuser"
	} else if bypass {
		"has BYPASSRLS"
	} else if owner {
		"owns database objects"
	} else {
		return Ok(());
	};
	Err(Error::Invalid(format!(
		"the server's database login {reason}, so row-level security would not apply to it; \
		 connect as the server role (aidash_server) and run migrations as the owner role \
		 (aidash_owner)"
	)))
}
