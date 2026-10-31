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
			"migrations/execution/sql/forward/0082_remove_agent_cluster.sql",
			"migrations/registry/sql/forward/0050_remove_agent_cluster.sql",
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
