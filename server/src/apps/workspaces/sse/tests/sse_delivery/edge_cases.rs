use super::*;
use aidash_application::tenancy::TEST_FIXTURE;
use reinhardt::query::{Expr, LockType};
use sha2::{Digest, Sha256};

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audited_frames_allocate_sequences_only_after_serialization_lock(
	#[future(awt)]
	#[from(sse_fixture)]
	#[with(Duration::from_secs(60), Duration::from_secs(30), 128)]
	root: Arc<Fixture>,
) {
	let fixture = root;
	let ws = fixture.workspaces[0];
	let event = fixture.emit(ws, 1).await;
	let mut barrier = fixture.f.store.pool.fixture_driver().begin().await.unwrap();
	sqlx::query(
		&Query::select()
			.expr(Expr::cust("pg_advisory_xact_lock(71003202)"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *barrier)
	.await
	.unwrap();
	let sequence_sql = Query::select()
		.column(Alias::new("last_value"))
		.from(Alias::new("authorization_decisions_sequence_seq"))
		.to_string(PostgresQueryBuilder);
	let before: i64 = sqlx::query_scalar(&sequence_sql)
		.fetch_one(fixture.f.store.pool.fixture_driver())
		.await
		.unwrap();
	let mut observers = Vec::new();
	for _ in 0..2 {
		let mut response = fixture.open(event.sequence - 1, Some(ws), None).await;
		assert_eq!(response.status, 200);
		observers.push(tokio::spawn(async move {
			let mut frames = response.take_stream_body().unwrap().boxed();
			frame(&mut frames).await
		}));
	}
	tokio::time::timeout(Duration::from_secs(3), async {
		loop {
			let waiting: i64 = sqlx::query_scalar(
				&Query::select()
					.expr(Expr::cust("COUNT(*)"))
					.from(Alias::new("pg_locks"))
					.inner_join(
						Alias::new("pg_stat_activity"),
						Expr::col((Alias::new("pg_locks"), Alias::new("pid")))
							.equals((Alias::new("pg_stat_activity"), Alias::new("pid"))),
					)
					.and_where(Expr::col(Alias::new("application_name")).eq(fixture.schema.clone()))
					.and_where(Expr::col(Alias::new("locktype")).eq("advisory"))
					.and_where(Expr::col(Alias::new("objid")).eq(71003202))
					.and_where(
						Expr::col(Alias::new("granted")).eq(reinhardt::query::Expr::value(false)),
					)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(fixture.f.store.pool.fixture_driver())
			.await
			.unwrap();
			if waiting == 2 {
				break;
			}
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("both per-frame audits must wait for serialization");
	let blocked: i64 = sqlx::query_scalar(&sequence_sql)
		.fetch_one(fixture.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		blocked, before,
		"waiting audits cannot allocate sequence IDs"
	);
	assert!(observers.iter().all(|task| !task.is_finished()));
	barrier.commit().await.unwrap();
	for observer in observers {
		assert_eq!(
			observer.await.unwrap(),
			(
				event.sequence,
				event.cloud_event(fixture.f.config.deployment.as_str())
			)
		);
	}
	let audits: Vec<(i64, String)> = sqlx::query_as(
		&Query::select()
			.columns([Alias::new("sequence"), Alias::new("action")])
			.from(Alias::new("authorization_decisions"))
			.and_where(Expr::col(Alias::new("sequence")).gt(before))
			.order_by(Alias::new("sequence"), reinhardt::query::Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(fixture.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(
		audits,
		[
			"workspace.read",
			"workspace.events",
			"workspace.read",
			"workspace.events"
		]
		.into_iter()
		.enumerate()
		.map(|(n, action)| (before + n as i64 + 1, action.to_owned()))
		.collect::<Vec<_>>()
	);
	fixture.finish().await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn buffered_resource_revocation_filters_without_rewinding(
	#[future(awt)]
	#[from(sse_fixture)]
	#[with(Duration::from_millis(250), Duration::from_secs(30), 128)]
	root: Arc<Fixture>,
) {
	let fixture = root;
	let ws = fixture.workspaces[0];
	fixture
		.f
		.store
		.message(&TEST_FIXTURE, ws, "alice", "protected message", None)
		.await
		.unwrap();
	let message = fixture
		.f
		.store
		.snapshot(&TEST_FIXTURE, ws)
		.await
		.unwrap()
		.messages[0]
		.id;
	let (status, visible) = request(
		&fixture.app,
		&fixture.token,
		"GET",
		&format!("/api/events?workspace_id={ws}"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200);
	assert!(
		visible
			.as_array()
			.unwrap()
			.iter()
			.any(|event| event["kind"] == "message.created"
				&& event["data"]["id"] == message.to_string())
	);
	let mut stream = fixture.stream(0, Some(ws), None).await;
	assert_eq!(frame(&mut stream).await.1["type"], "workspace.created");
	// The first frame proves the page containing the later message has been
	// loaded. Revoke only that resource before polling the remaining page.
	let mut denied = policy(fixture.f.config.deployment.as_str());
	denied["policies"].as_array_mut().unwrap().push(json!({"id":"message-denied","effect":"deny","subjects":{"any":true},"actions":["message.read"],"resources":{"kinds":["message"],"ids":[message]}}));
	for (revision, bundle) in [
		(1, denied),
		(2, policy(fixture.f.config.deployment.as_str())),
	] {
		common::replace_policy(&fixture.f.store, "acme", bundle).await;
		let allowed = fixture.emit(ws, 100 + revision).await;
		assert_eq!(
			frame(&mut stream).await,
			(
				allowed.sequence,
				allowed.cloud_event(fixture.f.config.deployment.as_str())
			)
		);
	}
	drop(stream);
	fixture.finish().await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_flood_preserves_fallback_and_idle_revocation(
	#[from(sse_fixture)]
	#[with(Duration::from_millis(500), Duration::from_secs(30), 128)]
	root: SseFuture,

	#[future(awt)]
	#[from(sse_subscription)]
	#[with(root.clone())]
	subscription: SseSubscription,
) {
	let fixture = root.await;
	let (stop, subscriber) = subscription;
	let mut selected = fixture.stream(-1, Some(fixture.workspaces[0]), None).await;
	let mut unrelated = fixture.stream(-1, Some(fixture.workspaces[1]), None).await;
	until(|| fixture.service.snapshot().query_causes[0] == 4).await;
	let first = fixture.emit(fixture.workspaces[0], 1).await;
	let missed = fixture.emit(fixture.workspaces[1], 2).await;
	let nats = async_nats::connect(&fixture.f.config.nats_url)
		.await
		.unwrap();
	let subject = format!(
		"aidash.{}.events",
		fixture
			.f
			.config
			.deployment
			.as_str()
			.trim_start_matches("aidash://")
	);
	let payload = first
		.cloud_event(fixture.f.config.deployment.as_str())
		.to_string();
	let (end, mut ending) = tokio::sync::watch::channel(false);
	let flood = tokio::spawn(async move {
		let mut ticks = tokio::time::interval(Duration::from_millis(5));
		loop {
			tokio::select! {
				_ = ending.changed() => break,
				_ = ticks.tick() => {
					for _ in 0..16 {
						nats.publish(subject.clone(), payload.clone().into()).await.unwrap();
					}
					nats.flush().await.unwrap();
				}
			}
		}
	});
	assert_eq!(frame(&mut selected).await.0, first.sequence);
	assert_eq!(frame(&mut unrelated).await.0, missed.sequence);
	let audit_sql = Query::select()
		.expr(Expr::cust("COUNT(*)"))
		.from(Alias::new("authorization_decisions"))
		.to_string(PostgresQueryBuilder);
	let audit_before: i64 = sqlx::query_scalar(&audit_sql)
		.fetch_one(fixture.f.store.pool.fixture_driver())
		.await
		.unwrap();
	until(|| fixture.service.snapshot().query_causes[3] >= 4).await;
	let audit_after: i64 = sqlx::query_scalar(&audit_sql)
		.fetch_one(fixture.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		audit_after, audit_before,
		"empty notification/fallback/authority checks do not append decisions"
	);
	let before = fixture.service.snapshot();
	assert!(before.notifications >= 100);
	assert_eq!(before.registered_scopes, 2);
	// Prove that authority monitoring progresses while notifications continue.
	// After revocation, a reader may detect denial and close the stream before
	// the next monitor tick, so that tick is not a condition of safe closure.
	until(|| fixture.service.snapshot().authority_checks > before.authority_checks).await;
	aidash_server::authorization::Authorization {
		pool: fixture.f.store.control_pool.clone(),
	}
	.revoke_credential("acme", fixture.credential)
	.await
	.unwrap();
	// Neither body is polled while notifications continue. Revocation must
	// close both connections and remove registrations independently.
	until(|| fixture.service.snapshot().registered_scopes == 0).await;
	end.send_replace(true);
	flood.await.unwrap();
	stop.send_replace(true);
	subscriber.await.unwrap();
	drop((selected, unrelated));
	fixture.finish().await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn notification_during_initial_authority_read_is_retained(
	#[from(sse_fixture)]
	#[with(Duration::from_secs(60), Duration::from_secs(30), 128)]
	root: SseFuture,

	#[future(awt)]
	#[from(sse_subscription)]
	#[with(root.clone())]
	subscription: SseSubscription,
) {
	let fixture = root.await;
	let (stop, subscriber) = subscription;
	let ws = fixture.workspaces[0];
	let before = fixture.emit(ws, 1).await;
	let mut ownership = fixture.f.store.pool.fixture_driver().begin().await.unwrap();
	sqlx::query(
		&Query::select()
			.column(Alias::new("workspace_id"))
			.from(Alias::new("authorization_workspaces"))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *ownership)
	.await
	.unwrap();
	// Initial liveness uses a read-only snapshot, but the canonical page reader
	// cannot pass the real ownership lease until this transaction releases it.
	let mut response = fixture.open(before.sequence, Some(ws), None).await;
	assert_eq!(response.status, 200);
	until(|| fixture.service.snapshot().registered_scopes == 1).await;
	let event = fixture.emit(ws, 2).await;
	let nats = async_nats::connect(&fixture.f.config.nats_url)
		.await
		.unwrap();
	hint(
		&nats,
		&fixture.f,
		event.cloud_event(fixture.f.config.deployment.as_str()),
	)
	.await;
	until(|| fixture.service.snapshot().notifications == 1).await;
	assert_eq!(fixture.service.snapshot().event_queries, 0);
	ownership.rollback().await.unwrap();
	let mut frames = response.take_stream_body().unwrap().boxed();
	assert_eq!(
		frame(&mut frames).await,
		(
			event.sequence,
			event.cloud_event(fixture.f.config.deployment.as_str())
		)
	);
	assert_eq!(fixture.service.snapshot().query_causes[3], 0);
	stop.send_replace(true);
	subscriber.await.unwrap();
	drop(frames);
	fixture.finish().await;
}

#[rstest::rstest]
#[case::session_revoked("session", "revoked_at")]
#[case::session_expired("session", "expires_at")]
#[case::session_idle_expired("session", "last_activity_at")]
#[case::mapping_disabled("mapping", "enabled")]
#[case::identity_disabled("identity", "disabled_at")]
#[case::deployment_administrator_grant_removed("deployment_administrator", "enabled")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_invalidation_closes_idle_and_unpolled_buffered_streams(
	#[case] change: &str,
	#[case] column: &str,
	#[future(awt)]
	#[from(browser_sse_fixture)]
	root: Arc<Fixture>,
) {
	let fixture = root;
	let issuer = "https://accounts.google.com";
	let identity = Uuid::new_v4();
	let session = Uuid::new_v4();
	let mapping = Uuid::new_v4();
	{
		let query_bind_1 = identity;
		let query_bind_2 = issuer;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("dashboard_identities"))
				.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(Expr::value(query_bind_1.to_owned()))
						.expr(Expr::value(query_bind_2.to_owned()))
						.expr(Expr::val("browser-fixture"))
						.expr(Expr::cust("clock_timestamp()"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(fixture.f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	{
		let query_bind_1 = session;
		let query_bind_2 = common::browser_session_digest(b"sse-browser-session");
		let query_bind_3 = Sha256::digest(b"csrf").to_vec();
		let query_bind_4 = identity;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("dashboard_sessions"))
				.columns(
					[
						"id",
						"token_hash",
						"csrf_hash",
						"identity_id",
						"created_at",
						"last_activity_at",
						"expires_at",
					]
					.map(Alias::new),
				)
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.expr(Expr::cust("clock_timestamp()"))
						.expr(Expr::cust("clock_timestamp()"))
						.expr(Expr::cust("clock_timestamp()+interval '12 hours'"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(fixture.f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	{
		let query_bind_1 = mapping;
		let query_bind_2 = identity;
		let query_bind_3 = fixture.credential;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("dashboard_mappings"))
				.columns(
					["id", "identity_id", "tenant", "subject", "credential_id"].map(Alias::new),
				)
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(Expr::value(query_bind_1.to_owned()))
						.expr(Expr::value(query_bind_2.to_owned()))
						.expr(Expr::val("acme"))
						.expr(Expr::val("alice"))
						.expr(Expr::value(query_bind_3.to_owned()))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(fixture.f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	let selector = if change == "deployment_administrator" {
		{
			let query_bind_1 = identity;
			sqlx::query(
				&Query::insert()
					.into_table(Alias::new("dashboard_deployment_administrator_grants"))
					.columns([Alias::new("identity_id")])
					.from_subquery(
						reinhardt::query::Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(fixture.f.store.pool.fixture_driver())
			.await
		}
		.unwrap();
		"deployment_administrator".to_owned()
	} else {
		format!("mapping:acme:{mapping}")
	};
	let mut responses = Vec::new();
	for (workspace, after) in [(fixture.workspaces[0], 0), (fixture.workspaces[1], -1)] {
		let response = fixture
			.app
			.clone()
			.native_oneshot(
				Request::get(format!(
					"/api/events/stream?after={after}&workspace_id={workspace}"
				))
				.header("cookie", "__Host-aidash-session=sse-browser-session")
				.header("x-aidash-context", &selector)
				.body(bytes::Bytes::new())
				.unwrap(),
			)
			.await
			.unwrap();
		// A Deployment Administrator context reads no Tenant Workspace, so it
		// never holds a stream for the grant removal to close (#484).
		if change == "deployment_administrator" {
			assert_eq!(response.status, 403);
			fixture.finish().await;
			return;
		}
		assert_eq!(response.status, 200);
		responses.push(response);
	}
	until(|| fixture.service.snapshot().query_causes[0] == 3).await;
	let reads = fixture.service.snapshot().event_queries;
	assert_eq!(fixture.open(-1, None, None).await.status, 503);
	let (table, key, id) = match change {
		"session" => ("dashboard_sessions", "id", session),
		"mapping" => ("dashboard_mappings", "id", mapping),
		"identity" => ("dashboard_identities", "id", identity),
		"deployment_administrator" => (
			"dashboard_deployment_administrator_grants",
			"identity_id",
			identity,
		),
		_ => unreachable!(),
	};
	let value = if column == "enabled" {
		Expr::val(false)
	} else if column == "last_activity_at" {
		Expr::cust("clock_timestamp()-interval '1 day'")
	} else {
		Expr::cust("clock_timestamp()")
	};
	{
		let query_bind_1 = id;
		sqlx::query(
			&Query::update()
				.table(Alias::new(table))
				.value_expr(Alias::new(column), value)
				.and_where(Expr::col(Alias::new(key)).eq(Expr::value(query_bind_1.to_owned())))
				.to_string(PostgresQueryBuilder),
		)
		.execute(fixture.f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	// Retain both bodies unpolled past the independent browser/authority check.
	// Closed streams keep their connection admissions until the transport drops
	// the response bodies, even if no further body polling occurs.
	until(|| fixture.service.snapshot().registered_scopes == 0).await;
	assert_eq!(fixture.service.snapshot().event_queries, reads);
	assert_eq!(fixture.service.snapshot().backpressure_disconnects, 0);
	let blocked = fixture
		.app
		.clone()
		.native_oneshot(
			Request::get("/api/events/stream?after=-1")
				.header("authorization", format!("Bearer {}", fixture.token))
				.body(bytes::Bytes::new())
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(blocked.status, 503);
	drop(blocked);
	for mut response in responses {
		let body = tokio::time::timeout(Duration::from_secs(1), async {
			let mut bytes = bytes::BytesMut::new();
			let mut stream = response.take_stream_body().unwrap();
			while let Some(chunk) = stream.next().await {
				bytes.extend_from_slice(&chunk?);
			}
			Ok::<_, Box<dyn std::error::Error + Send + Sync>>(bytes.freeze())
		})
		.await
		.unwrap()
		.unwrap();
		assert!(!String::from_utf8_lossy(&body).contains("event: mesh"));
	}
	// The fixture credential backs the mapping, so it inherits a disabled
	// mapping or identity. A credential outside the mapping proves the closed
	// streams released their admissions.
	let replacement_token =
		common::issued_credential(&fixture.f.store, "acme", "alice").await["token"]
			.as_str()
			.unwrap()
			.to_owned();
	let replacement = fixture
		.app
		.clone()
		.native_oneshot(
			Request::get("/api/events/stream?after=-1")
				.header("authorization", format!("Bearer {replacement_token}"))
				.body(bytes::Bytes::new())
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(replacement.status, 200);
	drop(replacement);
	fixture.finish().await;
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use reinhardt::query::SimpleExpr;

#[fixture]
fn browser_sse_runtime(sse_runtime: common::RuntimeFuture) -> common::RuntimeFuture {
	async move {
		let mut runtime = sse_runtime.await;
		runtime.federation.config.oidc = Some(aidash_server::config::OidcConfig {
			issuer: "https://accounts.google.com".into(),
			client_id: "aidash".into(),
			client_secret: "fixture".into(),
			public_origin: "https://127.0.0.1:8080".into(),
			session_absolute_seconds: 43200,
			session_idle_seconds: 1800,
		});
		runtime
	}
	.boxed()
	.shared()
}
#[fixture]
fn browser_sse_fixture(
	#[from(sse_service)] service: sse::Service,
	browser_sse_runtime: common::RuntimeFuture,
	#[from(sse_fixture)]
	#[with(Duration::from_secs(60),Duration::from_secs(30),2,service.clone(),browser_sse_runtime.clone())]
	root: SseFuture,
) -> SseFuture {
	drop((service, browser_sse_runtime));
	root
}
