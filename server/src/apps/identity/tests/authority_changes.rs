//! Authority Changes against running work (ADR 0121), over the production router
//! and PostgreSQL. A hold here stands in for a running step: a transaction that
//! keeps the Tenant's policy row shared.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../execution/tests/support/endpoint.rs"]
mod endpoint_fixtures;
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::apps::identity::services::node_purge;
use aidash_server::authorization::Authorization;
use aidash_server::database::authority_change;
use aidash_server::database::native::{Pool, Transaction};
use endpoint_fixtures::{EndpointFixture, endpoint};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use reinhardt::test::{APIClient, api_client_from_url};
use rstest::rstest;
use serde_json::{Value, json};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Every test first shortens the limit, before anything reads it, to this value.
const LIMIT: Duration = Duration::from_secs(2);
const PAUSE_SECONDS: u64 = 6;

fn short_limit() {
	static SET: Once = Once::new();
	// Sound because every test comes here first, so no other thread reads the
	// environment until this has returned.
	SET.call_once(|| unsafe { std::env::set_var("AIDASH_POLICY_WAIT_SECONDS", "2") });
}

fn bundle(tenant: &str) -> Value {
	json!({
		"tenant": tenant,
		"subjects": {
			"admin": {"kind":"user","groups":["administrators"]},
			"member": {"kind":"user"}
		},
		"groups": {"administrators": {"roles":["administrator"]}},
		"roles": {"administrator": {}},
		"policies": [{
			"id":"administer","effect":"allow","subjects":{"roles":["administrator"]},
			"actions":["*"],"resources":{"kinds":["*"]}
		}, {
			"id":"read","effect":"allow","subjects":{"any":true},
			"actions":["workspace.read"],"resources":{"kinds":["workspace"]}
		}]
	})
}

async fn tenant(app: &EndpointFixture, name: &str) {
	endpoint_fixtures::tenant_root(&app.runtime, name).await;
	endpoint_fixtures::assert_json(
		app.deployment_administrator
			.post(
				&format!("/api/authorization/{name}"),
				&json!({"expected_revision":0,"bundle":bundle(name)}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
}

async fn client(app: &EndpointFixture, tenant: &str, subject: &str) -> Arc<APIClient> {
	let token = endpoint_fixtures::subject_credential(&app.runtime.store, tenant, subject).await;
	let client = api_client_from_url(&app.server.url);
	client
		.set_header("Authorization", &format!("Bearer {token}"))
		.await
		.unwrap();
	Arc::new(client)
}

/// A transaction that keeps `tenant`'s policy row shared, as a running step does.
async fn hold(pool: &Pool, tenant: &str) -> Transaction {
	let mut tx = pool.begin(&TEST_FIXTURE).await.unwrap();
	let sql = Query::select()
		.column(Alias::new("revision"))
		.from(Alias::new("authorization_bundles"))
		.and_where(Expr::col(Alias::new("tenant")).eq(tenant))
		.lock(LockType::Share)
		.to_string(PostgresQueryBuilder);
	aidash_server::database::native::query(&sql)
		.execute(&mut tx)
		.await
		.unwrap();
	tx
}

async fn revision(pool: &Pool, tenant: &str) -> i64 {
	Authorization { pool: pool.clone() }
		.snapshot(tenant)
		.await
		.unwrap()
		.revision
}

/// Asks to replace the policy as a Deployment Administrator would: an Authority
/// Change with nothing else in front of it.
fn replacing(
	pool: &Pool,
	tenant: &str,
	expected: i64,
) -> tokio::task::JoinHandle<aidash_server::Result<i64>> {
	let pool = pool.clone();
	let tenant = tenant.to_owned();
	tokio::spawn(async move {
		Authorization { pool }
			.replace(
				&tenant,
				expected,
				serde_json::from_value(bundle(&tenant)).unwrap(),
				"deployment_administrator",
			)
			.await
			.map(|snapshot| snapshot.revision)
	})
}

/// Waits until the change has raised its flag.
async fn raised(pool: &Pool, tenant: &str) {
	tokio::time::timeout(LIMIT, async {
		while authority_change::is_clear(pool, tenant).await {
			tokio::time::sleep(Duration::from_millis(5)).await;
		}
	})
	.await
	.expect("the change raised its flag");
}

#[rstest]
#[tokio::test]
async fn a_change_passes_while_new_work_overlaps_continuously(#[future] endpoint: EndpointFixture) {
	short_limit();
	let app = endpoint.await;
	tenant(&app, "overlap").await;
	let pool = app.runtime.store.pool.clone();
	let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
	// A new step every 100 ms, each holding 300 ms, so that holds always overlap.
	// Without the entrance a change would never see the row free.
	let steps = {
		let (pool, stop) = (pool.clone(), stop.clone());
		tokio::spawn(async move {
			let mut running = tokio::task::JoinSet::new();
			while !stop.load(std::sync::atomic::Ordering::Relaxed) {
				let pool = pool.clone();
				running.spawn(async move {
					authority_change::enter(&pool, "overlap").await;
					let step = hold(&pool, "overlap").await;
					tokio::time::sleep(Duration::from_millis(300)).await;
					step.commit().await.unwrap();
				});
				tokio::time::sleep(Duration::from_millis(100)).await;
			}
			while running.join_next().await.is_some() {}
		})
	};
	tokio::time::sleep(Duration::from_millis(500)).await;
	let applied = replacing(&pool, "overlap", 1).await.unwrap();
	stop.store(true, std::sync::atomic::Ordering::Relaxed);
	steps.await.unwrap();
	assert_eq!(applied.unwrap(), 2);
}

#[rstest]
#[tokio::test]
async fn a_change_is_refused_at_the_limit_and_can_be_sent_again(
	#[future] endpoint: EndpointFixture,
) {
	short_limit();
	let app = endpoint.await;
	tenant(&app, "refused").await;
	let pool = app.runtime.store.pool.clone();
	let admin = app.deployment_administrator.clone();
	let body = json!({"expected_revision":1,"bundle":bundle("refused")});
	let step = hold(&pool, "refused").await;

	let started = Instant::now();
	let refused = admin
		.post("/api/authorization/refused", &body, "json")
		.await
		.unwrap();
	assert!(started.elapsed() >= LIMIT);
	assert_eq!(refused.status_code(), 503);
	assert_eq!(refused.json_value().unwrap()["error"], "POLICY_BUSY");
	assert_eq!(
		refused.header("retry-after").unwrap(),
		PAUSE_SECONDS.to_string()
	);
	assert!(refused.header("x-aidash-transaction-pending").is_none());
	step.commit().await.unwrap();
	assert_eq!(revision(&pool, "refused").await, 1);

	// The pause refuses at once, and new work does not wait behind it.
	let started = Instant::now();
	let paused = admin
		.post("/api/authorization/refused", &body, "json")
		.await
		.unwrap();
	assert_eq!(paused.status_code(), 503);
	assert!(started.elapsed() < LIMIT);
	assert!(authority_change::is_clear(&pool, "refused").await);

	tokio::time::sleep(Duration::from_secs(PAUSE_SECONDS)).await;
	let applied = admin
		.post("/api/authorization/refused", &body, "json")
		.await
		.unwrap();
	assert_eq!(applied.status_code(), 200, "{}", applied.text());
	assert_eq!(revision(&pool, "refused").await, 2);
}

#[rstest]
#[tokio::test]
async fn someone_without_the_permission_is_refused_without_holding_anyone_up(
	#[future] endpoint: EndpointFixture,
) {
	short_limit();
	let app = endpoint.await;
	tenant(&app, "unpermitted").await;
	let pool = app.runtime.store.pool.clone();
	let member = client(&app, "unpermitted", "member").await;
	let step = hold(&pool, "unpermitted").await;

	let started = Instant::now();
	let refused = member
		.post(
			"/api/authorization/unpermitted/catalog/approvals",
			&json!({
				"node": aidash_domain::nodes::NodeName::derived("unpermitted"),
				"approvals": [{
					"entry": {"id":"agent","version":"1.0.0"},
					"enabled": true,
					"expected_revision": 0
				}]
			}),
			"json",
		)
		.await
		.unwrap();
	assert!(
		started.elapsed() < LIMIT,
		"{}: {}",
		refused.status_code(),
		refused.text()
	);
	assert_ne!(refused.status_code(), 503);
	assert!(authority_change::is_clear(&pool, "unpermitted").await);
	step.commit().await.unwrap();
	// Nothing paused the Tenant either.
	replacing(&pool, "unpermitted", 1)
		.await
		.unwrap()
		.expect("a permitted change passes");
}

#[rstest]
#[tokio::test]
async fn stops_and_reads_inside_a_hold_pass_a_waiting_change(#[future] endpoint: EndpointFixture) {
	short_limit();
	let app = endpoint.await;
	tenant(&app, "stops").await;
	let pool = app.runtime.store.pool.clone();
	let admin = client(&app, "stops", "admin").await;
	let step = hold(&pool, "stops").await;
	let change = replacing(&pool, "stops", 1);
	raised(&pool, "stops").await;

	// A read inside a hold takes the row without testing the flag.
	revision(&pool, "stops").await;
	assert!(!change.is_finished());
	// These Runs do not exist, so the route refuses them, but they got past the
	// entrance while the change still waited.
	let unknown = uuid::Uuid::new_v4();
	for path in [
		format!("/api/runs/{unknown}/control"),
		format!("/api/runs/{unknown}/management"),
	] {
		let response = admin
			.post(&path, &json!({"action":"pause"}), "json")
			.await
			.unwrap();
		assert_ne!(response.status_code(), 503, "{}", response.text());
		assert!(!change.is_finished(), "{path} waited behind the change");
	}
	step.commit().await.unwrap();
	change.await.unwrap().expect("the change passes");
}

#[rstest]
#[tokio::test]
async fn an_api_request_waits_behind_a_waiting_change(#[future] endpoint: EndpointFixture) {
	short_limit();
	let app = endpoint.await;
	tenant(&app, "entrance").await;
	let pool = app.runtime.store.pool.clone();
	let admin = client(&app, "entrance", "admin").await;
	let step = hold(&pool, "entrance").await;
	let change = replacing(&pool, "entrance", 1);
	raised(&pool, "entrance").await;

	let reader = tokio::spawn(async move { admin.get("/api/state").await.unwrap().status_code() });
	tokio::time::sleep(Duration::from_millis(300)).await;
	assert!(!reader.is_finished(), "new work passed a waiting change");
	assert!(!change.is_finished());
	step.commit().await.unwrap();
	change.await.unwrap().expect("the change passes");
	assert_eq!(reader.await.unwrap(), 200);
}

#[rstest]
#[tokio::test]
async fn work_waiting_behind_a_change_leaves_connections_for_other_tenants(
	#[future] endpoint: EndpointFixture,
) {
	short_limit();
	let app = endpoint.await;
	tenant(&app, "crowded").await;
	tenant(&app, "neighbour").await;
	let pool = app.runtime.store.pool.clone();
	let step = hold(&pool, "crowded").await;
	let change = replacing(&pool, "crowded", 1);
	raised(&pool, "crowded").await;

	let mut waiting = tokio::task::JoinSet::new();
	for _ in 0..64 {
		let pool = pool.clone();
		waiting.spawn(async move { authority_change::enter(&pool, "crowded").await });
	}
	tokio::time::sleep(Duration::from_millis(300)).await;
	assert_eq!(waiting.len(), 64);
	// The whole of a small pool would be gone if they each held a connection.
	revision(&pool, "neighbour").await;
	assert!(!change.is_finished());

	step.commit().await.unwrap();
	change.await.unwrap().expect("the change passes");
	while waiting.join_next().await.is_some() {}
}

#[rstest]
#[tokio::test]
async fn revoking_a_credential_in_use_is_refused_at_the_limit(#[future] endpoint: EndpointFixture) {
	short_limit();
	let app = endpoint.await;
	tenant(&app, "revoking").await;
	let pool = app.runtime.store.pool.clone();
	let service = Authorization { pool: pool.clone() };
	let issued = service
		.issue_credential("revoking", "member", 3600, "deployment_administrator")
		.await
		.unwrap();
	// A running step keeps its credential shared as well as the policy.
	let mut step = pool.begin(&TEST_FIXTURE).await.unwrap();
	aidash_server::database::native::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("authorization_credentials"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(issued.credential.id)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut step)
	.await
	.unwrap();
	assert!(matches!(
		service
			.revoke_credential("revoking", issued.credential.id)
			.await,
		Err(aidash_server::Error::PolicyBusy {
			retry_after: PAUSE_SECONDS
		})
	));
	step.commit().await.unwrap();
	tokio::time::sleep(Duration::from_secs(PAUSE_SECONDS)).await;
	service
		.revoke_credential("revoking", issued.credential.id)
		.await
		.expect("the revocation passes once the step has ended");
}

#[rstest]
#[tokio::test]
async fn a_resume_waits_at_the_entrance_before_it_holds_a_visibility_lease(
	#[future] endpoint: EndpointFixture,
) {
	short_limit();
	let app = endpoint.await;
	tenant(&app, "resuming").await;
	tenant(&app, "bystander").await;
	let pool = app.runtime.store.pool.clone();
	let admin = client(&app, "resuming", "admin").await;
	let bystander = client(&app, "bystander", "admin").await;
	let step = hold(&pool, "resuming").await;
	let change = replacing(&pool, "resuming", 1);
	raised(&pool, "resuming").await;

	// More resumes than the visibility pool has connections (16). Each one that
	// waited under its lease would pin one, and no other request could get one.
	let unknown = Uuid::new_v4();
	let mut resuming = tokio::task::JoinSet::new();
	for _ in 0..24 {
		let admin = admin.clone();
		resuming.spawn(async move {
			admin
				.post(
					&format!("/api/runs/{unknown}/control"),
					&json!({"action":"resume"}),
					"json",
				)
				.await
				.unwrap()
				.status_code()
		});
	}
	tokio::time::sleep(Duration::from_millis(300)).await;
	let reader = tokio::time::timeout(Duration::from_secs(1), bystander.get("/api/state"))
		.await
		.expect("resumes waiting at the entrance took the visibility pool from another Tenant")
		.unwrap();
	assert_eq!(reader.status_code(), 200);
	assert!(
		resuming.len() == 24 && !change.is_finished(),
		"a resume passed a waiting change"
	);
	step.commit().await.unwrap();
	change.await.unwrap().expect("the change passes");
	while let Some(status) = resuming.join_next().await {
		// The Run does not exist, but the resume got through the entrance.
		assert_ne!(status.unwrap(), 503);
	}
}

/// A due Node deletion of `tenant` on a Node of its own. `purged` is how many
/// days ago the purge finished, if it has; otherwise the purge is due since
/// `due` days.
async fn deletion(app: &EndpointFixture, tenant: &str, due: i32, purged: Option<i32>) -> Uuid {
	let db = app.runtime.store.pool.fixture_driver();
	let node = endpoint_fixtures::second_node_in(db, tenant).await;
	let id = Uuid::new_v4();
	sqlx::query(
		"INSERT INTO node_deletions (id,tenant,node,deleted_at,deleted_by_kind,purge_after,purged_at,purge_failures) \
		 VALUES ($1,$2,$3,clock_timestamp()-make_interval(days=>$4+30),'deployment_administrator',\
		 clock_timestamp()-make_interval(days=>$4),clock_timestamp()-make_interval(days=>$5),0)",
	)
	.bind(id)
	.bind(tenant)
	.bind(node.as_str())
	.bind(due)
	.bind(purged)
	.execute(db)
	.await
	.unwrap();
	id
}

async fn deletions(app: &EndpointFixture, id: Uuid) -> i64 {
	sqlx::query_scalar("SELECT count(*) FROM node_deletions WHERE id = $1")
		.bind(id)
		.fetch_one(app.runtime.store.pool.fixture_driver())
		.await
		.unwrap()
}

#[rstest]
#[tokio::test]
async fn a_busy_tenant_does_not_stop_the_purge_of_other_tenants(
	#[future] endpoint: EndpointFixture,
) {
	short_limit();
	let app = endpoint.await;
	tenant(&app, "busy-purge").await;
	tenant(&app, "quiet-purge").await;
	let pool = app.runtime.store.pool.clone();
	// The busy Tenant's deadline is the older, so it comes first.
	let busy = deletion(&app, "busy-purge", 10, None).await;
	let quiet = deletion(&app, "quiet-purge", 1, None).await;
	let step = hold(&pool, "busy-purge").await;

	let claimed = node_purge::claim(&app.runtime.store).await;
	step.commit().await.unwrap();
	let claimed = claimed.expect("a Tenant that stays busy leaves its Node to the next pass");
	assert_eq!(
		claimed
			.iter()
			.map(|deletion| deletion.id)
			.collect::<Vec<_>>(),
		vec![quiet]
	);
	assert_eq!(deletions(&app, busy).await, 1);
}

#[rstest]
#[tokio::test]
async fn a_busy_tenant_does_not_stop_the_audit_erasure_of_other_tenants(
	#[future] endpoint: EndpointFixture,
) {
	short_limit();
	let app = endpoint.await;
	tenant(&app, "busy-audit").await;
	tenant(&app, "quiet-audit").await;
	let pool = app.runtime.store.pool.clone();
	// The busy Tenant's purge is the older, so it comes first.
	let busy = deletion(&app, "busy-audit", 401, Some(400)).await;
	let quiet = deletion(&app, "quiet-audit", 400, Some(399)).await;
	let step = hold(&pool, "busy-audit").await;

	let erased = node_purge::erase_audit(&app.runtime.store, 365).await;
	step.commit().await.unwrap();
	erased.expect("a Tenant that stays busy leaves its audit to the next pass");
	assert_eq!(deletions(&app, quiet).await, 0);
	assert_eq!(deletions(&app, busy).await, 1);
}

#[rstest]
#[tokio::test]
async fn a_deployment_administrator_disabling_a_mapping_is_an_authority_change(
	#[future] endpoint: EndpointFixture,
) {
	short_limit();
	let app = endpoint.await;
	tenant(&app, "disabling").await;
	let pool = app.runtime.store.pool.clone();
	let mapping = endpoint_fixtures::browser_mapping(&app, "disabling", "member").await;
	let administrator = Arc::new(endpoint_fixtures::browser_administrator(&app).await);
	let credential: Uuid = mapping.credential.parse().unwrap();
	let (id, revision): (Uuid, i64) =
		sqlx::query_as("SELECT id, revision FROM dashboard_mappings WHERE credential_id = $1")
			.bind(credential)
			.fetch_one(pool.fixture_driver())
			.await
			.unwrap();
	// A running step keeps the Mapping's credential shared.
	let mut step = pool.begin(&TEST_FIXTURE).await.unwrap();
	aidash_server::database::native::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("authorization_credentials"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(credential)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut step)
	.await
	.unwrap();

	let started = Instant::now();
	let disabling = {
		let administrator = administrator.clone();
		tokio::spawn(async move {
			administrator
				.send(
					"POST",
					&format!("/api/dashboard/mappings/{id}/disable"),
					json!({"expected_revision": revision}),
				)
				.await
		})
	};
	// It must raise the flag, or overlapping Runs would starve it for good.
	raised(&pool, "disabling").await;
	let (status, body) = disabling.await.unwrap();
	assert!(started.elapsed() >= LIMIT);
	assert_eq!((status, body["error"].as_str()), (503, Some("POLICY_BUSY")));
	step.commit().await.unwrap();
	let enabled: bool = sqlx::query_scalar("SELECT enabled FROM dashboard_mappings WHERE id = $1")
		.bind(id)
		.fetch_one(pool.fixture_driver())
		.await
		.unwrap();
	assert!(enabled, "the refused change left the Mapping as it was");
}
