//! Order reversal through Reinhardt's complete dependency graph.
//!
//! The pinned command scopes reversal to records in the selected app (#6515).
//! Remove this compatibility planner when native targets include applied
//! dependents across apps and complete baseline reversal/replay tests pass.

use reinhardt::db::migrations::{
	Migration, Result,
	graph::{MigrationGraph, MigrationKey},
	recorder::MigrationRecord,
};
use std::collections::HashSet;

/// Preserve native target direction, then add applied dependent migrations.
pub(super) fn plan(
	migrations: &[Migration],
	applied: &[MigrationRecord],
	app: &str,
	target: &str,
) -> Result<Option<Vec<Migration>>> {
	let records: Vec<_> = applied.iter().filter(|record| record.app == app).collect();
	let first = if target == "zero" {
		0
	} else if let Some(position) = records.iter().position(|record| record.name == target) {
		position + 1
	} else {
		return Ok(None);
	};
	let mut graph = MigrationGraph::new();
	for migration in migrations {
		graph.add_migration(
			MigrationKey::new(&migration.app_label, &migration.name),
			migration
				.dependencies
				.iter()
				.map(|(app, name)| MigrationKey::new(app, name))
				.collect(),
		);
	}
	let applied: HashSet<_> = applied
		.iter()
		.map(|record| MigrationKey::new(&record.app, &record.name))
		.collect();
	let mut pending: Vec<_> = records[first..]
		.iter()
		.map(|record| MigrationKey::new(&record.app, &record.name))
		.collect();
	let mut selected = HashSet::new();
	while let Some(key) = pending.pop() {
		if !selected.insert(key.clone()) {
			continue;
		}
		pending.extend(
			graph
				.get_dependents(&key)
				.into_iter()
				.filter(|key| applied.contains(*key))
				.cloned(),
		);
	}
	let mut ordered = Vec::new();
	for key in graph.topological_sort()? {
		if selected.contains(&key) {
			let migration = migrations
				.iter()
				.find(|migration| {
					migration.app_label == key.app_label && migration.name == key.name
				})
				.expect("graph contains only supplied migration keys");
			ordered.push(migration.clone());
		}
	}
	Ok(Some(ordered))
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;

	#[rstest]
	#[case::zero("zero", Some(vec![("owner", "0001_tables"), ("dependent", "0001_references"), ("owner", "0002_state")]))]
	#[case::retained_target("0001_tables", Some(vec![("owner", "0002_state")]))]
	#[case::current_target("0002_state", Some(vec![]))]
	#[case::forward_target("0003_future", None)]
	fn reverse_targets_include_only_applied_dependents(
		#[case] target: &str,
		#[case] expected: Option<Vec<(&str, &str)>>,
	) {
		// Arrange: a second app references the owner's table, while another app
		// shares only the environment prerequisite and must remain applied.
		let migrations = vec![
			Migration::new("0000_environment", "operations"),
			Migration::new("0001_tables", "owner").add_dependency("operations", "0000_environment"),
			Migration::new("0001_references", "dependent").add_dependency("owner", "0001_tables"),
			Migration::new("0002_state", "owner")
				.add_dependency("owner", "0001_tables")
				.state_only(true),
			Migration::new("0001_tables", "independent")
				.add_dependency("operations", "0000_environment"),
			Migration::new("0003_future", "owner").add_dependency("owner", "0002_state"),
		];
		let applied: Vec<_> = migrations[..5]
			.iter()
			.map(|migration| MigrationRecord {
				app: migration.app_label.clone(),
				name: migration.name.clone(),
				applied: Default::default(),
			})
			.collect();
		// Act
		let actual = plan(&migrations, &applied, "owner", target).unwrap();
		// Assert: prerequisites precede dependents for the native reverse iterator.
		assert_eq!(
			actual.as_ref().map(|plan| plan
				.iter()
				.map(|migration| (migration.app_label.as_str(), migration.name.as_str()))
				.collect::<Vec<_>>()),
			expected
		);
	}
}
