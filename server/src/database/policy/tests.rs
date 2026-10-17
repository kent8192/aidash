use super::*;
use crate::database::classification::{Classification, TABLES};
use regex::Regex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const ASSET: &str = "_row_level_security.sql";

fn migrations() -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations")
}

fn files(directory: &Path, found: &mut Vec<PathBuf>) {
	for entry in std::fs::read_dir(directory).unwrap() {
		let path = entry.unwrap().path();
		if path.is_dir() {
			files(&path, found);
		} else if path
			.extension()
			.is_some_and(|extension| extension == "rs" || extension == "sql")
		{
			found.push(path);
		}
	}
}

fn tenant_owned() -> BTreeSet<&'static str> {
	TABLES
		.iter()
		.filter(|table| table.classification == Classification::TenantOwned)
		.map(|table| table.name)
		.collect()
}

/// The tables a forward policy asset enables, in asset order.
fn enabled(asset: &str) -> Vec<String> {
	Regex::new(r"(?m)^ALTER TABLE public\.([a-z0-9_]+) ENABLE ROW LEVEL SECURITY;$")
		.unwrap()
		.captures_iter(asset)
		.map(|captures| captures[1].to_owned())
		.collect()
}

#[test]
fn every_policy_asset_is_the_rendered_template() {
	// Arrange
	let mut covered = BTreeSet::new();
	let mut assets = 0;
	for app in std::fs::read_dir(migrations()).unwrap() {
		let forward = app.unwrap().path().join("sql/forward");
		if !forward.is_dir() {
			continue;
		}
		for entry in std::fs::read_dir(&forward).unwrap() {
			let path = entry.unwrap().path();
			let name = path.file_name().unwrap().to_string_lossy().into_owned();
			if !name.ends_with(ASSET) {
				continue;
			}
			assets += 1;
			let asset = std::fs::read_to_string(&path).unwrap();
			let backward =
				std::fs::read_to_string(forward.join("../backward").join(&name)).unwrap();
			let tables = enabled(&asset);
			let tables = tables.iter().map(String::as_str).collect::<Vec<_>>();
			// Act + Assert: the asset holds nothing but the template.
			assert_eq!(asset, enable(&tables), "{}", path.display());
			assert_eq!(backward, disable(&tables), "{} reversed", path.display());
			for table in tables {
				assert!(covered.insert(table.to_owned()), "{table} is enabled twice");
			}
		}
	}
	// Every enforced table, and nothing else, has a policy asset.
	assert!(assets > 0);
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

/// Migrations that wrote Tenant-owned rows before #264 enforced their tables'
/// policies. They run while no policy exists: forward before the policy
/// migration, and backward after its reversal.
const BEFORE_POLICIES: &[&str] = &[
	"identity/sql/forward/0020_tenancy_backfill.sql",
	"identity/sql/backward/0020_history_identity_nullable.sql",
];

/// A policy raises only when a statement scans a row, so a migration that
/// writes Tenant-owned rows after their policy exists passes on an empty
/// database and fails on a Deployment with data. Seed such rows from the
/// server, inside a scope, or edit the table's baseline definition.
#[test]
fn no_migration_writes_tenant_owned_rows_under_their_policy() {
	// Arrange
	let root = migrations();
	let mut sources = Vec::new();
	files(&root, &mut sources);
	let owned = tenant_owned();
	let statement = Regex::new(
		r#"(?i)\b(?:INSERT\s+INTO|UPDATE|DELETE\s+FROM|TRUNCATE(?:\s+TABLE)?|MERGE\s+INTO|COPY)\s+(?:ONLY\s+)?(?:public\.)?"?([a-z_][a-z0-9_]*)"#,
	)
	.unwrap();
	// Act
	let mut writers = BTreeSet::new();
	for path in &sources {
		let text = without_function_bodies(&std::fs::read_to_string(path).unwrap());
		for captures in statement.captures_iter(&text) {
			if owned.contains(captures[1].to_ascii_lowercase().as_str()) {
				let relative = path.strip_prefix(&root).unwrap().to_string_lossy();
				writers.insert(relative.replace('\\', "/"));
			}
		}
	}
	// Assert: the scan finds the known writers, and only them.
	let known = BEFORE_POLICIES
		.iter()
		.map(|path| (*path).to_owned())
		.collect::<BTreeSet<_>>();
	assert_eq!(writers, known);
}

#[test]
fn function_bodies_are_removed_but_do_blocks_are_kept() {
	let text = "CREATE FUNCTION f() RETURNS void AS $body$ BEGIN DELETE FROM a; END $body$;\n\
	            DO $x$ BEGIN DELETE FROM b; END $x$;";
	let kept = without_function_bodies(text);
	assert!(!kept.contains("DELETE FROM a"), "{kept}");
	assert!(kept.contains("DELETE FROM b"), "{kept}");
}
