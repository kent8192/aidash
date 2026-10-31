//! Opening a transaction in its tenancy scope (#263).
//!
//! Right after `BEGIN`, one statement writes the scope into transaction-local
//! settings that the row-level policy functions (#264) and the activity
//! observer share:
//!
//! - `aidash.scope` is always set: `tenant`, `deployment` or `none`.
//! - `aidash.tenant` holds the Tenant name in a Tenant scope and is empty
//!   otherwise.
//! - `application_name` is `deployment:<label>` in a Deployment Scope.
//! - A Deployment Scope whose reason only reads also sets
//!   `transaction_read_only`. Switching a transaction to read-only is allowed
//!   after its first statement, so this costs no extra round trip.
//!
//! Only this module writes `aidash.scope` and `aidash.tenant`; a test enforces
//! it. `server/migrations/README.md` documents the contract.
use crate::Result;
use aidash_application::tenancy::TenancyScope;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use uuid::Uuid;

/// The setting that names the kind of scope.
pub const SCOPE_SETTING: &str = "aidash.scope";
/// The setting that names the Tenant of a Tenant scope.
pub const TENANT_SETTING: &str = "aidash.tenant";

/// The value of `aidash.scope` for a scope.
pub fn scope_value(scope: &TenancyScope) -> &'static str {
	match scope {
		TenancyScope::Tenant(_) => "tenant",
		TenancyScope::Deployment(_) => "deployment",
		TenancyScope::NoTenant => "none",
	}
}

fn set_config(name: &str, value: String) -> SimpleExpr {
	SimpleExpr::FunctionCall(
		"set_config".into_iden(),
		vec![
			Expr::value(name.to_owned()).into(),
			Expr::value(value).into(),
			Expr::value(true).into(),
		],
	)
}

/// The Tenant of the transaction's scope, as the value of an `INSERT`'s
/// `tenant` column. The row-level policy refuses any other value. A Deployment
/// Scope has no Tenant, so the expression is NULL there and the column's
/// `NOT NULL` refuses the row: a writer in that scope names its Tenant, or
/// takes it from a parent row with [`tenant_of`].
pub(crate) fn tenant() -> Expr {
	Expr::cust("(SELECT public.aidash_scope_tenant())")
}

/// The Tenant of the row of `table` whose `column` is `key`: a child row takes
/// its parent's.
pub(crate) fn tenant_of(table: &str, column: &str, key: Uuid) -> SimpleExpr {
	SimpleExpr::CustomWithExpr(
		format!("(SELECT tenant FROM {table} WHERE {column} = ?)"),
		vec![Expr::value(key).into()],
	)
}

/// The Node of a Workspace, as the value of an `INSERT`'s `node` column: a
/// Task, Run, event or generation request belongs to its Workspace's Node.
pub(crate) fn workspace_node(workspace: Uuid) -> SimpleExpr {
	SimpleExpr::CustomWithExpr(
		"(SELECT node FROM workspaces WHERE id = ?)".to_owned(),
		vec![Expr::value(workspace).into()],
	)
}

/// The address of `node` in `deployment`, as the value of a `home_node` column
/// for work that runs where its Task is.
pub(crate) fn node_address(deployment: &str, node: SimpleExpr) -> SimpleExpr {
	SimpleExpr::CustomWithExpr(
		"(? || '/' || ?)".to_owned(),
		vec![Expr::value(deployment.to_owned()).into(), node],
	)
}

/// A `home` (or `home_node`) column holding the address of any Node of
/// `deployment`.
pub(crate) fn in_deployment(column: &str, deployment: &str) -> SimpleExpr {
	SimpleExpr::CustomWithExpr(
		format!("(starts_with({column}, ?))"),
		vec![Expr::value(format!("{deployment}/")).into()],
	)
}

/// A Run (or generation request) of table alias `table` whose Home is its own
/// Node in `deployment`.
pub(crate) fn local_home(table: &str, deployment: &str) -> SimpleExpr {
	SimpleExpr::CustomWithExpr(
		format!("({table}.home_node = ? || '/' || {table}.node)"),
		vec![Expr::value(deployment.to_owned()).into()],
	)
}

/// The Tenant of the row of `table` whose `column` is `key`, for a child row
/// built in Rust.
pub(crate) async fn parent_tenant(
	tx: &mut dyn TransactionExecutor,
	table: &str,
	column: &str,
	key: Uuid,
) -> Result<String> {
	let (sql, values) = Query::select()
		.column(Alias::new("tenant"))
		.from(Alias::new(table))
		.and_where(Expr::col(Alias::new(column)).eq(Expr::value(key)))
		.build(PostgresQueryBuilder);
	let row = tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.ok_or_else(|| crate::Error::NotFound(table.into()))?;
	Ok(row
		.get::<String>("tenant")
		.map_err(reinhardt::core::exception::Error::from)?)
}

/// The Tenant of an event: its Workspace's, or the transaction scope's when it
/// concerns no Workspace, such as a Marketplace installation.
pub(crate) fn event_tenant(workspace: Option<Uuid>) -> SimpleExpr {
	match workspace {
		Some(workspace) => tenant_of("workspaces", "id", workspace),
		None => tenant().into(),
	}
}

/// A SQLx pool whose sessions hold the test-fixture Deployment Scope, for test
/// fixtures that seed or inspect rows of every Tenant with plain SQL. A
/// transaction opened with a scope still writes its own transaction-local
/// settings, which take precedence until it ends, so server code on this pool
/// behaves as on any other. Only debug builds have it, and clippy keeps it out
/// of server code.
#[cfg(debug_assertions)]
pub fn test_fixture_driver(
	options: sqlx::postgres::PgConnectOptions,
	max_connections: u32,
) -> sqlx::PgPool {
	sqlx::postgres::PgPoolOptions::new()
		.max_connections(max_connections)
		.after_connect(|connection, _| {
			Box::pin(async move {
				sqlx::query(
					"SELECT set_config($1, 'deployment', false), set_config($2, '', false), \
					 set_config('application_name', 'deployment:test-fixture', false)",
				)
				.bind(SCOPE_SETTING)
				.bind(TENANT_SETTING)
				.execute(connection)
				.await?;
				Ok(())
			})
		})
		.connect_lazy_with(options)
}

/// Starts a SQLx transaction that holds the test-fixture Deployment Scope, for
/// fixtures that write Tenant-owned rows through a pool with no scope of its
/// own. Only debug builds have it, and clippy keeps it out of server code.
#[cfg(debug_assertions)]
pub async fn begin_fixture_deployment(
	pool: &sqlx::PgPool,
) -> sqlx::Result<sqlx::Transaction<'static, sqlx::Postgres>> {
	let mut transaction = pool.begin().await?;
	sqlx::query("SELECT set_config($1, 'deployment', true), set_config($2, '', true)")
		.bind(SCOPE_SETTING)
		.bind(TENANT_SETTING)
		.execute(&mut *transaction)
		.await?;
	Ok(transaction)
}

/// Write `scope` into the transaction's settings. Call it once, before any
/// other statement of the transaction.
pub(crate) async fn enter(tx: &mut dyn TransactionExecutor, scope: &TenancyScope) -> Result<()> {
	let tenant = scope
		.tenant_name()
		.map(|name| name.as_str().to_owned())
		.unwrap_or_default();
	let mut select = Query::select();
	select
		.expr(set_config(SCOPE_SETTING, scope_value(scope).into()))
		.expr(set_config(TENANT_SETTING, tenant));
	if let TenancyScope::Deployment(reason) = scope {
		select.expr(set_config(
			"application_name",
			format!("deployment:{}", reason.label()),
		));
	}
	if scope.read_only() {
		select.expr(set_config("transaction_read_only", "on".into()));
	}
	let (sql, values) = select.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

/// Materialized by the migration asset generator for cross-Tenant backfills.
pub(crate) fn migration_deployment() -> String {
	Query::select()
		.expr(set_config(SCOPE_SETTING, "deployment".into()))
		.expr(set_config(TENANT_SETTING, String::new()))
		.to_string(PostgresQueryBuilder)
}

#[cfg(test)]
mod tests;
