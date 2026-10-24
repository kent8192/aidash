use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[path = "comparison.rs"]
mod comparison;
use comparison::{Observed, compare};

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

fn enforced() -> Observed {
	let template = crate::database::policy::EXPRESSION.to_owned();
	Observed {
		tenant_column: true,
		row_security: true,
		forced: true,
		policies: 1,
		policy: Some((
			crate::database::policy::POLICY.to_owned(),
			"ALL".to_owned(),
			vec!["public".to_owned()],
			template.clone(),
			template,
		)),
	}
}

fn open() -> Observed {
	Observed {
		tenant_column: false,
		row_security: false,
		forced: false,
		policies: 0,
		policy: None,
	}
}

fn observed<const N: usize>(tables: [(&str, Observed); N]) -> BTreeMap<String, Observed> {
	tables
		.into_iter()
		.map(|(name, table)| (name.to_owned(), table))
		.collect()
}

const LIST: &[Table] = &[
	table("app", "owned", Classification::TenantOwned),
	table("app", "wide", Classification::DeploymentWide),
];

#[test]
fn the_comparison_accepts_a_database_that_matches_its_list() {
	compare(
		&observed([("owned", enforced()), ("wide", open())]),
		LIST,
		"the list",
	);
}

#[test]
#[should_panic(expected = "classify every table in the list.\nunlisted: [\"extra\"]")]
fn the_comparison_names_a_table_the_list_lacks_and_the_file_to_list_it_in() {
	compare(
		&observed([("owned", enforced()), ("wide", open()), ("extra", open())]),
		LIST,
		"the list",
	);
}

#[test]
#[should_panic(expected = "missing from the database: [\"wide\"]")]
fn the_comparison_names_a_listed_table_the_database_lacks() {
	compare(&observed([("owned", enforced())]), LIST, "the list");
}

#[test]
#[should_panic(expected = "owned: Observed")]
fn a_tenant_owned_table_without_its_policy_fails() {
	compare(
		&observed([
			(
				"owned",
				Observed {
					tenant_column: true,
					..open()
				},
			),
			("wide", open()),
		]),
		LIST,
		"the list",
	);
}

#[test]
#[should_panic(expected = "wide is Deployment-wide but has row-level security")]
fn a_deployment_wide_table_with_a_policy_fails() {
	compare(
		&observed([("owned", enforced()), ("wide", enforced())]),
		LIST,
		"the list",
	);
}
