use super::*;
use aidash_application::tenancy::DeploymentScopeReason;
use std::path::{Path, PathBuf};

#[test]
fn every_scope_has_its_own_setting_value() {
	assert_eq!(
		scope_value(&TenancyScope::tenant("acme").unwrap()),
		"tenant"
	);
	assert_eq!(
		scope_value(&TenancyScope::Deployment(DeploymentScopeReason::Retention)),
		"deployment"
	);
	assert_eq!(scope_value(&TenancyScope::NoTenant), "none");
}

fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
	for entry in std::fs::read_dir(directory).unwrap() {
		let path = entry.unwrap().path();
		if path.is_dir() {
			sources(&path, found);
		} else if path
			.extension()
			.is_some_and(|extension| extension == "rs" || extension == "sql")
		{
			found.push(path);
		}
	}
}

/// Only the code that opens transactions writes the scope settings, so no
/// statement can widen or switch a transaction's scope after it began.
#[test]
fn only_the_transaction_opener_writes_the_scope_settings() {
	let root = Path::new(env!("CARGO_MANIFEST_DIR"));
	let opener = root.join("src/database/scope.rs");
	let mut files = Vec::new();
	for directory in ["src", "migrations", "tests"] {
		if root.join(directory).is_dir() {
			sources(&root.join(directory), &mut files);
		}
	}
	let needles = [
		format!("{SCOPE_SETTING:?}"),
		format!("{TENANT_SETTING:?}"),
		format!("'{SCOPE_SETTING}'"),
		format!("'{TENANT_SETTING}'"),
	];
	let writers = files
		.iter()
		.filter(|path| **path != opener && !path.starts_with(root.join("src/database/scope")))
		.filter(|path| {
			let text = std::fs::read_to_string(path).unwrap();
			text.lines().any(|line| {
				line.contains("set_config") && needles.iter().any(|needle| line.contains(needle))
			}) || needles[..2]
				.iter()
				.any(|needle| text.contains(needle.as_str()))
		})
		.collect::<Vec<_>>();
	assert!(writers.is_empty(), "{writers:?}");
	// These owner-only atomic migrations open their Deployment Scope before
	// touching rows. No other SQL asset may change it later in a transaction.
	let mut migration_openers = files
		.iter()
		.filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
		.filter(|path| {
			std::fs::read_to_string(path)
				.unwrap()
				.contains("SET LOCAL aidash.scope")
		})
		.map(|path| {
			path.strip_prefix(root)
				.unwrap()
				.to_string_lossy()
				.into_owned()
		})
		.collect::<Vec<_>>();
	migration_openers.sort();
	assert_eq!(
		migration_openers,
		[
			"migrations/execution/sql/backward/0113_content_object_references_triggers.sql",
			"migrations/execution/sql/backward/0150_execution_events.sql",
			"migrations/execution/sql/forward/0089_remove_agent_cluster.sql",
			"migrations/execution/sql/forward/0090_run_created_at.sql",
			"migrations/execution/sql/forward/0113_content_object_references_legacy.sql",
			"migrations/execution/sql/forward/0141_remove_research_records.sql",
			"migrations/execution/sql/forward/0148_duplicate_areas.sql",
			"migrations/execution/sql/forward/0150_execution_events.sql",
			"migrations/execution/sql/forward/0153_area_queue_sleep_backfill.sql",
			"migrations/identity/sql/backward/0083_data_protection_switches.sql",
			"migrations/identity/sql/backward/0098_duty_member_policy.sql",
			"migrations/identity/sql/backward/0111_thread_member_policy.sql",
			"migrations/identity/sql/backward/0113_budget_notice_delivery.sql",
			"migrations/identity/sql/forward/0067_scratch_member_policy.sql",
			"migrations/identity/sql/forward/0080_schedule_member_policy.sql",
			"migrations/identity/sql/forward/0082_remove_package_index_approvals.sql",
			"migrations/identity/sql/forward/0083_data_protection_switches.sql",
			"migrations/identity/sql/forward/0085_remove_research_approvals.sql",
			"migrations/identity/sql/forward/0098_duty_member_policy.sql",
			"migrations/identity/sql/forward/0110_reply_thread_modes.sql",
			"migrations/identity/sql/forward/0111_thread_member_policy.sql",
			"migrations/identity/sql/forward/0114_policy_revision_backfill.sql",
			"migrations/registry/sql/forward/0054_remove_agent_cluster.sql",
			"migrations/registry/sql/forward/0055_remove_structured_harness_contracts.sql",
			"migrations/registry/sql/forward/0070_remove_package_index_draft_bindings.sql",
			"migrations/registry/sql/forward/0072_remove_agent_external_service_draft_bindings.sql",
			"migrations/registry/sql/forward/0074_remove_research_providers.sql",
			"migrations/workspaces/sql/backward/0033_duty_halts.sql",
			"migrations/workspaces/sql/backward/0049_workspace_goal.sql",
			"migrations/workspaces/sql/backward/0067_task_duty_start.sql",
			"migrations/workspaces/sql/forward/0021_conversation_scratch_space.sql",
			"migrations/workspaces/sql/forward/0024_workspace_scratch_space.sql",
			"migrations/workspaces/sql/forward/0033_duty_halts.sql",
			"migrations/workspaces/sql/forward/0067_task_duty_start.sql",
		]
	);
	for path in migration_openers {
		let text = std::fs::read_to_string(root.join(path)).unwrap();
		assert_eq!(text.matches("SET LOCAL aidash.scope").count(), 1);
		let first = text
			.lines()
			.find(|line| !line.trim().is_empty() && !line.trim_start().starts_with("--"))
			.unwrap();
		assert_eq!(first, "SET LOCAL aidash.scope = 'deployment';");
	}
}
