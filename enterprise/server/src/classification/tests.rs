// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

use super::TABLES;
use crate::BUILD;
use aidash_server::database::classification::TABLES as COMMUNITY;
use std::collections::BTreeSet;
use std::path::Path;

#[test]
fn the_build_lists_its_tables_after_communitys() {
	assert!(BUILD.tables().skip(COMMUNITY.len()).eq(TABLES.iter()));
}

#[test]
fn tables_are_listed_once_under_an_app_the_build_adds() {
	let migrations = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
	let mut names = BTreeSet::new();
	for table in BUILD.tables() {
		assert!(names.insert(table.name), "{} is listed twice", table.name);
	}
	for table in TABLES {
		assert!(
			BUILD.apps.contains(&table.app) && migrations.join(table.app).is_dir(),
			"{}: the build adds no app {} with migrations",
			table.name,
			table.app
		);
	}
}
