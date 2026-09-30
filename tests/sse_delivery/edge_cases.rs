use super::*;
use sea_orm::sea_query::{Expr, LockType};
use sha2::{Digest, Sha256};

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audited_frames_allocate_sequences_only_after_serialization_lock(
	#[future(awt)] test_environment: Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(
		&test_environment,
		Duration::from_secs(60),
		Duration::from_secs(30),
		128,
	)
	.await;
	let ws = fixture.workspaces[0];
	let event = fixture.emit(ws, 1).await;
	let mut barrier = fixture.f.store.pool.begin().await.unwrap();
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
		.fetch_one(&fixture.f.store.pool)
		.await
		.unwrap();
	let mut observers = Vec::new();
	for _ in 0..2 {
		let response = fixture.open(event.sequence - 1, Some(ws), None).await;
		assert_eq!(response.status(), 200);
		observers.push(tokio::spawn(async move {
			let mut frames = response.into_body().into_data_stream().boxed();
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
					.and_where(Expr::col(Alias::new("granted")).eq(false))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&fixture.f.store.pool)
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
		.fetch_one(&fixture.f.store.pool)
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
			(event.sequence, event.cloud_event())
		);
	}
	let audits: Vec<(i64, String)> = sqlx::query_as(
		&Query::select()
			.columns([Alias::new("sequence"), Alias::new("action")])
			.from(Alias::new("authorization_decisions"))
			.and_where(Expr::col(Alias::new("sequence")).gt(before))
			.order_by(Alias::new("sequence"), sea_orm::sea_query::Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&fixture.f.store.pool)
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
	#[future(awt)] test_environment: Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(
		&test_environment,
		Duration::from_millis(250),
		Duration::from_secs(30),
		128,
	)
	.await;
	let ws = fixture.workspaces[0];
	fixture
		.f
		.store
		.message(ws, "alice", "protected message", None)
		.await
		.unwrap();
	let message = fixture.f.store.snapshot(ws).await.unwrap().messages[0].id;
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
	let mut denied = policy(&fixture.f.config.node_id);
	denied["policies"].as_array_mut().unwrap().push(json!({"id":"message-denied","effect":"deny","subjects":{"any":true},"actions":["message.read"],"resources":{"kinds":["message"],"ids":[message]}}));
	for (revision, bundle) in [(1, denied), (2, policy(&fixture.f.config.node_id))] {
		let (status, value) = request(
			&fixture.app,
			&fixture.f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":revision,"bundle":bundle}),
		)
		.await;
		assert_eq!(status, 200, "{value}");
		let allowed = fixture.emit(ws, 100 + revision).await;
		assert_eq!(
			frame(&mut stream).await,
			(allowed.sequence, allowed.cloud_event())
		);
	}
	drop(stream);
	fixture.finish().await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_flood_preserves_fallback_and_idle_revocation(
	#[future(awt)] test_environment: Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(
		&test_environment,
		Duration::from_millis(500),
		Duration::from_secs(30),
		128,
	)
	.await;
	let (stop, subscriber) = fixture.subscriber().await;
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
		fixture.f.config.node_id.trim_start_matches("aidash://")
	);
	let payload = first.cloud_event().to_string();
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
		.fetch_one(&fixture.f.store.pool)
		.await
		.unwrap();
	until(|| fixture.service.snapshot().query_causes[3] >= 4).await;
	let audit_after: i64 = sqlx::query_scalar(&audit_sql)
		.fetch_one(&fixture.f.store.pool)
		.await
		.unwrap();
	assert_eq!(
		audit_after, audit_before,
		"empty notification/fallback/authority checks do not append decisions"
	);
	let before = fixture.service.snapshot();
	assert!(before.notifications >= 100);
	assert_eq!(before.registered_scopes, 2);
	let (status, value) = request(
		&fixture.app,
		&fixture.f.config.api_token,
		"POST",
		&format!(
			"/api/authorization/acme/credentials/{}/revoke",
			fixture.credential
		),
		json!({}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	// Neither body is polled while notifications continue. Monitoring must
	// close both connections and remove registrations independently.
	until(|| fixture.service.snapshot().registered_scopes == 0).await;
	assert!(fixture.service.snapshot().authority_checks > before.authority_checks);
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
	#[future(awt)] test_environment: Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(
		&test_environment,
		Duration::from_secs(60),
		Duration::from_secs(30),
		128,
	)
	.await;
	let (stop, subscriber) = fixture.subscriber().await;
	let ws = fixture.workspaces[0];
	let before = fixture.emit(ws, 1).await;
	let mut ownership = fixture.f.store.pool.begin().await.unwrap();
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
	let response = fixture.open(before.sequence, Some(ws), None).await;
	assert_eq!(response.status(), 200);
	until(|| fixture.service.snapshot().registered_scopes == 1).await;
	let event = fixture.emit(ws, 2).await;
	let nats = async_nats::connect(&fixture.f.config.nats_url)
		.await
		.unwrap();
	hint(&nats, &fixture.f, event.cloud_event()).await;
	until(|| fixture.service.snapshot().notifications == 1).await;
	assert_eq!(fixture.service.snapshot().event_queries, 0);
	ownership.rollback().await.unwrap();
	let mut frames = response.into_body().into_data_stream().boxed();
	assert_eq!(
		frame(&mut frames).await,
		(event.sequence, event.cloud_event())
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
#[case::operator_grant_removed("operator", "enabled")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_invalidation_closes_idle_and_unpolled_buffered_streams(
	#[case] change: &str,
	#[case] column: &str,
	#[future(awt)] test_environment: Arc<TestEnvironment>,
) {
	let mut fixture = Fixture::new(
		&test_environment,
		Duration::from_secs(60),
		Duration::from_secs(30),
		2,
	)
	.await;
	let issuer = "https://accounts.google.com";
	fixture.f.config.oidc = Some(aidash::config::OidcConfig {
		issuer: issuer.into(),
		client_id: "aidash".into(),
		client_secret: "fixture".into(),
		public_origin: "http://127.0.0.1:8080".into(),
		keycloak_admin_url: String::new(),
		status_client_id: String::new(),
		status_client_secret: String::new(),
		session_absolute_seconds: 43200,
		session_idle_seconds: 1800,
	});
	fixture.app = api::router_with_event_streams(
		fixture.f.clone(),
		aidash::http::Settings {
			sse_connections: 2,
			..Default::default()
		},
		fixture.service.clone(),
	);
	let identity = Uuid::new_v4();
	let session = Uuid::new_v4();
	let mapping = Uuid::new_v4();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("dashboard_identities"))
			.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::val("browser-fixture").into(),
				Expr::cust("clock_timestamp()"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(identity)
	.bind(issuer)
	.execute(&fixture.f.store.pool)
	.await
	.unwrap();
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
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
				Expr::cust("clock_timestamp()"),
				Expr::cust("clock_timestamp()"),
				Expr::cust("clock_timestamp()+interval '12 hours'"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(session)
	.bind(Sha256::digest(b"sse-browser-session").to_vec())
	.bind(Sha256::digest(b"csrf").to_vec())
	.bind(identity)
	.execute(&fixture.f.store.pool)
	.await
	.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("dashboard_mappings"))
			.columns(["id", "identity_id", "tenant", "subject", "credential_id"].map(Alias::new))
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::val("acme").into(),
				Expr::val("alice").into(),
				Expr::cust("$3"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(mapping)
	.bind(identity)
	.bind(fixture.credential)
	.execute(&fixture.f.store.pool)
	.await
	.unwrap();
	let selector = if change == "operator" {
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("dashboard_operator_grants"))
				.columns([Alias::new("identity_id")])
				.values_panic([Expr::cust("$1")])
				.to_string(PostgresQueryBuilder),
		)
		.bind(identity)
		.execute(&fixture.f.store.pool)
		.await
		.unwrap();
		"operator".to_owned()
	} else {
		format!("mapping:{mapping}")
	};
	let mut responses = Vec::new();
	for (workspace, after) in [(fixture.workspaces[0], 0), (fixture.workspaces[1], -1)] {
		let response = fixture
			.app
			.clone()
			.oneshot(
				Request::get(format!(
					"/api/events/stream?after={after}&workspace_id={workspace}"
				))
				.header("cookie", "aidash-session=sse-browser-session")
				.header("x-aidash-context", &selector)
				.body(Body::empty())
				.unwrap(),
			)
			.await
			.unwrap();
		assert_eq!(response.status(), 200);
		responses.push(response);
	}
	until(|| fixture.service.snapshot().query_causes[0] == 3).await;
	let reads = fixture.service.snapshot().event_queries;
	assert_eq!(fixture.open(-1, None, None).await.status(), 503);
	let (table, key, id) = match change {
		"session" => ("dashboard_sessions", "id", session),
		"mapping" => ("dashboard_mappings", "id", mapping),
		"identity" => ("dashboard_identities", "id", identity),
		"operator" => ("dashboard_operator_grants", "identity_id", identity),
		_ => unreachable!(),
	};
	let value = if column == "enabled" {
		Expr::val(false).into()
	} else if column == "last_activity_at" {
		Expr::cust("clock_timestamp()-interval '1 day'")
	} else {
		Expr::cust("clock_timestamp()")
	};
	sqlx::query(
		&Query::update()
			.table(Alias::new(table))
			.value(Alias::new(column), value)
			.and_where(Expr::col(Alias::new(key)).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.execute(&fixture.f.store.pool)
	.await
	.unwrap();
	// Retain both bodies unpolled past the independent browser/authority check.
	// Closing must reclaim both admissions before either body is dropped.
	until(|| fixture.service.snapshot().registered_scopes == 0).await;
	assert_eq!(fixture.service.snapshot().event_queries, reads);
	assert_eq!(fixture.service.snapshot().backpressure_disconnects, 0);
	let operator = fixture
		.app
		.clone()
		.oneshot(
			Request::get("/api/events/stream?after=-1")
				.header(
					"authorization",
					format!("Bearer {}", fixture.f.config.api_token),
				)
				.body(Body::empty())
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(
		operator.status(),
		200,
		"both stale browser admissions are released"
	);
	for response in responses {
		let body = tokio::time::timeout(
			Duration::from_secs(1),
			axum::body::to_bytes(response.into_body(), 1024),
		)
		.await
		.unwrap()
		.unwrap();
		assert!(!String::from_utf8_lossy(&body).contains("event: mesh"));
	}
	drop(operator);
	fixture.finish().await;
}
