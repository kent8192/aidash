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
