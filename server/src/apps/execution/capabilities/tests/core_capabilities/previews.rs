//! Preview gateway: session lifetime, host folding and periodic expiry.
use super::*;
use aidash_domain::preview::{PreviewLabel, PreviewSecret};
use aidash_server::capabilities::preview_gateway_for_test;
use chrono::{Duration, Utc};

/// The fixture token's Tenant, subject and credential. The identity type is
/// private to the server crate, so read the credential id from its store.
async fn identity(c: &CoreFixture) -> (String, String, Uuid) {
	let credential: Uuid = sqlx::query_scalar(
		"SELECT id FROM authorization_credentials
		 WHERE tenant = 'acme' AND subject = 'alice' LIMIT 1",
	)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	("acme".into(), "alice".into(), credential)
}

async fn insert_preview(c: &CoreFixture, preview: Uuid, tenant: &str, label: &str, subject: &str) {
	let run = admit(c).await;
	let live = Uuid::new_v4();
	let now = Utc::now();
	let db = c.f.store.pool.fixture_driver();
	sqlx::query(
		"INSERT INTO core_live_previews
		 (id, tenant, run_id, port, state, idle_seconds, absolute_seconds, started_at, last_request_at, expires_at)
		 VALUES ($1, $2, $3, 3000, 'running', 900, 7200, $4, $4, $5)",
	)
	.bind(live).bind(tenant).bind(run.id).bind(now).bind(now + Duration::hours(2))
	.execute(db).await.unwrap();
	sqlx::query(
		"INSERT INTO core_previews (id, tenant, kind, live_preview_id, label, created_by)
		 VALUES ($1, $2, 'live', $3, $4, $5)",
	)
	.bind(preview)
	.bind(tenant)
	.bind(live)
	.bind(label)
	.bind(subject)
	.execute(db)
	.await
	.unwrap();
}

async fn insert_session(
	c: &CoreFixture,
	preview: Uuid,
	tenant: &str,
	subject: &str,
	credential: Uuid,
	secret: &str,
	expires_at: chrono::DateTime<Utc>,
) {
	sqlx::query(
		"INSERT INTO core_preview_sessions
		 (id, tenant, preview_id, subject, credential_id, origin, digest, served_bytes, expires_at)
		 VALUES ($1, $2, $3, $4, $5, 'https://dashboard.example', $6, 0, $7)",
	)
	.bind(Uuid::new_v4())
	.bind(tenant)
	.bind(preview)
	.bind(subject)
	.bind(credential)
	.bind(PreviewSecret::parse(secret).unwrap().digest())
	.bind(expires_at)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
}

async fn insert_ticket(
	c: &CoreFixture,
	preview: Uuid,
	tenant: &str,
	subject: &str,
	credential: Uuid,
	secret: &str,
	expires_at: chrono::DateTime<Utc>,
) {
	sqlx::query(
		"INSERT INTO core_preview_tickets
		 (id, tenant, preview_id, subject, credential_id, origin, digest, expires_at)
		 VALUES ($1, $2, $3, $4, $5, 'https://dashboard.example', $6, $7)",
	)
	.bind(Uuid::new_v4())
	.bind(tenant)
	.bind(preview)
	.bind(subject)
	.bind(credential)
	.bind(PreviewSecret::parse(secret).unwrap().digest())
	.bind(expires_at)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
}

async fn count(c: &CoreFixture, table: &str) -> i64 {
	sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap()
}

fn gateway(c: &CoreFixture) -> std::sync::Arc<dyn aidash_application::preview::Authority> {
	preview_gateway_for_test(c.f.store.clone(), "localhost")
}

#[rstest::rstest]
#[tokio::test]
async fn a_preview_ticket_is_single_use_and_expires(#[future] capability_fixture: CoreFixture) {
	let c = capability_fixture.await;
	let (tenant, subject, credential) = identity(&c).await;
	let preview = Uuid::new_v4();
	let label = PreviewLabel::generate().unwrap();
	insert_preview(&c, preview, &tenant, label.as_str(), &subject).await;
	let ticket = "t".repeat(43);
	insert_ticket(
		&c,
		preview,
		&tenant,
		&subject,
		credential,
		&ticket,
		Utc::now() + Duration::seconds(60),
	)
	.await;
	let host = format!("{}.localhost", label.as_str());
	gateway(&c).exchange(&host, &ticket).await.unwrap();
	assert!(
		matches!(
			gateway(&c).exchange(&host, &ticket).await.unwrap_err(),
			aidash_application::Error::Forbidden
		),
		"a consumed ticket cannot be exchanged twice"
	);
	let expired = "x".repeat(43);
	insert_ticket(
		&c,
		preview,
		&tenant,
		&subject,
		credential,
		&expired,
		Utc::now() - Duration::seconds(1),
	)
	.await;
	assert!(
		matches!(
			gateway(&c).exchange(&host, &expired).await.unwrap_err(),
			aidash_application::Error::Forbidden
		),
		"an expired ticket cannot be exchanged"
	);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn the_reclamation_sweep_removes_expired_tickets_and_sessions(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let (tenant, subject, credential) = identity(&c).await;
	let preview = Uuid::new_v4();
	let label = PreviewLabel::generate().unwrap();
	insert_preview(&c, preview, &tenant, label.as_str(), &subject).await;
	let expired = Utc::now() - Duration::seconds(1);
	insert_ticket(
		&c,
		preview,
		&tenant,
		&subject,
		credential,
		&"q".repeat(43),
		expired,
	)
	.await;
	insert_session(
		&c,
		preview,
		&tenant,
		&subject,
		credential,
		&"r".repeat(43),
		expired,
	)
	.await;
	insert_session(
		&c,
		preview,
		&tenant,
		&subject,
		credential,
		&"s".repeat(43),
		Utc::now() + Duration::seconds(SESSION_SECONDS),
	)
	.await;
	assert_eq!(count(&c, "core_preview_tickets").await, 1);
	assert_eq!(count(&c, "core_preview_sessions").await, 2);
	let WorkerControl { stop, receiver: rx } = worker_control();
	let worker = tokio::spawn(aidash_server::capabilities::reclamation::run(
		c.f.store.clone(),
		rx,
	));
	tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
	assert_eq!(
		count(&c, "core_preview_tickets").await,
		0,
		"the sweep removes an expired ticket"
	);
	assert_eq!(
		count(&c, "core_preview_sessions").await,
		1,
		"the sweep removes only the expired session"
	);
	let _ = stop.send(true);
	worker.await.unwrap().unwrap();
	c.close().await;
}
