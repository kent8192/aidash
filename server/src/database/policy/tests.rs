use super::*;
use crate::database::classification::{Classification, TABLES};
use regex::Regex;
use reinhardt::db::migrations::{FilesystemSource, Migration, MigrationSource, Operation};
use std::collections::BTreeSet;
use std::path::Path;

/// Every migration as the Reinhardt filesystem source loads it, with each
/// `RunSQL` asset resolved to its text.
async fn migrations() -> Vec<Migration> {
	FilesystemSource::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"))
		.all_migrations()
		.await
		.unwrap()
}

/// The forward and backward SQL of every `RunSQL` operation in `migration`.
fn statements(migration: &Migration) -> impl Iterator<Item = (&str, Option<&str>)> {
	migration
		.operations
		.iter()
		.filter_map(|operation| match operation {
			Operation::RunSQL { sql, reverse_sql } => Some((sql.as_str(), reverse_sql.as_deref())),
			_ => None,
		})
}

fn tenant_owned() -> BTreeSet<&'static str> {
	TABLES
		.iter()
		.filter(|table| table.classification == Classification::TenantOwned)
		.map(|table| table.name)
		.collect()
}

/// The tables a forward policy statement enables, in statement order.
fn enabled(sql: &str) -> Vec<String> {
	Regex::new(r"(?m)^ALTER TABLE public\.([a-z0-9_]+) ENABLE ROW LEVEL SECURITY;$")
		.unwrap()
		.captures_iter(sql)
		.map(|captures| captures[1].to_owned())
		.collect()
}

#[rstest::rstest]
#[tokio::test]
async fn every_policy_migration_runs_the_rendered_template() {
	// Arrange
	let migrations = migrations().await;
	let mut covered = BTreeSet::new();
	let mut policies = 0;
	for migration in migrations
		.iter()
		.filter(|migration| migration.name.ends_with("_row_level_security"))
	{
		for (sql, reverse_sql) in statements(migration) {
			let tables = enabled(sql);
			if tables.is_empty() {
				continue;
			}
			policies += 1;
			let tables = tables.iter().map(String::as_str).collect::<Vec<_>>();
			// Act + Assert: the operation holds nothing but the template.
			let label = format!("{}.{}", migration.app_label, migration.name);
			assert_eq!(sql, enable(&tables), "{label}");
			assert_eq!(
				reverse_sql,
				Some(disable(&tables).as_str()),
				"{label} reversed"
			);
			for table in tables {
				assert!(covered.insert(table.to_owned()), "{table} is enabled twice");
			}
		}
	}
	// Every enforced table, and nothing else, has a policy migration.
	assert!(policies > 0);
	let expected = tenant_owned()
		.into_iter()
		.map(str::to_owned)
		.collect::<BTreeSet<_>>();
	assert_eq!(covered, expected);
}

/// Removes the bodies of `CREATE FUNCTION … AS $tag$ … $tag$`. They run when
/// the function is called, inside the caller's scope, not while migrating. A
/// `DO` block runs while migrating and is kept.
fn without_function_bodies(text: &str) -> String {
	let start = Regex::new(r"(?i)\bAS\s+(\$[A-Za-z0-9_]*\$)").unwrap();
	let mut kept = String::new();
	let mut rest = text;
	while let Some(captures) = start.captures(rest) {
		let whole = captures.get(0).unwrap();
		let tag = captures.get(1).unwrap().as_str();
		kept.push_str(&rest[..whole.start()]);
		match rest[whole.end()..].find(tag) {
			Some(end) => rest = &rest[whole.end() + end + tag.len()..],
			None => {
				rest = &rest[whole.start()..];
				break;
			}
		}
	}
	kept.push_str(rest);
	kept
}

/// Migration directions that wrote Tenant-owned rows before #264 enforced
/// their tables' policies. They run while no policy exists: forward before the
/// policy migration, and backward after its reversal.
const BEFORE_POLICIES: &[&str] = &[
	"identity.0020_tenancy_schema forward",
	"identity.0020_tenancy_schema backward",
];

/// A policy raises only when a statement scans a row, so a migration that
/// writes Tenant-owned rows after their policy exists passes on an empty
/// database and fails on a Deployment with data. Seed such rows from the
/// server, inside a scope, or edit the table's baseline definition.
#[rstest::rstest]
#[tokio::test]
async fn no_migration_writes_tenant_owned_rows_under_their_policy() {
	// Arrange
	let migrations = migrations().await;
	let owned = tenant_owned();
	let statement = Regex::new(
		r#"(?i)\b(?:INSERT\s+INTO|UPDATE|DELETE\s+FROM|TRUNCATE(?:\s+TABLE)?|MERGE\s+INTO|COPY)\s+(?:ONLY\s+)?(?:public\.)?"?([a-z_][a-z0-9_]*)"#,
	)
	.unwrap();
	let writes = |sql: &str| {
		statement
			.captures_iter(&without_function_bodies(sql))
			.any(|captures| owned.contains(captures[1].to_ascii_lowercase().as_str()))
	};
	// Act
	let mut writers = BTreeSet::new();
	for migration in &migrations {
		let label = format!("{}.{}", migration.app_label, migration.name);
		for (sql, reverse_sql) in statements(migration) {
			if writes(sql) {
				writers.insert(format!("{label} forward"));
			}
			if reverse_sql.is_some_and(writes) {
				writers.insert(format!("{label} backward"));
			}
		}
	}
	// Assert: the scan finds the known writers, and only them.
	let known = BEFORE_POLICIES
		.iter()
		.map(|label| (*label).to_owned())
		.collect::<BTreeSet<_>>();
	assert_eq!(writers, known);
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
