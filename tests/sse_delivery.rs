mod common;
#[path = "sse_delivery/edge_cases.rs"]
mod edge_cases;
#[path = "sse_delivery/process.rs"]
mod process;
use aidash::{api, domain::Event, federation::Federation, sse};
use axum::{Router, body::Body, http::Request};
use common::*;
use futures_util::{StreamExt, stream::BoxStream};
use sea_orm::sea_query::{Alias, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;
use tower::ServiceExt;
use uuid::Uuid;

type Frames = BoxStream<'static, Result<axum::body::Bytes, axum::Error>>;

struct Fixture {
	f: Federation,
	url: String,
	schema: String,
	app: Router,
	service: sse::Service,
	token: String,
	credential: Uuid,
	workspaces: Vec<Uuid>,
}
impl Fixture {
	async fn new(
		environment: &TestEnvironment,
		interval: Duration,
		timeout: Duration,
		slots: usize,
	) -> Self {
		let (mut f, url, schema) = setup(environment).await;
		f.config.node_id = format!("aidash://sse-{}", Uuid::new_v4().simple());
		f.store.node_id = f.config.node_id.clone();
		f.registry = aidash::registry::Registry::new(f.store.pool.clone(), &f.config.node_id);
		let service = sse::Service::new(sse::Settings {
			reconcile_interval: interval,
			backpressure_timeout: timeout,
		});
		let app = api::router_with_event_streams(
			f.clone(),
			aidash::http::Settings {
				sse_connections: slots,
				..Default::default()
			},
			service.clone(),
		);
		let operator = &f.config.api_token;
		let (status, value) = request(
			&app,
			operator,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":0,"bundle":policy(&f.config.node_id)}),
		)
		.await;
		assert_eq!(status, 200, "{value}");
		let (status, credential) = request(
			&app,
			operator,
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"alice"}),
		)
		.await;
		assert_eq!(status, 200, "{credential}");
		let token = credential["token"].as_str().unwrap().to_owned();
		let credential = credential["credential"]["id"]
			.as_str()
			.unwrap()
			.parse()
			.unwrap();
		let mut workspaces = Vec::new();
		for n in 0..10 {
			let (status, workspace) = request(
				&app,
				&token,
				"POST",
				"/api/workspaces",
				json!({"title":format!("SSE fixture {n}"),"goal":"delivery"}),
			)
			.await;
			assert_eq!(status, 200, "{workspace}");
			workspaces.push(workspace["id"].as_str().unwrap().parse().unwrap());
		}
		Self {
			f,
			url,
			schema,
			app,
			service,
			token,
			credential,
			workspaces,
		}
	}
	async fn open(
		&self,
		after: i64,
		workspace: Option<Uuid>,
		header: Option<&str>,
	) -> axum::response::Response {
		let mut path = format!("/api/events/stream?after={after}");
		if let Some(id) = workspace {
			path.push_str(&format!("&workspace_id={id}"));
		}
		let mut request =
			Request::get(path).header("authorization", format!("Bearer {}", self.token));
		if let Some(header) = header {
			request = request.header("last-event-id", header);
		}
		self.app
			.clone()
			.oneshot(request.body(Body::empty()).unwrap())
			.await
			.unwrap()
	}
	async fn stream(&self, after: i64, workspace: Option<Uuid>, header: Option<&str>) -> Frames {
		let response = self.open(after, workspace, header).await;
		assert_eq!(response.status(), 200);
		response.into_body().into_data_stream().boxed()
	}
	async fn subscriber(
		&self,
	) -> (
		tokio::sync::watch::Sender<bool>,
		tokio::task::JoinHandle<()>,
	) {
		let (stop, stopping) = tokio::sync::watch::channel(false);
		let service = self.service.clone();
		let url = self.f.config.nats_url.clone();
		let node = self.f.config.node_id.clone();
		let task = tokio::spawn(async move {
			service.run(&url, &node, stopping).await.unwrap();
		});
		until(|| self.service.snapshot().transport_ready).await;
		(stop, task)
	}
	async fn emit(&self, workspace: Uuid, number: usize) -> Event {
		self.f
			.store
			.emit(
				Some(workspace),
				"workspace.updated",
				json!({"id":workspace,"number":number}),
			)
			.await
			.unwrap()
	}
	async fn finish(self) {
		self.service.shutdown();
		until(|| self.service.snapshot().registered_scopes == 0).await;
		cleanup(self.f, &self.url, &self.schema).await;
	}
}
async fn until(mut predicate: impl FnMut() -> bool) {
	tokio::time::timeout(Duration::from_secs(10), async {
		while !predicate() {
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("condition deadline");
}
async fn frame(stream: &mut Frames) -> (i64, Value) {
	let bytes = tokio::time::timeout(Duration::from_secs(3), stream.next())
		.await
		.expect("notification delivery deadline")
		.expect("stream ended")
		.unwrap();
	let text = String::from_utf8(bytes.to_vec()).unwrap();
	assert!(text.contains("event: mesh"), "{text}");
	let id = text
		.lines()
		.find_map(|line| line.strip_prefix("id: "))
		.unwrap()
		.parse()
		.unwrap();
	let data = text
		.lines()
		.find_map(|line| line.strip_prefix("data: "))
		.unwrap();
	(id, serde_json::from_str(data).unwrap())
}
async fn hint(client: &async_nats::Client, f: &Federation, payload: Value) {
	client
		.publish(
			format!(
				"aidash.{}.events",
				f.config.node_id.strip_prefix("aidash://").unwrap()
			),
			payload.to_string().into(),
		)
		.await
		.unwrap();
	client.flush().await.unwrap();
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hints_are_scoped_untrusted_and_loss_recovers_from_postgres(
	#[future(awt)] test_environment: Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(
		&test_environment,
		Duration::from_secs(1),
		Duration::from_secs(30),
		128,
	)
	.await;
	let (stop, subscriber) = fixture.subscriber().await;
	let ws = fixture.workspaces[0];
	let other = fixture.workspaces[1];
	let mut selected = fixture.stream(-1, Some(ws), None).await;
	let mut all = fixture.stream(-1, None, None).await;
	let mut unrelated = fixture.stream(-1, Some(other), None).await;
	until(|| fixture.service.snapshot().query_causes[0] >= 6).await;
	let nats = async_nats::connect(&fixture.f.config.nats_url)
		.await
		.unwrap();
	// Unknown UUIDs, incorrect Workspace hints and supplied data never become frames.
	hint(&nats, &fixture.f, json!({"specversion":"1.0","id":Uuid::new_v4(),"source":fixture.f.config.node_id,"subject":ws,"sequence":i64::MAX,"data":{"secret":"forged"},"dataref":"http://127.0.0.1:1/must-not-fetch"})).await;
	let mut tx = fixture.f.store.pool.begin().await.unwrap();
	let rolled_back = fixture
		.f
		.store
		.event(
			&mut tx,
			Some(ws),
			"workspace.updated",
			json!({"secret":"uncommitted"}),
		)
		.await
		.unwrap();
	hint(&nats, &fixture.f, rolled_back.cloud_event()).await;
	assert!(
		tokio::time::timeout(Duration::from_millis(150), selected.next())
			.await
			.is_err()
	);
	tx.rollback().await.unwrap();
	hint(
		&nats,
		&fixture.f,
		json!({"id":Uuid::new_v4(),"specversion":"1.0","source":"aidash://wrong","subject":ws}),
	)
	.await;
	hint(&nats, &fixture.f, json!("malformed")).await;
	let first = fixture.emit(ws, 1).await;
	let second = fixture.emit(ws, 2).await;
	// Duplicate and reverse order are merely wakeups; only canonical order is emitted.
	hint(&nats, &fixture.f, second.cloud_event()).await;
	hint(&nats, &fixture.f, first.cloud_event()).await;
	hint(&nats, &fixture.f, second.cloud_event()).await;
	for stream in [&mut selected, &mut all] {
		for event in [&first, &second] {
			assert_eq!(frame(stream).await, (event.sequence, event.cloud_event()));
		}
	}
	assert!(
		tokio::time::timeout(Duration::from_millis(100), unrelated.next())
			.await
			.is_err()
	);
	assert_eq!(fixture.service.snapshot().rejected_notifications, 2);
	// No hint for the final committed event. Independent fallback must recover it.
	let missed = fixture.emit(ws, 3).await;
	let began = Instant::now();
	assert_eq!(frame(&mut selected).await.0, missed.sequence);
	assert!(began.elapsed() < Duration::from_millis(1500));
	assert!(fixture.service.snapshot().query_causes[3] > 0);
	stop.send_replace(true);
	subscriber.await.unwrap();
	drop((selected, all, unrelated));
	fixture.finish().await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bounded_pages_drain_denied_history_and_preserve_cursor_contract(
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
	let mut tx = fixture.f.store.pool.begin().await.unwrap();
	let mut expected = Vec::new();
	for n in 0..550 {
		let kind = if n < 325 {
			"unrecognized.private-event"
		} else {
			"workspace.updated"
		};
		let event = fixture
			.f
			.store
			.event(&mut tx, Some(ws), kind, json!({"id":ws,"number":n}))
			.await
			.unwrap();
		if n >= 325 {
			expected.push(event);
		}
	}
	tx.commit().await.unwrap();
	let created = fixture.f.store.events(0, Some(ws), 1).await.unwrap()[0].sequence;
	let began = Instant::now();
	let mut stream = fixture
		.stream(0, Some(ws), Some(&created.to_string()))
		.await;
	for event in &expected {
		assert_eq!(
			frame(&mut stream).await,
			(event.sequence, event.cloud_event())
		);
	}
	assert!(
		began.elapsed() < Duration::from_secs(10),
		"backlog must not wait for the 60s fallback"
	);
	assert_eq!(fixture.service.snapshot().query_causes[3], 0);
	assert!(fixture.service.snapshot().query_causes[4] >= 5);
	let mut malformed = fixture
		.stream(created, Some(ws), Some("not-a-number"))
		.await;
	assert_eq!(frame(&mut malformed).await.0, expected[0].sequence);
	let mut future = fixture.stream(i64::MAX, Some(ws), None).await;
	let mut tail = fixture.stream(-1, Some(ws), None).await;
	assert!(
		tokio::time::timeout(Duration::from_millis(100), future.next())
			.await
			.is_err()
	);
	assert!(
		tokio::time::timeout(Duration::from_millis(100), tail.next())
			.await
			.is_err()
	);
	drop((stream, malformed, future, tail));
	fixture.finish().await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unpolled_output_expires_but_idle_streams_and_authority_checks_remain_live(
	#[future(awt)] test_environment: Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(
		&test_environment,
		Duration::from_secs(60),
		Duration::from_secs(1),
		2,
	)
	.await;
	let (stop, subscriber) = fixture.subscriber().await;
	let ws = fixture.workspaces[0];
	let response = fixture.open(0, Some(ws), None).await;
	assert_eq!(response.status(), 200); // Intentionally never poll the body.
	let mut idle = fixture.stream(-1, Some(fixture.workspaces[1]), None).await;
	assert_eq!(fixture.open(-1, None, None).await.status(), 503);
	let healthy = fixture.emit(fixture.workspaces[1], 42).await;
	let nats = async_nats::connect(&fixture.f.config.nats_url)
		.await
		.unwrap();
	hint(&nats, &fixture.f, healthy.cloud_event()).await;
	assert_eq!(
		frame(&mut idle).await.0,
		healthy.sequence,
		"an unread body must not block other readers"
	);
	until(|| fixture.service.snapshot().backpressure_disconnects == 1).await;
	assert_eq!(fixture.service.snapshot().registered_scopes, 1);
	let replacement = fixture.open(-1, None, None).await;
	assert_eq!(
		replacement.status(),
		200,
		"release admission even with the original body retained"
	);
	until(|| fixture.service.snapshot().query_causes[0] == 5).await;
	let before = fixture.service.snapshot();
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
	until(|| fixture.service.snapshot().registered_scopes == 0).await;
	let after = fixture.service.snapshot();
	assert!(after.authority_checks > before.authority_checks);
	assert_eq!(
		after.event_queries, before.event_queries,
		"revocation does not need an event query"
	);
	drop((response, idle, replacement));
	stop.send_replace(true);
	subscriber.await.unwrap();
	fixture.finish().await;
}

#[rstest::rstest]
#[case::resume(false)]
#[case::cancel_unpolled_body(true)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closed_gate_suppresses_reads_and_resumes_without_a_new_hint(
	#[case] cancel: bool,
	#[future(awt)] test_environment: Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(
		&test_environment,
		Duration::from_secs(60),
		Duration::from_secs(1),
		128,
	)
	.await;
	let ws = fixture.workspaces[0];
	fixture.emit(ws, 1).await;
	let mut stream = fixture.stream(0, Some(ws), None).await;
	frame(&mut stream).await;
	let mut gate = fixture.f.store.control_pool.begin().await.unwrap();
	// Hold the real durable gate's exclusive row lock. No production test hooks.
	sqlx::query(
		&Query::select()
			.column(Alias::new("singleton"))
			.from(Alias::new("atomic_gate"))
			.lock(sea_orm::sea_query::LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *gate)
	.await
	.unwrap();
	let before = fixture.service.snapshot();
	assert!(
		tokio::time::timeout(Duration::from_millis(1300), stream.next())
			.await
			.is_err()
	);
	if cancel {
		assert_eq!(fixture.service.snapshot().visibility_waiters, 1);
		fixture.service.shutdown();
		until(|| fixture.service.snapshot().registered_scopes == 0).await;
		assert_eq!(
			fixture.service.snapshot().visibility_waiters,
			0,
			"shutdown clears a body gate wait without another body poll"
		);
		assert_eq!(fixture.service.snapshot().backpressure_disconnects, 0);
		drop(stream);
		gate.rollback().await.unwrap();
		fixture.finish().await;
		return;
	}
	let keepalive = tokio::time::timeout(Duration::from_secs(16), stream.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	assert!(
		keepalive.starts_with(b":"),
		"visibility wait preserves idle keepalives"
	);
	let after = fixture.service.snapshot();
	assert_eq!(after.event_queries, before.event_queries);
	assert_eq!(after.visibility_waiters, 1);
	assert!(after.gate_checks >= before.gate_checks + 3);
	assert!(after.authority_checks >= before.authority_checks + 3);
	assert_eq!(
		after.backpressure_disconnects, 0,
		"server gate wait is not client backpressure"
	);
	gate.rollback().await.unwrap();
	assert_eq!(frame(&mut stream).await.1["data"]["number"], 1);
	assert_eq!(fixture.service.snapshot().visibility_waiters, 0);
	drop(stream);
	fixture.finish().await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn global_stream_filters_revoked_workspaces_without_rewinding_on_restore(
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
	let mut all = fixture.stream(-1, None, None).await;
	let mut selected = fixture.stream(-1, Some(ws), None).await;
	let mut denied = policy(&fixture.f.config.node_id);
	denied["policies"].as_array_mut().unwrap().push(json!({"id":"revoked","effect":"deny","subjects":{"any":true},"actions":["workspace.events"],"resources":{"kinds":["workspace"],"ids":[ws]}}));
	for (revision, bundle) in [
		(1, denied),
		(
			2,
			json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[]}),
		),
		(3, policy(&fixture.f.config.node_id)),
	] {
		let (status, value) = request(
			&fixture.app,
			&fixture.f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":revision,"bundle":bundle}),
		)
		.await;
		assert_eq!(status, 200, "{value}");
		match revision {
			1 => {
				fixture.emit(ws, 11).await;
				let allowed = fixture.emit(fixture.workspaces[1], 12).await;
				assert_eq!(frame(&mut all).await.0, allowed.sequence);
				let terminal = tokio::time::timeout(Duration::from_secs(2), selected.next())
					.await
					.unwrap()
					.unwrap()
					.unwrap();
				assert!(String::from_utf8_lossy(&terminal).contains("event: error"));
				assert!(selected.next().await.is_none());
			}
			2 => {
				assert!(
					tokio::time::timeout(Duration::from_millis(750), all.next())
						.await
						.is_err()
				);
				assert_eq!(
					fixture.service.snapshot().registered_scopes,
					1,
					"valid identity with no Workspaces stays connected"
				);
			}
			3 => {
				let future = fixture.emit(ws, 13).await;
				assert_eq!(
					frame(&mut all).await.0,
					future.sequence,
					"restoration must not rewind past already scanned history"
				);
			}
			_ => unreachable!(),
		}
	}
	// Existing oversized canonical records remain deliverable, without buffering
	// unbounded numbers of them in the subscriber or serializing a whole page.
	let large = fixture
		.f
		.store
		.emit(
			Some(ws),
			"workspace.updated",
			json!({"id":ws,"text":"x".repeat(300_000)}),
		)
		.await
		.unwrap();
	assert_eq!(frame(&mut all).await, (large.sequence, large.cloud_event()));
	drop((all, selected));
	fixture.finish().await;
}
