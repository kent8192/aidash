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
	Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};

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

/// The scope of rows that record their Tenant by name. A legacy Workspace's
/// rows record none until #265 gives every Workspace a Tenant.
pub(crate) fn recorded(tenant: &str) -> Result<TenancyScope> {
	if tenant.is_empty() {
		Ok(TenancyScope::Deployment(
			aidash_application::tenancy::DeploymentScopeReason::LegacyRun,
		))
	} else {
		Ok(TenancyScope::tenant(tenant)?)
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

#[cfg(test)]
mod tests;
