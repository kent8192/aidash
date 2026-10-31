//! What a build's migrations must do with row-level policies (#264, #266).
//!
//! This file is test source, not part of the library, shared by `#[path]`
//! like `classification/comparison.rs`: Community's unit tests run it on
//! Community's migrations, and the Enterprise build's tests run it on theirs.
use aidash_server::database::classification::{Classification, Table};
use aidash_server::database::policy::{disable, enable};
use regex::Regex;
use reinhardt::db::migrations::{FilesystemSource, Migration, MigrationSource, Operation};
use std::collections::BTreeSet;
use std::path::Path;

/// Every migration under `root` as the Reinhardt filesystem source loads it,
/// with each `RunSQL` asset resolved to its text.
pub async fn migrations(root: &Path) -> Vec<Migration> {
	FilesystemSource::new(root).all_migrations().await.unwrap()
}

/// The forward and backward SQL of every `RunSQL` operation in `migration`.
pub fn statements(migration: &Migration) -> impl Iterator<Item = (&str, Option<&str>)> {
	migration
		.operations
		.iter()
		.filter_map(|operation| match operation {
			Operation::RunSQL { sql, reverse_sql } => Some((sql.as_str(), reverse_sql.as_deref())),
			_ => None,
		})
}

/// The names of the Tenant-owned tables among `tables`.
pub fn tenant_owned(tables: &[Table]) -> BTreeSet<&'static str> {
	tables
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

/// Every policy migration in `migrations` runs nothing but the rendered
/// template, and together they enable the `owned` tables and no other. Returns
/// how many operations enable tables.
pub fn assert_policy_migrations_run_the_rendered_template(
	migrations: &[Migration],
	owned: &BTreeSet<&str>,
) -> usize {
	let mut covered = BTreeSet::new();
	let mut policies = 0;
	// A schema migration can carry its table's policy asset as well. Inspect
	// operations rather than requiring a separate policy migration filename.
	for migration in migrations {
		for (sql, reverse_sql) in statements(migration) {
			let tables = enabled(sql);
			if tables.is_empty() {
				continue;
			}
			policies += 1;
			let tables = tables.iter().map(String::as_str).collect::<Vec<_>>();
			// The operation holds nothing but the template.
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
	let expected: BTreeSet<String> = owned.iter().copied().map(str::to_owned).collect();
	assert_eq!(
		covered, expected,
		"left: tables the policy migrations enable; right: Tenant-owned tables of the classification"
	);
	policies
}

/// Removes the bodies of `CREATE FUNCTION … AS $tag$ … $tag$`. They run when
/// the function is called, inside the caller's scope, not while migrating. A
/// `DO` block runs while migrating and is kept.
pub fn without_function_bodies(text: &str) -> String {
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

/// A policy raises only when a statement scans a row, so a migration that
/// writes Tenant-owned rows after their policy exists passes on an empty
/// database and fails on a Deployment with data. Seed such rows from the
/// server, inside a scope, or edit the table's baseline definition.
///
/// Panics unless the migration directions in `migrations` that write `owned`
/// rows are exactly `known`, written `app.name forward` or `app.name backward`.
pub fn assert_no_migration_writes_tenant_owned_rows(
	migrations: &[Migration],
	owned: &BTreeSet<&str>,
	known: &[&str],
) {
	let statement = Regex::new(
		r#"(?i)\b(?:INSERT\s+INTO|UPDATE|DELETE\s+FROM|TRUNCATE(?:\s+TABLE)?|MERGE\s+INTO|COPY)\s+(?:ONLY\s+)?(?:public\.)?"?([a-z_][a-z0-9_]*)"#,
	)
	.unwrap();
	let writes = |sql: &str| {
		statement
			.captures_iter(&without_function_bodies(sql))
			.any(|captures| owned.contains(captures[1].to_ascii_lowercase().as_str()))
	};
	let mut writers = BTreeSet::new();
	for migration in migrations {
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
	let known = known
		.iter()
		.map(|label| (*label).to_owned())
		.collect::<BTreeSet<_>>();
	assert_eq!(
		writers, known,
		"left: migration directions that write Tenant-owned rows; right: the known writers"
	);
}
