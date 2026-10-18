//! The row-level policy of every Tenant-owned table (#223, #264).
//!
//! Migrations hold the policy statements as literal SQL assets, because the
//! migration source reads assets without running Rust. This module renders
//! them, and a test checks every asset against its rendering, so all tables
//! keep one template.

/// The name of the policy every Tenant-owned table carries.
pub const POLICY: &str = "tenant_isolation";

/// The rows a transaction may read and write: every row in a Deployment
/// Scope, and the scope's Tenant's rows in a Tenant scope. Each function call
/// is a sub-select, so PostgreSQL evaluates it once per statement as an
/// InitPlan instead of once per row.
pub const EXPRESSION: &str =
	"(SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant())";

/// The SQLSTATE the policy functions raise when a transaction carries no
/// usable tenancy scope. It belongs to no PostgreSQL class.
pub const SCOPE_MISSING: &str = "TS001";

/// Statements that enable and force the policy on `table`, a schema-qualified
/// name.
pub fn enable_table(table: &str) -> String {
	format!(
		"ALTER TABLE {table} ENABLE ROW LEVEL SECURITY;\n\
		 ALTER TABLE {table} FORCE ROW LEVEL SECURITY;\n\
		 CREATE POLICY {POLICY} ON {table} AS PERMISSIVE FOR ALL TO PUBLIC\n    \
		 USING ({EXPRESSION})\n    \
		 WITH CHECK ({EXPRESSION});\n"
	)
}

/// Statements that remove what [`enable_table`] added.
pub fn disable_table(table: &str) -> String {
	format!(
		"DROP POLICY {POLICY} ON {table};\n\
		 ALTER TABLE {table} NO FORCE ROW LEVEL SECURITY;\n\
		 ALTER TABLE {table} DISABLE ROW LEVEL SECURITY;\n"
	)
}

/// The forward asset of a migration that enforces the policy on `tables` in
/// the public schema.
pub fn enable(tables: &[&str]) -> String {
	tables
		.iter()
		.map(|table| enable_table(&format!("public.{table}")))
		.collect()
}

/// The backward asset of that migration, in reverse table order.
pub fn disable(tables: &[&str]) -> String {
	tables
		.iter()
		.rev()
		.map(|table| disable_table(&format!("public.{table}")))
		.collect()
}

#[cfg(test)]
mod tests;
