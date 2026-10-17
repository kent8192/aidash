//! Tenancy scopes and the table classification (#263) on a migrated database.
use super::native_database::{DatabaseFixture, database};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_server::database::classification::{Classification, TABLES};
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
				// #265 copies the Tenant onto the tables that reach it only
				// through a parent.
				match issue {
					265 => assert!(!actual.tenant_column, "{} has a tenant column", table.name),
					241 => {}
					other => panic!("{}: no step of #223 is #{other}", table.name),
				}
			}
			Classification::DeploymentWide => assert!(
				!actual.row_security && actual.policies == 0,
				"{} is Deployment-wide but has row-level security: {actual:?}",
				table.name
			),
		}
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
