//! Tenancy scopes and the table classification (#263) on a migrated database.
use super::native_database::{DatabaseFixture, database, owner_pool};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_server::database::classification::{self, Classification, TABLES};
use aidash_server::database::purge::{self, Reference};
use aidash_server::database::{native, policy, runtime};
use rstest::rstest;
use std::collections::{BTreeMap, BTreeSet};

/// One table as the database sees it.
#[derive(Debug)]
struct Observed {
	tenant_column: bool,
	row_security: bool,
	forced: bool,
	policies: i64,
	/// The one policy's name, command, roles, USING and WITH CHECK, when the
	/// table has exactly one permissive policy.
	policy: Option<(String, String, Vec<String>, String, String)>,
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

#[rstest]
#[tokio::test]
async fn every_table_is_classified_and_matches_its_policies(#[future] database: DatabaseFixture) {
	// Arrange
	let f = database.await;
	let pool = runtime::connect(&f.url, 2)
		.await
		.unwrap()
		.into_postgres()
		.unwrap();
	// Act
	let observed = observe(&pool).await;
	// Assert: the list and the database name the same tables, once each.
	let listed = TABLES.iter().map(|table| table.name).collect::<Vec<_>>();
	let unique = listed.iter().copied().collect::<BTreeSet<_>>();
	assert_eq!(unique.len(), listed.len(), "a table is listed twice");
	let tables = observed.keys().map(String::as_str).collect::<BTreeSet<_>>();
	let unlisted = tables.difference(&unique).collect::<Vec<_>>();
	let missing = unique.difference(&tables).collect::<Vec<_>>();
	assert!(
		unlisted.is_empty() && missing.is_empty(),
		"classify every table in server/src/database/classification.rs.\n\
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
	for table in TABLES {
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
			Classification::Pending(issue) => {
				assert!(
					!actual.row_security && actual.policies == 0,
					"{} is enforced; classify it Tenant-owned: {actual:?}",
					table.name
				);
				// #241 gives Nodes and peers a Tenant.
				assert_eq!(issue, 241, "{}: no step of #223 is #{issue}", table.name);
			}
			Classification::DeploymentWide => assert!(
				!actual.row_security && actual.policies == 0,
				"{} is Deployment-wide but has row-level security: {actual:?}",
				table.name
			),
		}
	}
}

/// The tables whose `tenant` stays nullable. A Deployment Administrator's
/// decision on a request that addresses no Tenant keeps a row without one
/// (#264, #265), and so does an event that concerns the Deployment, such as a
/// peer registration, until #241 gives peers a Tenant.
const NULLABLE_TENANT: &[&str] = &[
	"dashboard_administration_history",
	"events",
	"research_revocations",
];

/// A foreign key as the catalog lists it.
struct ForeignKey {
	table: String,
	name: String,
	columns: Vec<String>,
	parent: String,
	parent_columns: Vec<String>,
}

async fn foreign_keys(pool: &sqlx::PgPool) -> Vec<ForeignKey> {
	// Catalog introspection that Reinhardt Query has no builder for.
	type Row = (String, String, Vec<String>, String, Vec<String>);
	let rows: Vec<Row> = sqlx::query_as(
		"SELECT child.relname::text, c.conname::text, \
		 ARRAY(SELECT a.attname::text FROM unnest(c.conkey) WITH ORDINALITY k(n, o) \
		 JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.n ORDER BY k.o), \
		 parent.relname::text, \
		 ARRAY(SELECT a.attname::text FROM unnest(c.confkey) WITH ORDINALITY k(n, o) \
		 JOIN pg_attribute a ON a.attrelid = c.confrelid AND a.attnum = k.n ORDER BY k.o) \
		 FROM pg_constraint c \
		 JOIN pg_class child ON child.oid = c.conrelid \
		 JOIN pg_class parent ON parent.oid = c.confrelid \
		 WHERE c.contype = 'f' AND c.connamespace = 'public'::regnamespace ORDER BY 1, 2",
	)
	.fetch_all(pool)
	.await
	.unwrap();
	rows.into_iter()
		.map(
			|(table, name, columns, parent, parent_columns)| ForeignKey {
				table,
				name,
				columns,
				parent,
				parent_columns,
			},
		)
		.collect()
}

#[rstest]
#[tokio::test]
async fn tenant_owned_tables_reference_the_tenant_root_and_agree_with_their_parents(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let f = database.await;
	let pool = runtime::connect(&f.url, 2)
		.await
		.unwrap()
		.into_postgres()
		.unwrap();
	let owned = TABLES
		.iter()
		.filter(|table| table.classification == Classification::TenantOwned)
		.map(|table| table.name)
		.collect::<BTreeSet<_>>();
	// Act
	let not_null: Vec<(String, bool)> = sqlx::query_as(
		"SELECT c.relname::text, a.attnotnull FROM pg_attribute a \
		 JOIN pg_class c ON c.oid = a.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
		 WHERE a.attname = 'tenant' AND NOT a.attisdropped AND c.relkind = 'r' \
		 AND n.nspname = 'public'",
	)
	.fetch_all(&pool)
	.await
	.unwrap();
	let not_null = not_null.into_iter().collect::<BTreeMap<_, _>>();
	let keys = foreign_keys(&pool).await;
	// Assert: every Tenant-owned table names its Tenant, and the Tenant root
	// record survives purge as a tombstone for it to reference.
	for table in &owned {
		let required = !NULLABLE_TENANT.contains(table);
		assert_eq!(
			not_null.get(*table),
			Some(&required),
			"{table}: `tenant` is NOT NULL unless the table is a #264 exception"
		);
		assert!(
			keys.iter().any(|key| key.table == *table
				&& key.columns == ["tenant"]
				&& key.parent == "tenants"
				&& key.parent_columns == ["name"]),
			"{table} has no foreign key from `tenant` to tenants(name)"
		);
	}
	// Assert: a child keeps its parent's Tenant, so every reference between
	// two Tenant-owned tables compares the Tenants too.
	for key in keys
		.iter()
		.filter(|key| owned.contains(key.table.as_str()) && owned.contains(key.parent.as_str()))
	{
		let child = key.columns.iter().position(|column| column == "tenant");
		let parent = key
			.parent_columns
			.iter()
			.position(|column| column == "tenant");
		assert!(
			child.is_some() && child == parent,
			"{} ({}): {:?} -> {}{:?} lets a child name another Tenant than its parent",
			key.table,
			key.name,
			key.columns,
			key.parent,
			key.parent_columns
		);
	}
}

async fn settings(tx: &mut native::Transaction) -> (String, String, String, String) {
	native::query_as::<(String, String, String, String)>(
		"SELECT coalesce(current_setting('aidash.scope', true), '') AS scope, \
		 coalesce(current_setting('aidash.tenant', true), '') AS tenant, \
		 current_setting('application_name') AS application, \
		 current_setting('transaction_read_only') AS read_only",
	)
	.columns(&["scope", "tenant", "application", "read_only"])
	.fetch_one(&mut **tx)
	.await
	.unwrap()
}

#[rstest]
#[case::tenant(TenancyScope::tenant("acme_team:one").unwrap(), ("tenant", "acme_team:one", false, "off"))]
#[case::no_tenant(TenancyScope::NoTenant, ("none", "", false, "off"))]
#[case::deployment_write(
	TenancyScope::Deployment(DeploymentScopeReason::Retention),
	("deployment", "", true, "off")
)]
#[case::deployment_read(
	TenancyScope::Deployment(DeploymentScopeReason::DeploymentAdministratorList),
	("deployment", "", true, "on")
)]
#[tokio::test]
async fn a_transaction_carries_its_scope_until_it_ends(
	#[future] database: DatabaseFixture,
	#[case] scope: TenancyScope,
	#[case] expected: (&str, &str, bool, &str),
) {
	// Arrange: one connection, so the next transaction reuses it.
	let f = database.await;
	let pool = native::Pool::from(
		runtime::connect(&f.url, 1)
			.await
			.unwrap()
			.into_postgres()
			.unwrap(),
	);
	// Act
	let mut tx = pool.begin(&scope).await.unwrap();
	let inside = settings(&mut tx).await;
	tx.commit().await.unwrap();
	let after = pool
		.scoped(&TenancyScope::NoTenant, async |tx| Ok(settings(tx).await))
		.await
		.unwrap();
	// Assert
	let (kind, tenant, deployment, read_only) = expected;
	assert_eq!(inside.0, kind);
	assert_eq!(inside.1, tenant);
	let label = match &scope {
		TenancyScope::Deployment(reason) => format!("deployment:{}", reason.label()),
		_ => String::new(),
	};
	assert_eq!(deployment, !label.is_empty());
	if deployment {
		assert_eq!(inside.2, label);
	} else {
		assert!(!inside.2.starts_with("deployment:"), "{}", inside.2);
	}
	assert_eq!(inside.3, read_only);
	assert_eq!((after.0.as_str(), after.1.as_str()), ("none", ""));
	assert!(!after.2.starts_with("deployment:"), "{}", after.2);
	assert_eq!(after.3, "off");
}

#[rstest]
#[tokio::test]
async fn a_read_only_reason_refuses_writes_and_row_locks(#[future] database: DatabaseFixture) {
	// Arrange
	let f = database.await;
	let pool = native::Pool::from(
		runtime::connect(&f.url, 1)
			.await
			.unwrap()
			.into_postgres()
			.unwrap(),
	);
	let scope = TenancyScope::Deployment(DeploymentScopeReason::DeploymentAdministratorList);
	for statement in [
		"DELETE FROM inbox WHERE false",
		"SELECT 1 FROM inbox FOR UPDATE",
	] {
		// Act
		let refused = pool
			.scoped(&scope, async |tx| {
				native::query(statement).execute(&mut **tx).await?;
				Ok(())
			})
			.await;
		// Assert: SQLSTATE 25006, read_only_sql_transaction.
		let error = refused.expect_err(statement).to_string();
		assert!(
			error.contains("read-only transaction"),
			"{statement}: {error}"
		);
	}
}

#[rstest]
#[tokio::test]
async fn scoped_work_returns_its_error_and_orm_work_carries_the_scope(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let f = database.await;
	let pool = native::Pool::from(
		runtime::connect(&f.url, 1)
			.await
			.unwrap()
			.into_postgres()
			.unwrap(),
	);
	let scope = TenancyScope::tenant("acme").unwrap();
	// Act: a failing unit of work after a statement that took effect.
	let failed = pool
		.scoped(&scope, async |tx| {
			native::query("SELECT set_config('aidash.probe', 'kept', true)")
				.execute(&mut **tx)
				.await?;
			Err::<(), _>(aidash_server::Error::Conflict("probe".into()))
		})
		.await;
	let orm = pool
		.atomic(&scope, async |tx| {
			native::query_scalar::<String>("SELECT current_setting('aidash.tenant', true)")
				.scalar_one(tx)
				.await
		})
		.await
		.unwrap();
	// Assert
	assert!(matches!(failed, Err(aidash_server::Error::Conflict(_))));
	assert_eq!(orm, "acme");
}

/// The Deployment-wide tables that carry a `tenant` column without being
/// Tenant data, and why purge leaves them. Anything else with a `tenant` column
/// must be classified Tenant-owned, so purge deletes it or declares it kept.
const DEPLOYMENT_WIDE_TENANT_REFERENCES: &[(&str, &str)] = &[
	(
		"dashboard_registration_requests",
		"a request holds only an identity's id and expires on its own; purge only detaches its Invitation",
	),
	(
		"tenant_deletions",
		"the deletion record outlives purge (ADR 0060)",
	),
];

async fn tables_with_a_tenant_column(pool: &sqlx::PgPool) -> BTreeSet<String> {
	// Catalog introspection that Reinhardt Query has no builder for.
	sqlx::query_scalar::<_, String>(
		"SELECT c.relname::text FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid \
		 JOIN pg_namespace n ON n.oid = c.relnamespace \
		 WHERE a.attname = 'tenant' AND NOT a.attisdropped AND c.relkind IN ('r', 'p') \
		 AND n.nspname = 'public'",
	)
	.fetch_all(pool)
	.await
	.unwrap()
	.into_iter()
	.collect()
}

#[rstest]
#[tokio::test]
async fn purge_covers_every_tenant_table_and_nothing_it_keeps_blocks_it(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let f = database.await;
	let pool = runtime::connect(&f.url, 2)
		.await
		.unwrap()
		.into_postgres()
		.unwrap();
	let references: Vec<Reference> = native::Pool::from(pool.clone())
		.atomic(&TenancyScope::NoTenant, async |tx| {
			purge::references(tx).await
		})
		.await
		.unwrap();
	let purged = classification::purged().collect::<BTreeSet<_>>();
	let audit = classification::audit_history().collect::<BTreeSet<_>>();
	// Act
	let order = purge::deletion_order(
		classification::purged(),
		&references,
		classification::UNLINKED,
	)
	.unwrap();
	let audit_order =
		purge::deletion_order(classification::audit_history(), &references, &[]).unwrap();
	let with_tenant = tables_with_a_tenant_column(&pool).await;
	// Assert: purge orders every purged table, children before parents.
	assert_eq!(order.iter().copied().collect::<BTreeSet<_>>(), purged);
	assert_eq!(audit_order.iter().copied().collect::<BTreeSet<_>>(), audit);
	let position = |table: &str| order.iter().position(|name| *name == table);
	let unlinked = |reference: &Reference| {
		classification::UNLINKED.iter().any(|(table, column)| {
			*table == reference.child && reference.columns.iter().any(|name| name == column)
		})
	};
	for reference in references
		.iter()
		.filter(|reference| !reference.cascades && !unlinked(reference))
	{
		if let (Some(child), Some(parent)) =
			(position(&reference.child), position(&reference.parent))
		{
			assert!(
				child <= parent,
				"{} references {} but would be deleted after it",
				reference.child,
				reference.parent
			);
		}
	}
	// Assert: every unlinked column is nullable, belongs to a foreign key, and
	// is needed to order the purge.
	for (index, (table, column)) in classification::UNLINKED.iter().enumerate() {
		let nullable: Option<bool> = sqlx::query_scalar(
			"SELECT is_nullable = 'YES' FROM information_schema.columns \
			 WHERE table_schema = 'public' AND table_name = $1 AND column_name = $2",
		)
		.bind(table)
		.bind(column)
		.fetch_optional(&pool)
		.await
		.unwrap();
		assert_eq!(
			nullable,
			Some(true),
			"{table}.{column} must be a nullable column"
		);
		assert!(
			references.iter().any(|reference| reference.child == *table
				&& reference.columns.iter().any(|name| name == column)),
			"{table}.{column} is part of no foreign key"
		);
		let others = classification::UNLINKED
			.iter()
			.enumerate()
			.filter(|(other, _)| *other != index)
			.map(|(_, unlink)| *unlink)
			.collect::<Vec<_>>();
		let closes_a_cycle =
			purge::deletion_order(classification::purged(), &references, &others).is_err();
		let detaches_a_kept_table = !classification::purged().any(|purged| purged == *table);
		assert!(
			closes_a_cycle || detaches_a_kept_table,
			"{table}.{column} is needed for neither; remove it from UNLINKED"
		);
	}
	// Assert: no table purge keeps references a table it deletes, or the
	// purge would fail on the reference. Audit history therefore holds no
	// foreign key into data that goes first.
	for reference in references.iter().filter(|reference| !unlinked(reference)) {
		if purged.contains(reference.parent.as_str()) {
			assert!(
				purged.contains(reference.child.as_str()),
				"{} references {}, which purge deletes, but purge keeps {}",
				reference.child,
				reference.parent,
				reference.child
			);
		}
	}
	// Assert: a table with a `tenant` column is purged, kept as audit history
	// or a tombstone, or listed above as Deployment-wide on purpose.
	let allowed = DEPLOYMENT_WIDE_TENANT_REFERENCES
		.iter()
		.map(|(table, _)| *table)
		.collect::<BTreeSet<_>>();
	let undeclared = TABLES
		.iter()
		.filter(|table| with_tenant.contains(table.name))
		.filter(|table| match table.classification {
			Classification::TenantOwned | Classification::Pending(_) => false,
			Classification::DeploymentWide => !table.names_tenant && !allowed.contains(table.name),
		})
		.map(|table| table.name)
		.collect::<Vec<_>>();
	assert!(
		undeclared.is_empty(),
		"{undeclared:?} have a `tenant` column but are neither Tenant-owned nor a declared \
		 Deployment-wide reference; purge would leave their rows behind"
	);
	for table in allowed {
		assert!(
			with_tenant.contains(table),
			"{table} no longer has a `tenant` column"
		);
	}
	for table in TABLES.iter().filter(|table| table.names_tenant) {
		assert!(
			with_tenant.contains(table.name),
			"{} is purged by its `tenant` column but has none",
			table.name
		);
	}
}

#[rstest]
#[case::tenant(TenancyScope::tenant("acme").unwrap(), false)]
#[case::retention(TenancyScope::Deployment(DeploymentScopeReason::Retention), false)]
#[case::purge(TenancyScope::Deployment(DeploymentScopeReason::TenantPurge), true)]
#[tokio::test]
async fn only_a_tenant_purge_deletes_rows_that_triggers_keep_for_their_parents(
	#[future] database: DatabaseFixture,
	#[case] scope: TenancyScope,
	#[case] allowed: bool,
) {
	// Arrange
	let f = database.await;
	let pool = native::Pool::from(
		runtime::connect(&f.url, 1)
			.await
			.unwrap()
			.into_postgres()
			.unwrap(),
	);
	for table in [
		"task_dependencies",
		"registry_agent_model_refs",
		"registry_agent_resource_refs",
		"registry_memory_role_refs",
	] {
		// Act: statement triggers fire without a row.
		let deleted = pool
			.scoped(&scope, async |tx| {
				native::query(&format!("DELETE FROM {table} WHERE tenant = 'acme'"))
					.execute(&mut **tx)
					.await?;
				Ok(())
			})
			.await;
		// Assert
		match (allowed, deleted) {
			(true, result) => result.unwrap_or_else(|error| panic!("{table}: {error}")),
			(false, result) => {
				let error = result.expect_err(table).to_string();
				assert!(error.contains("maintained by"), "{table}: {error}");
			}
		}
	}
}

#[rstest]
#[case::tenant(TenancyScope::tenant("acme").unwrap(), "DELETE", false)]
#[case::purge_deletes(
	TenancyScope::Deployment(DeploymentScopeReason::TenantPurge),
	"DELETE",
	true
)]
#[case::purge_never_updates(
	TenancyScope::Deployment(DeploymentScopeReason::TenantPurge),
	"UPDATE",
	false
)]
#[tokio::test]
async fn immutable_marketplace_content_goes_only_with_a_purged_tenant(
	#[future] database: DatabaseFixture,
	#[case] scope: TenancyScope,
	#[case] statement: &str,
	#[case] allowed: bool,
) {
	// Arrange: a table guarded by the same trigger function, as only its owner may create.
	let f = database.await;
	let pool = native::Pool::from(
		runtime::connect(&f.url, 1)
			.await
			.unwrap()
			.into_postgres()
			.unwrap(),
	);
	let owner = owner_pool(pool.driver()).await;
	for ddl in [
		"CREATE TABLE fixture_immutable (tenant text)",
		"CREATE TRIGGER marketplace_immutable BEFORE DELETE OR UPDATE ON fixture_immutable \
		 FOR EACH ROW EXECUTE FUNCTION public.marketplace_immutable()",
		"INSERT INTO fixture_immutable VALUES ('acme')",
		"GRANT ALL ON fixture_immutable TO PUBLIC",
	] {
		sqlx::query(ddl).execute(&owner).await.unwrap();
	}
	let sql = match statement {
		"DELETE" => "DELETE FROM fixture_immutable WHERE tenant = 'acme'",
		_ => "UPDATE fixture_immutable SET tenant = 'acme' WHERE tenant = 'acme'",
	};
	// Act
	let result = pool
		.scoped(&scope, async |tx| {
			native::query(sql).execute(&mut **tx).await?;
			Ok(())
		})
		.await;
	// Assert
	match (allowed, result) {
		(true, result) => result.unwrap(),
		(false, result) => {
			let error = result.expect_err(statement).to_string();
			assert!(error.contains("immutable"), "{error}");
		}
	}
}
