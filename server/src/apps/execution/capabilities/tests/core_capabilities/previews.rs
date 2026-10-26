//! Preview gateway: session lifetime, host folding and periodic expiry.
use super::*;
use aidash_domain::preview::{PreviewLabel, PreviewSecret, SESSION_SECONDS};
use aidash_server::apps::execution::repositories::previews::Gateway;
use aidash_server::authorization::identity::Actor;
use chrono::{Duration, Utc};

async fn actor(c: &CoreFixture) -> Actor {
	aidash_server::authorization::Authorization {
		pool: c.f.store.pool.clone(),
	}
	.authenticate(&c.token)
	.await
	.unwrap()
}

async fn area_and_file(c: &CoreFixture) -> (Uuid, Uuid) {
	let run = admit(c).await;
	let (status, created) = request(
		&c.app,
		&c.token,
		"POST",
		&path_for_patch(run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"index.html":null,"assets/app.js":null},"patch":"*** Begin Patch\n*** Add File: index.html\n+<html>preview</html>\n*** Add File: assets/app.js\n+app()\n*** End Patch"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let (status, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let file: Uuid = serde_json::from_value(
		area["manifest"]
			.as_array()
			.unwrap()
			.iter()
			.find(|file| file["path"] == "index.html")
			.unwrap()["file_id"]
			.clone(),
	)
	.unwrap();
	(area_id, file)
}

async fn insert_preview(
	c: &CoreFixture,
	preview: Uuid,
	tenant: &str,
	area: Uuid,
	file: Uuid,
	label: &str,
	subject: &str,
) {
	sqlx::query(
		"INSERT INTO core_previews (id, tenant, kind, area_id, file_id, label, created_by)
		 VALUES ($1, $2, 'file', $3, $4, $5, $6)",
	)
	.bind(preview)
	.bind(tenant)
	.bind(area)
	.bind(file)
	.bind(label)
	.bind(subject)
	.execute(c.f.store.pool.fixture_driver())
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

fn gateway(c: &CoreFixture) -> Gateway {
	Gateway {
		store: c.f.store.clone(),
		domain: "localhost".into(),
	}
}

#[rstest::rstest]
#[tokio::test]
async fn an_expired_session_is_refused_while_a_live_one_is_served(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let (area, file) = area_and_file(&c).await;
	let Actor::Subject(identity) = actor(&c).await else {
		panic!("the fixture token is a subject");
	};
	let preview = Uuid::new_v4();
	let label = PreviewLabel::generate().unwrap();
	insert_preview(
		&c,
		preview,
		&identity.tenant,
		area,
		file,
		label.as_str(),
		&identity.subject,
	)
	.await;
	let expired = "e".repeat(43);
	insert_session(
		&c,
		preview,
		&identity.tenant,
		&identity.subject,
		identity.credential_id,
		&expired,
		Utc::now() - Duration::seconds(1),
	)
	.await;
	let error = gateway(&c)
		.view(
			&format!("{}.LOCALHOST", label.as_str().to_ascii_uppercase()),
			&expired,
			"/",
		)
		.await
		.unwrap_err();
	assert!(
		matches!(error, aidash_application::Error::Forbidden),
		"an expired cookie is refused: {error}"
	);

	let live = "l".repeat(43);
	insert_session(
		&c,
		preview,
		&identity.tenant,
		&identity.subject,
		identity.credential_id,
		&live,
		Utc::now() + Duration::seconds(SESSION_SECONDS),
	)
	.await;
	let page = gateway(&c)
		.view(&format!("{}.LOCALHOST", label.as_str()), &live, "/")
		.await
		.unwrap();
	assert!(String::from_utf8(page.bytes).unwrap().contains("preview"));
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_preview_ticket_is_single_use_and_expires(#[future] capability_fixture: CoreFixture) {
	let c = capability_fixture.await;
	let (area, file) = area_and_file(&c).await;
	let Actor::Subject(identity) = actor(&c).await else {
		panic!("the fixture token is a subject");
	};
	let preview = Uuid::new_v4();
	let label = PreviewLabel::generate().unwrap();
	insert_preview(
		&c,
		preview,
		&identity.tenant,
		area,
		file,
		label.as_str(),
		&identity.subject,
	)
	.await;
	let ticket = "t".repeat(43);
	insert_ticket(
		&c,
		preview,
		&identity.tenant,
		&identity.subject,
		identity.credential_id,
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
		&identity.tenant,
		&identity.subject,
		identity.credential_id,
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
	let (area, file) = area_and_file(&c).await;
	let Actor::Subject(identity) = actor(&c).await else {
		panic!("the fixture token is a subject");
	};
	let preview = Uuid::new_v4();
	let label = PreviewLabel::generate().unwrap();
	insert_preview(
		&c,
		preview,
		&identity.tenant,
		area,
		file,
		label.as_str(),
		&identity.subject,
	)
	.await;
	let expired = Utc::now() - Duration::seconds(1);
	insert_ticket(
		&c,
		preview,
		&identity.tenant,
		&identity.subject,
		identity.credential_id,
		&"q".repeat(43),
		expired,
	)
	.await;
	insert_session(
		&c,
		preview,
		&identity.tenant,
		&identity.subject,
		identity.credential_id,
		&"r".repeat(43),
		expired,
	)
	.await;
	insert_session(
		&c,
		preview,
		&identity.tenant,
		&identity.subject,
		identity.credential_id,
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
