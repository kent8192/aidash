//! Tenancy scopes and the table classification (#263) on a migrated database.
use super::native_database::{DatabaseFixture, database};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_server::database::classification::{Classification, TABLES};
use aidash_server::database::{native, runtime};
use rstest::rstest;
use std::collections::{BTreeMap, BTreeSet};

#[path = "../../../database/classification/comparison.rs"]
mod comparison;

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
	// Act + Assert
	comparison::check(&pool, TABLES, "server/src/database/classification.rs").await;
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
