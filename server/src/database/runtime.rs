//! The long-running server's PostgreSQL login and pool.
//!
//! Migrations run as the role that owns every table. Servers and workers log in
//! as a separate role that row-level security binds (#223), so they refuse a
//! login that would bypass it. The migration Job's maintenance steps,
//! `memory-recovery` and `demo-data`, run under that owner and so open
//! [`connect_maintenance`], which accepts the owner but nothing else that
//! bypasses row-level security.
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
	let connection = open(url, max_connections).await?;
	require_unprivileged(&connection).await?;
	Ok(connection)
}

/// Open a pool for a maintenance command that the migration Job runs under the
/// migration owner (`aidash_owner`), such as `memory-recovery` and `demo-data`.
/// The owner is accepted
/// because `FORCE ROW LEVEL SECURITY` binds it like any other login: a
/// statement that scans a Tenant-owned table fails without a scope (`TS001`),
/// and every write through [`Pool`](super::native::Pool) opens one. The
/// command's code issues no DDL. Everything that would bypass the policy is
/// still refused: a superuser, `BYPASSRLS`, `REPLICATION`, a session that
/// switched roles, a role that can `SET ROLE` to one of those, and membership
/// in a role that reaches the server's files or programs.
pub async fn connect_maintenance(url: &str, max_connections: u32) -> Result<DatabaseConnection> {
	let connection = open(url, max_connections).await?;
	refuse_bypass(&connection, Login::Maintenance).await?;
	Ok(connection)
}

async fn open(url: &str, max_connections: u32) -> Result<DatabaseConnection> {
	let pool = sqlx::postgres::PgPoolOptions::new()
		.max_connections(max_connections)
		.min_connections(1)
		.acquire_timeout(Duration::from_secs(10))
		.idle_timeout(Some(Duration::from_secs(10)))
		.max_lifetime(Some(Duration::from_secs(30 * 60)))
		.after_release(|connection, _| {
			Box::pin(async move {
				// RESET ALL, unlike DISCARD ALL, keeps SQLx's cached prepared
				// statements valid. It skips the active role, so a borrower's
				// SET ROLE is reset explicitly. A failed reset closes the connection.
				connection.execute("RESET ROLE").await?;
				connection.execute("RESET ALL").await?;
				Ok(true)
			})
		})
		.connect(url)
		.await?;
	Ok(DatabaseConnection::new(Arc::new(PostgresBackend::new(
		pool,
	))))
}

/// Which logins a check accepts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Login {
	/// The server and its workers: no privilege beyond row-level DML.
	Server,
	/// A maintenance command under the migration owner, which owns objects but
	/// stays bound by `FORCE ROW LEVEL SECURITY`.
	Maintenance,
}

/// Whether the session's role is, or is a member of, the role in `owner`.
/// `MEMBER` covers both inherited privileges and `SET ROLE`, so ownership
/// held through an owning role counts like direct ownership. Reinhardt Query
/// has no builder for this function call.
fn member_of(owner: &str) -> SimpleExpr {
	SimpleExpr::from(Expr::cust(format!("pg_has_role(r.oid, {owner}, 'MEMBER')")))
}

/// Refuse a superuser, a role with `BYPASSRLS` or `REPLICATION`, the owner of
/// the current database, or the owner of any schema or relation outside
/// PostgreSQL's own schemas, directly or through membership in the owning role.
/// The database owner implicitly owns `public` through `pg_database_owner`, and
/// a schema owner can create and drop its objects. Role attributes are never
/// inherited through membership, so the session's own role decides those.
/// Functions count as owned objects. Privileges such as `CREATE`, `TRUNCATE`
/// and `TRIGGER` are checked for every role the login is a member of, because
/// `SET ROLE` reaches a role whose privileges are not inherited.
pub async fn require_unprivileged(connection: &DatabaseConnection) -> Result<()> {
	refuse_bypass(connection, Login::Server).await
}

async fn refuse_bypass(connection: &DatabaseConnection, login: Login) -> Result<()> {
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
		.and_where(member_of("c.relowner"))
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
	let function = Alias::new("f");
	let function_namespace = Alias::new("fns");
	// A function owner can replace its definition, such as a trigger guard's.
	let owned_function = Query::select()
		.expr(Expr::val(1))
		.from_as(Alias::new("pg_proc"), function.clone())
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("pg_namespace"), function_namespace.clone()),
			SimpleExpr::from(Expr::col((function_namespace.clone(), Alias::new("oid"))))
				.eq(Expr::col((function, Alias::new("pronamespace")))),
		)
		.and_where(member_of("f.proowner"))
		.and_where(
			SimpleExpr::from(Expr::col((
				function_namespace.clone(),
				Alias::new("nspname"),
			)))
			.not_like("pg\\_%"),
		)
		.and_where(
			SimpleExpr::from(Expr::col((function_namespace, Alias::new("nspname"))))
				.ne("information_schema"),
		)
		.to_owned();
	let schema = Alias::new("s");
	let owned_schema = Query::select()
		.expr(Expr::val(1))
		.from_as(Alias::new("pg_namespace"), schema.clone())
		.and_where(member_of("s.nspowner"))
		.and_where(
			SimpleExpr::from(Expr::col((schema.clone(), Alias::new("nspname")))).not_like("pg\\_%"),
		)
		.and_where(
			SimpleExpr::from(Expr::col((schema, Alias::new("nspname")))).ne("information_schema"),
		)
		.to_owned();
	let database = Alias::new("d");
	let owned_database = Query::select()
		.expr(Expr::val(1))
		.from_as(Alias::new("pg_database"), database.clone())
		.and_where(member_of("d.datdba"))
		.and_where(
			SimpleExpr::from(Expr::col((database, Alias::new("datname"))))
				.eq(Expr::cust("current_database()")),
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
		.expr_as(
			Expr::col((role.clone(), Alias::new("rolreplication"))),
			Alias::new("replication"),
		)
		.expr_as(Expr::exists(owned), Alias::new("owner"))
		.expr_as(Expr::exists(owned_function), Alias::new("function_owner"))
		.expr_as(Expr::exists(owned_schema), Alias::new("schema_owner"))
		.expr_as(Expr::exists(owned_database), Alias::new("database_owner"))
		// A startup `role` setting can make `current_user` look safe while the
		// authenticated login keeps its own privileges: check that login, and
		// refuse a session that already switched roles.
		.expr_as(Expr::cust("session_user <> current_user"), Alias::new("switched"))
		// Membership usable through SET ROLE carries a superuser's, a
		// BYPASSRLS role's or a REPLICATION role's attributes, which
		// inheritance never does.
		.expr_as(
			Expr::cust(
				"EXISTS (SELECT 1 FROM pg_roles b WHERE b.oid <> r.oid \
				 AND (b.rolsuper OR b.rolbypassrls OR b.rolreplication) \
				 AND pg_has_role(r.oid, b.oid, 'SET'))",
			),
			Alias::new("privileged_member"),
		)
		// These predefined roles read and write the server's files or run
		// programs as its operating-system user, which bypasses every
		// database permission. Their privileges are inherited, so any
		// membership counts.
		.expr_as(
			Expr::cust(
				"EXISTS (SELECT 1 FROM pg_roles b WHERE b.rolname IN \
				 ('pg_execute_server_program', 'pg_read_server_files', 'pg_write_server_files') \
				 AND pg_has_role(r.oid, b.oid, 'MEMBER'))",
			),
			Alias::new("server_access"),
		)
		// CREATE on the database or an application schema lets the login
		// change the schema without owning anything yet. `MEMBER` covers the
		// login itself and every role it inherits from or can SET ROLE to.
		.expr_as(
			Expr::cust(
				"EXISTS (SELECT 1 FROM pg_roles m WHERE pg_has_role(r.oid, m.oid, 'MEMBER') \
				 AND (has_database_privilege(m.oid, current_database(), 'CREATE') \
				 OR EXISTS (SELECT 1 FROM pg_namespace p WHERE p.nspname NOT LIKE 'pg\\_%' \
				 AND p.nspname <> 'information_schema' \
				 AND has_schema_privilege(m.oid, p.oid, 'CREATE'))))",
			),
			Alias::new("creator"),
		)
		// TRUNCATE skips row-level security, so it empties every tenant's rows.
		// TRIGGER lets the login replace a table's triggers, including the
		// guards migrations install, with any function it may execute.
		.expr_as(
			Expr::cust(
				"EXISTS (SELECT 1 FROM pg_roles m \
				 JOIN pg_class t ON pg_has_role(r.oid, m.oid, 'MEMBER') \
				 JOIN pg_namespace q ON q.oid = t.relnamespace \
				 WHERE t.relkind IN ('r', 'p', 'f') AND q.nspname NOT LIKE 'pg\\_%' \
				 AND q.nspname <> 'information_schema' \
				 AND has_table_privilege(m.oid, t.oid, 'TRUNCATE, TRIGGER'))",
			),
			Alias::new("table_privileged"),
		)
		.from_as(Alias::new("pg_roles"), role.clone())
		.and_where(
			SimpleExpr::from(Expr::col((role, Alias::new("rolname"))))
				.eq(Expr::cust("session_user")),
		)
		.build(PostgresQueryBuilder);
	let row = connection.fetch_one(&sql, convert_values(values)).await?;
	let flag = |name: &str| {
		row.get::<bool>(name)
			.map_err(|error| Error::External(format!("database login check: {error}")))
	};
	let reason = if flag("switched")? {
		"starts as another role"
	} else if flag("superuser")? {
		"is a superuser"
	} else if flag("bypass")? {
		"has BYPASSRLS"
	} else if flag("replication")? {
		"has REPLICATION"
	} else if flag("privileged_member")? {
		"can SET ROLE to a superuser, BYPASSRLS or REPLICATION role"
	} else if flag("server_access")? {
		"is a member of a role that reaches the server's files or programs"
	} else if login == Login::Maintenance {
		return Ok(());
	} else if flag("database_owner")? {
		"owns the database"
	} else if flag("schema_owner")? {
		"owns a schema"
	} else if flag("owner")? {
		"owns database objects"
	} else if flag("function_owner")? {
		"owns database functions"
	} else if flag("creator")? {
		"can create schemas or objects"
	} else if flag("table_privileged")? {
		"can truncate tables or create triggers on them"
	} else {
		return Ok(());
	};
	Err(Error::Invalid(format!(
		"the {} database login {reason}, so row-level security would not apply to it; \
		 connect as the server role (aidash_server) and run migrations as the owner role \
		 (aidash_owner)",
		match login {
			Login::Server => "server's",
			Login::Maintenance => "maintenance",
		}
	)))
}
