//! Purging deleted Tenants and keeping audit history (#239, ADR 0061), over
//! HTTP and PostgreSQL.
use super::*;
use aidash_server::apps::identity::services::tenant_purge;
use aidash_server::database::classification::{self, TABLES};
use std::collections::BTreeMap;

const AUDIT_DAYS: u32 = 365;

impl Deployment {
	/// A Tenant with a Workspace and a Task, as its first administrator made them.
	async fn tenant_with_work(&self, person: &Person, label: &str) -> (String, String) {
		let (tenant, context) = self.tenant_of(person, label).await;
		let (status, workspace) = self
			.as_person(
				person,
				"POST",
				"/api/workspaces",
				Some(&context),
				json!({"title":"Work","goal":"Keep working"}),
			)
			.await;
		assert_eq!(status, 200, "{workspace}");
		let workspace = workspace["id"].as_str().unwrap().to_owned();
		let (status, task) = self
			.as_person(
				person,
				"POST",
				&format!("/api/workspaces/{workspace}/tasks"),
				Some(&context),
				json!({"title":"work","description":"keep working"}),
			)
			.await;
		assert_eq!(status, 200, "{task}");
		// Rows that only their parent maintains, and that triggers guard.
		let (status, dependent) = self
			.as_person(
				person,
				"POST",
				&format!("/api/workspaces/{workspace}/tasks"),
				Some(&context),
				json!({"title":"after","description":"wait","dependencies":[task["id"]]}),
			)
			.await;
		assert_eq!(status, 200, "{dependent}");
		(tenant, context)
	}

	/// The number of rows `tenant` owns in each table that has a `tenant` column.
	async fn rows_of(&self, tenant: &str) -> BTreeMap<&'static str, i64> {
		let mut counts = BTreeMap::new();
		for table in TABLES {
			let has_tenant: bool = sqlx::query_scalar(
				"SELECT EXISTS (SELECT 1 FROM information_schema.columns \
				 WHERE table_schema = 'public' AND table_name = $1 AND column_name = 'tenant')",
			)
			.bind(table.name)
			.fetch_one(self.federation.store.pool.fixture_driver())
			.await
			.unwrap();
			if !has_tenant {
				continue;
			}
			let count = sqlx::query_scalar(&format!(
				"SELECT count(*) FROM {} WHERE tenant = $1",
				table.name
			))
			.bind(tenant)
			.fetch_one(self.federation.store.pool.fixture_driver())
			.await
			.unwrap();
			counts.insert(table.name, count);
		}
		counts
	}

	/// Ends the grace period of a deleted Tenant, as time passing would.
	async fn grace_period_ends(&self, tenant: &str) {
		sqlx::query(
			"UPDATE tenant_deletions SET purge_after = deleted_at + interval '1 microsecond' \
			 WHERE tenant = $1 AND restored_at IS NULL",
		)
		.bind(tenant)
		.execute(self.federation.store.pool.fixture_driver())
		.await
		.unwrap();
	}

	async fn sweep(&self) {
		tenant_purge::sweep(&self.federation.store, AUDIT_DAYS)
			.await
			.unwrap();
	}

	async fn deployment_tenant(&self, tenant: &str) -> Value {
		let (status, list) = self
			.as_deployment_administrator("GET", "/api/tenants", Value::Null)
			.await;
		assert_eq!(status, 200, "{list}");
		list.as_array()
			.unwrap()
			.iter()
			.find(|row| row["name"] == tenant)
			.unwrap_or_else(|| panic!("{tenant} is not listed: {list}"))
			.clone()
	}
}

#[rstest::rstest]
#[tokio::test]
async fn purge_deletes_the_tenants_rows_and_leaves_other_tenants_and_audit_history(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	// Arrange: two Tenants with work, one of them deleted.
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let (tenant, context) = d.tenant_with_work(&alice, "Team").await;
	let (neighbour, _) = d.tenant_with_work(&bob, "Other").await;
	// A request made with the Tenant's Invitation, and a pending authorization
	// flow of the Tenant: Deployment-wide rows that name it.
	let carol = d.person("carol").await;
	let (status, invitation) = d
		.as_person(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/invitations"),
			Some(&context),
			json!({}),
		)
		.await;
	assert_eq!(status, 201, "{invitation}");
	let (status, request) = d
		.as_person(
			&carol,
			"POST",
			"/auth/invitations/redeem",
			None,
			json!({"tenant":tenant,"secret":invitation["secret"]}),
		)
		.await;
	assert_eq!(status, 200, "{request}");
	sqlx::query(
		"INSERT INTO provider_authorizations (state_hash, browser_hash, session_id, context, tenant, \
		 provider, provider_credential_id, pkce_verifier, callback_uri, return_to, expires_at) \
		 VALUES ('state', 'browser', $1, 'ctx', $2, 'openrouter', $1, 'verifier', 'http://x/cb', '/', now() + interval '1 hour')",
	)
	.bind(Uuid::new_v4())
	.bind(&tenant)
	.execute(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	let (status, body) = d.delete(&alice, &context, &tenant).await;
	assert_eq!(status, 200, "{body}");
	let before = d.rows_of(&tenant).await;
	let neighbour_before = d.rows_of(&neighbour).await;
	for table in [
		"workspaces",
		"tasks",
		"task_dependencies",
		"registry",
		"authorization_bundles",
		"events",
	] {
		assert!(before[table] > 0, "{table} has nothing to purge");
	}
	for table in classification::audit_history() {
		if [
			"authorization_revisions",
			"authorization_decisions",
			"dashboard_administration_history",
		]
		.contains(&table)
		{
			assert!(before[table] > 0, "{table} has no history to keep");
		}
	}
	d.grace_period_ends(&tenant).await;

	// Act
	d.sweep().await;

	// Assert: every purged table is empty for the Tenant, audit history stays,
	// and the neighbour lost nothing.
	let after = d.rows_of(&tenant).await;
	for table in classification::purged() {
		assert_eq!(after[table], 0, "{table} still holds the Tenant's rows");
	}
	for table in classification::audit_history() {
		assert_eq!(after[table], before[table], "{table} lost audit history");
	}
	assert_eq!(d.rows_of(&neighbour).await, neighbour_before);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM dashboard_registration_requests WHERE tenant = '{tenant}' AND invitation_id IS NULL"
		))
		.await,
		1,
		"the request stays, detached from its Invitation"
	);
	let record = d.deployment_tenant(&tenant).await;
	assert!(record["label"].is_null(), "{record}");
	assert!(!record["deletion"]["purged_at"].is_null(), "{record}");
	assert_eq!(record["purgeable"], false, "{record}");
	assert_eq!(
		d.deletions(&tenant, "purge_step IS NULL AND purge_failures = 0")
			.await,
		1
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_purged_tenant_stays_a_tombstone_that_nobody_restores(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.tenant_with_work(&alice, "Team").await;
	let (status, body) = d.delete(&alice, &context, &tenant).await;
	assert_eq!(status, 200, "{body}");
	d.grace_period_ends(&tenant).await;
	d.sweep().await;

	// The root record keeps the name, so it is never reused, and refuses a restore.
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM tenants WHERE name = '{tenant}'"
		))
		.await,
		1
	);
	let (status, body) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/tenants/{tenant}/restore"),
			Value::Null,
		)
		.await;
	assert_eq!(status, 409, "{body}");
	assert!(body.to_string().contains("tenant_purged"), "{body}");
	let (status, body) = d.restore(&alice, &context, &tenant).await;
	assert!(status == 403 || status == 409, "{status} {body}");
	let (status, body) = d
		.as_deployment_administrator("POST", &format!("/api/tenants/{tenant}/purge"), Value::Null)
		.await;
	assert_eq!(status, 409, "{body}");
	// A second sweep finds nothing to do.
	d.sweep().await;
	assert_eq!(d.deletions(&tenant, "purged_at IS NOT NULL").await, 1);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_failed_purge_step_is_retried_from_itself_and_a_purge_that_keeps_failing_is_stalled(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	// Arrange: a foreign key from outside the Tenant's data blocks deleting its
	// Workspaces until it is dropped. Only the table owner changes the schema.
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.tenant_with_work(&alice, "Team").await;
	let (status, body) = d.delete(&alice, &context, &tenant).await;
	assert_eq!(status, 200, "{body}");
	d.grace_period_ends(&tenant).await;
	let pool = d.federation.store.pool.fixture_driver();
	let owner = common::owner_pool(pool).await;
	sqlx::query(
		"CREATE TABLE fixture_purge_guard (id UUID PRIMARY KEY REFERENCES workspaces(id) ON DELETE RESTRICT)",
	)
	.execute(&owner)
	.await
	.unwrap();
	sqlx::query("INSERT INTO fixture_purge_guard SELECT id FROM workspaces WHERE tenant = $1")
		.bind(&tenant)
		.execute(pool)
		.await
		.unwrap();

	// Act: three sweeps fail at the same step.
	for _ in 0..3 {
		d.sweep().await;
	}

	// Assert: the purge waits at the failed step and is shown as stalled with its error.
	let record = d.deployment_tenant(&tenant).await;
	assert_eq!(record["purge"]["step"], "rows", "{record}");
	assert_eq!(record["purge"]["failures"], 3, "{record}");
	assert_eq!(record["purge"]["stalled"], true, "{record}");
	assert!(
		record["purge"]["error"]
			.as_str()
			.unwrap()
			.contains("fixture_purge_guard"),
		"{record}"
	);
	assert!(record["deletion"]["purged_at"].is_null(), "{record}");
	// The Tenant is still unusable while its purge waits.
	let (status, body) = d.delete(&alice, &context, &tenant).await;
	assert_ne!(status, 200, "{body}");

	// Act: the cause goes away.
	sqlx::query("DROP TABLE fixture_purge_guard")
		.execute(&owner)
		.await
		.unwrap();
	d.sweep().await;

	// Assert: the next sweep resumes and finishes.
	let record = d.deployment_tenant(&tenant).await;
	assert!(!record["deletion"]["purged_at"].is_null(), "{record}");
	assert!(record["purge"].is_null(), "{record}");
	assert_eq!(d.rows_of(&tenant).await["workspaces"], 0);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn only_a_deployment_administrator_purges_a_deleted_tenant_at_once(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.tenant_with_work(&alice, "Team").await;
	let path = format!("/api/tenants/{tenant}/purge");

	// A Tenant is not purged while it is in use, and a Tenant Administrator never purges.
	let (status, body) = d
		.as_deployment_administrator("POST", &path, Value::Null)
		.await;
	assert_eq!(status, 409, "{body}");
	let (status, body) = d.delete(&alice, &context, &tenant).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = d
		.as_person(&alice, "POST", &path, Some(&context), Value::Null)
		.await;
	assert!([401, 403, 410].contains(&status), "{status} {body}");
	assert!(
		d.deployment_tenant(&tenant).await["purgeable"]
			.as_bool()
			.unwrap()
	);

	// A Deployment Administrator skips the grace period; the Tenant is purged
	// shortly after and can no longer be restored.
	let (status, deletion) = d
		.as_deployment_administrator("POST", &path, Value::Null)
		.await;
	assert_eq!(status, 200, "{deletion}");
	let purge_after: chrono::DateTime<chrono::Utc> =
		serde_json::from_value(deletion["purge_after"].clone()).unwrap();
	// The database clock may differ a little from this host's.
	assert!(purge_after <= chrono::Utc::now() + chrono::Duration::minutes(5));
	let (status, body) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/tenants/{tenant}/restore"),
			Value::Null,
		)
		.await;
	assert_eq!(status, 409, "{body}");
	let mut purged = false;
	for _ in 0..100 {
		d.sweep().await;
		if d.deletions(&tenant, "purged_at IS NOT NULL").await == 1 {
			purged = true;
			break;
		}
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	assert!(purged, "the purge never finished");
	assert_eq!(d.rows_of(&tenant).await["workspaces"], 0);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn audit_history_is_kept_for_its_own_period_and_then_deleted_with_the_purge_loop(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	// Arrange: one purged Tenant, and a neighbour whose history must stay.
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let (tenant, context) = d.tenant_with_work(&alice, "Team").await;
	let (neighbour, _) = d.tenant_with_work(&bob, "Other").await;
	let (status, body) = d.delete(&alice, &context, &tenant).await;
	assert_eq!(status, 200, "{body}");
	d.grace_period_ends(&tenant).await;
	d.sweep().await;
	let kept = d.rows_of(&tenant).await;
	let neighbour_before = d.rows_of(&neighbour).await;
	assert!(
		classification::audit_history().any(|table| kept[table] > 0),
		"nothing was kept"
	);

	// Act: the period has not ended yet.
	d.sweep().await;
	// Assert
	assert_eq!(d.rows_of(&tenant).await, kept);

	// Act: the period ends.
	sqlx::query(
		"UPDATE tenant_deletions SET purged_at = now() - make_interval(days => $2) WHERE tenant = $1",
	)
	.bind(&tenant)
	.bind(i32::try_from(AUDIT_DAYS + 1).unwrap())
	.execute(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	d.sweep().await;

	// Assert: the Tenant's audit history is gone, once, and the neighbour's stays.
	let after = d.rows_of(&tenant).await;
	for table in classification::audit_history() {
		assert_eq!(after[table], 0, "{table} outlived its period");
	}
	assert_eq!(d.rows_of(&neighbour).await, neighbour_before);
	assert_eq!(d.deletions(&tenant, "audit_erased_at IS NOT NULL").await, 1);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM tenants WHERE name = '{tenant}'"
		))
		.await,
		1
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_restore_is_refused_while_a_purge_owns_the_tenant(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.tenant_with_work(&alice, "Team").await;
	let (status, body) = d.delete(&alice, &context, &tenant).await;
	assert_eq!(status, 200, "{body}");
	let path = format!("/api/tenants/{tenant}/restore");
	let pool = d.federation.store.pool.fixture_driver();

	// A purge worker's lease, and a step that already ran, each keep a restore out
	// even though the grace period has not ended.
	for claim in [
		"purge_lease = gen_random_uuid(), purge_leased_until = now() + interval '10 minutes'",
		"purge_step = 'rows'",
	] {
		sqlx::query(&format!(
			"UPDATE tenant_deletions SET {claim} WHERE tenant = $1 AND restored_at IS NULL"
		))
		.bind(&tenant)
		.execute(pool)
		.await
		.unwrap();
		let (status, body) = d
			.as_deployment_administrator("POST", &path, Value::Null)
			.await;
		assert_eq!(status, 409, "{claim}: {body}");
		sqlx::query(
			"UPDATE tenant_deletions SET purge_lease = NULL, purge_leased_until = NULL, purge_step = NULL \
			 WHERE tenant = $1 AND restored_at IS NULL",
		)
		.bind(&tenant)
		.execute(pool)
		.await
		.unwrap();
	}

	// Nothing owns it, so the restore works.
	let (status, body) = d
		.as_deployment_administrator("POST", &path, Value::Null)
		.await;
	assert_eq!(status, 200, "{body}");
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_purge_step_that_meets_a_restore_deletes_nothing(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	// Arrange: a purge claimed the Tenant, then a restore took the deletion row
	// and committed while the purge's first step waited for it.
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.tenant_with_work(&alice, "Team").await;
	let (status, body) = d.delete(&alice, &context, &tenant).await;
	assert_eq!(status, 200, "{body}");
	d.grace_period_ends(&tenant).await;
	let before = d.rows_of(&tenant).await;
	let mut claimed = tenant_purge::claim(&d.federation.store).await.unwrap();
	assert_eq!(claimed.len(), 1);
	let deletion = claimed.remove(0);
	let pool = d.federation.store.pool.fixture_driver();
	let mut restore = pool.begin().await.unwrap();
	sqlx::query("SELECT id FROM tenant_deletions WHERE id = $1 FOR UPDATE")
		.bind(deletion.id)
		.execute(&mut *restore)
		.await
		.unwrap();
	let store = d.federation.store.clone();
	let purge = tokio::spawn(async move { tenant_purge::purge_tenant(&store, deletion).await });
	tokio::time::sleep(std::time::Duration::from_millis(500)).await;
	assert!(!purge.is_finished(), "the step must wait for the row lock");

	// Act
	sqlx::query(
		"UPDATE tenant_deletions SET restored_at = now(), restored_by_kind = 'deployment_administrator', \
		 purge_lease = NULL, purge_leased_until = NULL WHERE tenant = $1 AND restored_at IS NULL",
	)
	.bind(&tenant)
	.execute(&mut *restore)
	.await
	.unwrap();
	restore.commit().await.unwrap();
	let outcome = purge.await.unwrap();

	// Assert
	assert!(outcome.is_err(), "a restored Tenant must not be purged");
	assert_eq!(d.rows_of(&tenant).await, before);
	d.finish().await;
}
