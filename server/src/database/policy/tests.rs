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
	let policies = assert_policy_migrations_run_the_rendered_template(
		&migrations,
		&tenant_owned(TABLES),
		RETIRED,
		RENAMED,
	);
	assert!(policies > 0);
}

/// Tables dropped after their policy migration ran: by the compaction rebuild
/// (#850), with the operation-token External service route (#738), and with
/// the Research category (#987).
const RETIRED: &[&str] = &[
	"core_retention_files",
	"core_retention_holds",
	"decision_states",
	"decision_outcomes",
	"generation_compaction_usage",
	"generation_summary_usage",
	"egress_budgets",
	"egress_sessions",
	// The Research category (#987, ADR 0138).
	"research_attempts",
	"research_fragments",
	"research_journal_results",
	"research_originals",
	"research_page_images",
	"research_revocations",
	"research_snapshots",
	"research_sources",
	"tenant_retention_rule_history",
	"tenant_retention_rules",
	// The Tenant Duty limits (#1012).
	"schedule_limits",
	// Reply threads and the Task to reply-thread map (#1008).
	"channel_threads",
	"core_task_sessions",
];

/// Tables renamed after their policy migration ran: Schedules became Duties
/// (#1011), the Workspace became the Thread (#1008), and the Run became the
/// Task's execution record (#1009).
const RENAMED: &[(&str, &str)] = &[
	("schedules", "duties"),
	("schedule_ticks", "duty_ticks"),
	("workspaces", "threads"),
	("authorization_workspaces", "authorization_threads"),
	("runs", "task_executions"),
];

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
	"execution.0089_remove_agent_cluster forward",
	"execution.0090_run_created_at forward",
	"execution.0141_remove_research forward",
	"execution.0148_threads forward",
	"execution.0150_task_executions backward",
	"execution.0150_task_executions forward",
	"execution.0153_area_queue_sleep forward",
	"execution.0113_content_object_references backward",
	"execution.0113_content_object_references forward",
	"identity.0067_scratch_member_policy forward",
	"identity.0080_schedule_member_policy forward",
	"identity.0082_remove_package_index_approvals forward",
	"identity.0083_data_protection_switches backward",
	"identity.0083_data_protection_switches forward",
	"identity.0085_remove_research_approvals forward",
	"identity.0098_duty_member_policy backward",
	"identity.0098_duty_member_policy forward",
	"identity.0110_threads forward",
	"identity.0111_thread_member_policy backward",
	"identity.0111_thread_member_policy forward",
	"identity.0113_budget_notice_delivery backward",
	"identity.0114_policy_revision_backfill forward",
	"registry.0054_remove_agent_cluster forward",
	"registry.0070_remove_package_index_draft_bindings forward",
	"registry.0072_remove_agent_external_service_draft_bindings forward",
	"registry.0074_remove_research_providers forward",
	"workspaces.0021_conversation_scratch_space forward",
	"workspaces.0024_workspace_scratch_space forward",
	"workspaces.0033_duty_checks backward",
	"workspaces.0033_duty_checks forward",
	"workspaces.0049_threads backward",
	"workspaces.0067_task_duty_start backward",
	"workspaces.0067_task_duty_start forward",
];

#[rstest::rstest]
#[tokio::test]
async fn no_migration_writes_tenant_owned_rows_under_their_policy() {
	// Arrange
	let migrations = migrations().await;
	// Act + Assert: the scan finds the known writers, and only them.
	// A renamed table's history writes it under its former name.
	let mut owned = tenant_owned(TABLES);
	owned.extend(RENAMED.iter().map(|(former, _)| *former));
	assert_no_migration_writes_tenant_owned_rows(
		&migrations,
		&owned,
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
