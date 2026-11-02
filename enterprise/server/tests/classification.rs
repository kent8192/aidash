// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! The Enterprise build's tables are classified and enforced like Community's
//! (#223, #266). Both workspaces run the same checks, compiled from the
//! Community sources.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../../server/src/database/classification/comparison.rs"]
mod comparison;
#[path = "../../../server/src/apps/execution/tests/support/native_database.rs"]
mod native_database;
#[path = "../../../server/src/database/policy/checks.rs"]
mod policy_checks;

use aidash_enterprise_server::BUILD;
use aidash_enterprise_server::classification::TABLES;
use aidash_server::database::classification::Table;
use aidash_server::database::runtime;
use native_database::{DatabaseFixture, database};
use rstest::rstest;
use std::path::Path;

fn every_table() -> Vec<Table> {
	BUILD.tables().copied().collect()
}

#[rstest]
#[tokio::test]
async fn every_table_is_classified_and_matches_its_policies(#[future] database: DatabaseFixture) {
	// Arrange: the fixture migrates Community's roots and the installed build's.
	aidash_server::bootstrap::build::install(&BUILD);
	let f = database.await;
	let pool = runtime::connect(&f.url, 2)
		.await
		.unwrap()
		.into_postgres()
		.unwrap();
	// Act + Assert: the expected list is Community's plus Enterprise's.
	comparison::check(
		&pool,
		&every_table(),
		"enterprise/server/src/classification.rs",
	)
	.await;
}

#[tokio::test]
async fn enterprise_policy_migrations_run_the_template_and_write_no_tenant_owned_rows() {
	// Arrange
	let migrations =
		policy_checks::migrations(&Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations")).await;
	// Act + Assert: Enterprise's migrations enable exactly Enterprise's
	// Tenant-owned tables, and write no Tenant-owned row of either workspace.
	policy_checks::assert_policy_migrations_run_the_rendered_template(
		&migrations,
		&policy_checks::tenant_owned(TABLES),
	);
	policy_checks::assert_no_migration_writes_tenant_owned_rows(
		&migrations,
		&policy_checks::tenant_owned(&every_table()),
		&[],
	);
}
