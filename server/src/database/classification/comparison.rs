//! The comparison between a build's classification list and its migrated
//! database (#223, #266).
//!
//! This file is test source, not part of the library. Each test crate that
//! runs the comparison compiles it with `#[path]`: Community's `startup` test
//! with Community's list, and the Enterprise build's tests with the list
//! [`Build::tables`](aidash_server::bootstrap::build::Build::tables) returns.
//! The rules therefore live here once, and Community names nothing of
//! Enterprise.
use aidash_server::database::classification::{Classification, Table};
use aidash_server::database::policy;
use std::collections::{BTreeMap, BTreeSet};

/// One table as the database sees it.
#[derive(Debug)]
pub struct Observed {
	pub tenant_column: bool,
	pub row_security: bool,
	pub forced: bool,
	pub policies: i64,
	/// The one policy's name, command, roles, USING and WITH CHECK, when the
	/// table has exactly one permissive policy.
	pub policy: Option<(String, String, Vec<String>, String, String)>,
}

async fn observe(pool: &sqlx::PgPool) -> BTreeMap<String, Observed> {
	// Catalog introspection that Reinhardt Query has no builder for.
	let rows: Vec<(String, bool, bool, bool, i64)> = sqlx::query_as(
		"SELECT c.relname::text, \
		 EXISTS (SELECT 1 FROM pg_attribute a WHERE a.attrelid = c.oid \
		 AND a.attname = 'tenant' AND NOT a.attisdropped), \
		 c.relrowsecurity, c.relforcerowsecurity, \
		 (SELECT count(*) FROM pg_policy p WHERE p.polrelid = c.oid) \
		 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
		 WHERE c.relkind IN ('r', 'p') AND n.nspname = 'public' \
		 ORDER BY 1",
	)
	.fetch_all(pool)
	.await
	.unwrap();
	let policies: Vec<(String, String, String, Vec<String>, String, String)> = sqlx::query_as(
		"SELECT tablename::text, policyname::text, cmd, roles::text[], \
		 coalesce(qual, ''), coalesce(with_check, '') FROM pg_policies \
		 WHERE schemaname = 'public' AND permissive = 'PERMISSIVE'",
	)
	.fetch_all(pool)
	.await
	.unwrap();
	rows.into_iter()
		.map(|(name, tenant_column, row_security, forced, count)| {
			let mut own = policies.iter().filter(|policy| policy.0 == name);
			let policy = match (own.next(), own.next()) {
				(Some((_, policy, command, roles, using, check)), None) => Some((
					policy.clone(),
					command.clone(),
					roles.clone(),
					using.clone(),
					check.clone(),
				)),
				_ => None,
			};
			(
				name,
				Observed {
					tenant_column,
					row_security,
					forced,
					policies: count,
					policy,
				},
			)
		})
		.collect()
}

/// Compare `tables`, every table of the build under test, with the database
/// `pool` reaches. `list_file` is where the list that failed lives, named in
/// the failure.
#[allow(dead_code)] // Unit tests of the comparison use `compare` alone.
pub async fn check(pool: &sqlx::PgPool, tables: &[Table], list_file: &str) {
	compare(&observe(pool).await, tables, list_file);
	for table in tables {
		if table.classification != Classification::DeploymentWide {
			aidash_server::database::classification::node::ownership(table.name).unwrap();
		}
	}
}

/// Panic unless `tables` and `observed` name the same tables, once each, and
/// every table's row-level security matches its classification.
pub fn compare(observed: &BTreeMap<String, Observed>, tables: &[Table], list_file: &str) {
	// The list and the database name the same tables, once each.
	let listed = tables.iter().map(|table| table.name).collect::<Vec<_>>();
	let unique = listed.iter().copied().collect::<BTreeSet<_>>();
	assert_eq!(unique.len(), listed.len(), "a table is listed twice");
	let names = observed.keys().map(String::as_str).collect::<BTreeSet<_>>();
	let unlisted = names.difference(&unique).collect::<Vec<_>>();
	let missing = unique.difference(&names).collect::<Vec<_>>();
	assert!(
		unlisted.is_empty() && missing.is_empty(),
		"classify every table in {list_file}.\n\
		 unlisted: {unlisted:?}\nmissing from the database: {missing:?}\n{}",
		observed
			.iter()
			.map(|(name, table)| format!("{name} tenant={}", table.tenant_column))
			.collect::<Vec<_>>()
			.join("\n")
	);
	// Every enforced table carries the same single policy, rendered from one
	// template (server/src/database/policy.rs).
	let mut template = None;
	for table in tables {
		let actual = &observed[table.name];
		let enforced = actual.row_security && actual.forced && actual.policies > 0;
		match table.classification {
			Classification::TenantOwned => {
				assert!(actual.tenant_column, "{} has no tenant column", table.name);
				assert!(enforced, "{}: {actual:?}", table.name);
				assert_eq!(actual.policies, 1, "{}: {actual:?}", table.name);
				let (name, command, roles, using, check) = actual
					.policy
					.clone()
					.unwrap_or_else(|| panic!("{}: {actual:?}", table.name));
				assert_eq!(name, policy::POLICY, "{}", table.name);
				assert_eq!(command, "ALL", "{}", table.name);
				assert_eq!(roles, ["public"], "{}", table.name);
				assert_eq!(using, check, "{}", table.name);
				assert!(
					using.contains("aidash_deployment_scope()")
						&& using.contains("aidash_scope_tenant()"),
					"{}: {using}",
					table.name
				);
				let expected = template.get_or_insert_with(|| using.clone());
				assert_eq!(&using, expected, "{} departs from the template", table.name);
			}
			Classification::DeploymentWide => assert!(
				!actual.row_security && actual.policies == 0,
				"{} is Deployment-wide but has row-level security: {actual:?}",
				table.name
			),
		}
	}
}
