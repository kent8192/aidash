use crate::database::classification::TABLES;
use reinhardt::db::migrations::Migration;
use std::path::Path;

#[path = "checks.rs"]
mod checks;
use checks::{
	assert_no_migration_writes_tenant_owned_rows,
	assert_policy_migrations_run_the_rendered_template, statements, tenant_owned,
	without_function_bodies,
};

/// Every Community migration as the Reinhardt filesystem source loads it.
async fn migrations() -> Vec<Migration> {
	checks::migrations(&Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations")).await
}

#[rstest::rstest]
#[tokio::test]
async fn every_policy_migration_runs_the_rendered_template() {
	// Arrange
	let migrations = migrations().await;
	// Act + Assert: every enforced table, and nothing else, has a policy migration.
	let policies =
		assert_policy_migrations_run_the_rendered_template(&migrations, &tenant_owned(TABLES));
	assert!(policies > 0);
}

/// Migration directions that wrote Tenant-owned rows before #264 enforced
/// their tables' policies. They run while no policy exists: forward before the
/// policy migration, and backward after its reversal.
const BEFORE_POLICIES: &[&str] = &[
	"identity.0020_tenancy_schema forward",
	"identity.0020_tenancy_schema backward",
];

/// Migration directions that write Tenant-owned rows under their policies
/// after opening Deployment Scope with `SET LOCAL aidash.scope`, because the
/// native migration executor has no scope API.
const IN_DEPLOYMENT_SCOPE: &[&str] = &[
	"execution.0084_remove_agent_cluster forward",
	"registry.0053_remove_agent_cluster forward",
];

#[rstest::rstest]
#[tokio::test]
async fn no_migration_writes_tenant_owned_rows_under_their_policy() {
	// Arrange
	let migrations = migrations().await;
	// Act + Assert: the scan finds the known writers, and only them.
	assert_no_migration_writes_tenant_owned_rows(
		&migrations,
		&tenant_owned(TABLES),
		&[BEFORE_POLICIES, IN_DEPLOYMENT_SCOPE].concat(),
	);
}

#[rstest::rstest]
#[tokio::test]
async fn function_bodies_are_removed_but_do_blocks_are_kept() {
	// Arrange: the policy functions' migration, whose bodies raise.
	let migrations = migrations().await;
	let functions = migrations
		.iter()
		.find(|migration| {
			(migration.app_label.as_str(), migration.name.as_str())
				== ("identity", "0028_tenancy_policy_functions")
		})
		.unwrap();
	let sql = statements(functions)
		.map(|(sql, _)| sql)
		.collect::<Vec<_>>()
		.join("\n");
	let block = "DO $x$ BEGIN DELETE FROM b; END $x$;";
	// Act
	let kept = without_function_bodies(&format!("{sql}\n{block}"));
	// Assert
	assert!(sql.contains("RAISE EXCEPTION"), "{sql}");
	assert!(
		kept.contains("CREATE FUNCTION public.aidash_scope_tenant()"),
		"{kept}"
	);
	assert!(!kept.contains("RAISE EXCEPTION"), "{kept}");
	assert!(kept.contains(block), "{kept}");
}
