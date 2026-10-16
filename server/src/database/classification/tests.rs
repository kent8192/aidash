use super::*;
use std::collections::BTreeSet;
use std::path::Path;

#[test]
fn tables_are_listed_once_under_an_app_that_has_migrations() {
	let migrations = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
	let mut names = BTreeSet::new();
	for table in TABLES {
		assert!(names.insert(table.name), "{} is listed twice", table.name);
		// The migration ledger belongs to Reinhardt rather than to an app.
		assert!(
			table.app == "reinhardt" || migrations.join(table.app).is_dir(),
			"{}: no migrations for app {}",
			table.name,
			table.app
		);
	}
}

#[test]
fn pending_tables_name_a_step_of_the_rollout() {
	for table in TABLES {
		if let Classification::Pending(issue) = table.classification {
			assert!(
				[264, 265, 241].contains(&issue),
				"{}: #{issue} enforces no policies",
				table.name
			);
		}
	}
}
