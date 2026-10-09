//! `GET /api/runs/{run}/inference/stream` delivery, replay and closure.
use super::{Conversation, common};
use aidash_domain::provider::progress::{InterruptionReason, ProgressOutcome};
use aidash_server::sse;
use axum::{body::Body, http::Request};
use common::{TestEnvironment, request, test_environment};
use futures_util::{StreamExt, stream::BoxStream};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

type Frames = BoxStream<'static, Result<axum::body::Bytes, axum::Error>>;

/// One parsed SSE frame: `event`, optional `id` and JSON `data`.
#[derive(Debug)]
struct Frame {
	event: String,
	id: Option<i64>,
	data: Value,
}

struct Stream {
	body: Frames,
	buffer: String,
}

impl Stream {
	/// The next non-comment frame, or `None` when the stream ends first.
	async fn next(&mut self) -> Option<Frame> {
		loop {
			while let Some(end) = self.buffer.find("\n\n") {
				let raw: String = self.buffer.drain(..end + 2).collect();
				let mut event = String::from("message");
				let mut id = None;
				let mut data = None;
				for line in raw.lines() {
					if let Some(value) = line.strip_prefix("event: ") {
						event = value.to_owned();
					} else if let Some(value) = line.strip_prefix("id: ") {
						id = Some(value.parse().unwrap());
					} else if let Some(value) = line.strip_prefix("data: ") {
						data = Some(serde_json::from_str(value).unwrap_or(json!(value)));
					}
				}
				if let Some(data) = data {
					return Some(Frame { event, id, data });
				}
			}
			let chunk = tokio::time::timeout(Duration::from_secs(10), self.body.next())
				.await
				.expect("frame delivery deadline")?
				.unwrap();
			self.buffer.push_str(std::str::from_utf8(&chunk).unwrap());
		}
	}

	async fn frame(&mut self) -> Frame {
		self.next().await.expect("stream ended")
	}

	/// No frame arrives within `window`.
	async fn quiet(&mut self, window: Duration) {
		if let Ok(frame) = tokio::time::timeout(window, self.next()).await {
			panic!("unexpected frame {frame:?}");
		}
	}
}

struct Fixture {
	c: Conversation,
	service: sse::Service,
}

impl Fixture {
	async fn new(environment: &TestEnvironment, backpressure: Duration) -> Self {
		let service = sse::Service::new(sse::Settings {
			reconcile_interval: Duration::from_millis(250),
			backpressure_timeout: backpressure,
		});
		let c = Conversation::new(environment, Some(service.clone())).await;
		Self { c, service }
	}

	async fn open(&self, after: Option<i64>, header: Option<i64>) -> axum::response::Response {
		let mut path = format!("/api/runs/{}/inference/stream", self.c.run.id);
		if let Some(after) = after {
			path.push_str(&format!("?after={after}"));
		}
		let mut request =
			Request::get(path).header("authorization", format!("Bearer {}", self.c.token));
		if let Some(header) = header {
			request = request.header("last-event-id", header.to_string());
		}
		self.c
			.app
			.clone()
			.oneshot(request.body(Body::empty()).unwrap())
			.await
			.unwrap()
	}

	async fn stream(&self, after: Option<i64>, header: Option<i64>) -> Stream {
		let response = self.open(after, header).await;
		assert_eq!(response.status(), 200);
		Stream {
			body: response.into_body().into_data_stream().boxed(),
			buffer: String::new(),
		}
	}

	async fn finish(self) {
		self.service.shutdown();
		until(|| self.service.snapshot().registered_scopes == 0).await;
		self.c.finish().await;
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

fn stall() -> ProgressOutcome {
	ProgressOutcome::Interrupted(InterruptionReason::Stall)
}

#[rstest::rstest]
#[case::last_event_id(None, Some(3))]
#[case::after_query(Some(3), None)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replay_resumes_after_the_cursor_without_duplicates(
	#[case] after: Option<i64>,
	#[case] header: Option<i64>,
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	// Arrange
	let fixture = Fixture::new(&test_environment, Duration::from_secs(30)).await;
	let c = &fixture.c;
	let worker = Uuid::new_v4();
	c.lease(worker).await;
	let attempt = c.start(worker).await;
	for text in ["one", "two", "three"] {
		c.text(worker, attempt, text).await;
	}
	c.f.store
		.finish_inference(c.run.id, worker, attempt, stall())
		.await
		.unwrap();
	let mut full = fixture.stream(None, None).await;
	let mut first = Vec::new();
	for _ in 0..5 {
		first.push(full.frame().await);
	}

	// Act
	let mut resumed = fixture.stream(after, header).await;

	// Assert
	assert_eq!(
		first.iter().map(|frame| frame.id).collect::<Vec<_>>(),
		[Some(1), Some(2), Some(3), Some(4), Some(5)]
	);
	assert_eq!(first[0].event, "inference.outcome");
	assert_eq!(first[0].data["data"]["item"], json!({"outcome":"pending"}));
	assert_eq!(first[1].event, "inference.delta");
	assert_eq!(first[1].data["type"], "aidash.inference.progress.v1");
	assert_eq!(first[1].data["subject"], json!(c.run.id));
	assert_eq!(first[1].data["data"]["attempt_id"], json!(attempt.0));
	assert_eq!(first[1].data["data"]["item"]["text"], "one");
	assert_eq!(
		first[4].data["data"]["item"],
		json!({"outcome":"interrupted","reason":"stall"})
	);
	let replay = [resumed.frame().await, resumed.frame().await];
	assert_eq!(replay[0].id, Some(4));
	assert_eq!(replay[0].data["data"]["item"]["text"], "three");
	assert_eq!(replay[1].id, Some(5));
	resumed.quiet(Duration::from_millis(750)).await;
	drop((full, resumed));
	fixture.finish().await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_cursor_the_stream_starts_at_the_latest_attempt(
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	// Arrange
	let fixture = Fixture::new(&test_environment, Duration::from_secs(30)).await;
	let c = &fixture.c;
	let worker = Uuid::new_v4();
	c.lease(worker).await;
	let earlier = c.start(worker).await;
	c.text(worker, earlier, "earlier").await;
	c.f.store
		.finish_inference(c.run.id, worker, earlier, stall())
		.await
		.unwrap();
	let latest = c.start(worker).await;

	// Act
	let mut stream = fixture.stream(None, None).await;
	let started = stream.frame().await;
	c.text(worker, latest, "live").await;
	let live = stream.frame().await;

	// Assert
	assert_eq!(started.id, Some(4));
	assert_eq!(started.data["data"]["attempt_id"], json!(latest.0));
	assert_eq!(started.data["data"]["kind"], "started");
	assert_eq!(live.id, Some(5));
	assert_eq!(live.event, "inference.delta");
	assert_eq!(live.data["data"]["item"]["text"], "live");
	drop(stream);
	fixture.finish().await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gap_replaces_pruned_rows_and_outcomes_are_still_delivered(
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	// Arrange
	let fixture = Fixture::new(&test_environment, Duration::from_secs(30)).await;
	let c = &fixture.c;
	let worker = Uuid::new_v4();
	c.lease(worker).await;
	let attempt = c.start(worker).await;
	c.text(worker, attempt, "first").await;
	c.text(worker, attempt, "second").await;
	c.f.store
		.finish_inference(c.run.id, worker, attempt, stall())
		.await
		.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("inference_attempts"))
			.value_expr(
				Alias::new("finished_at"),
				Expr::cust("CURRENT_TIMESTAMP - INTERVAL '16 minutes'"),
			)
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(attempt.0)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(c.f.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(c.f.store.prune_inference_progress().await.unwrap(), 1);
	assert_eq!(c.f.store.prune_inference_progress().await.unwrap(), 0);

	// Act
	let mut stream = fixture.stream(None, Some(0)).await;
	let frames = [
		stream.frame().await,
		stream.frame().await,
		stream.frame().await,
	];

	// Assert
	assert_eq!(frames[0].id, Some(1));
	assert_eq!(frames[0].data["data"]["kind"], "started");
	assert_eq!(frames[1].event, "gap");
	assert_eq!(frames[1].id, None);
	assert_eq!(
		frames[1].data,
		json!({"attempt_id":attempt.0,"outcome":"interrupted","from":2,"to":3})
	);
	assert_eq!(frames[2].id, Some(4));
	assert_eq!(frames[2].event, "inference.outcome");
	assert_eq!(
		frames[2].data["data"]["item"],
		json!({"outcome":"interrupted","reason":"stall"})
	);
	drop(stream);
	fixture.finish().await;
}

#[derive(Debug, Clone, Copy)]
enum Revocation {
	RunRead,
	Credential,
}

#[rstest::rstest]
#[case::run_read(Revocation::RunRead, 403)]
#[case::credential(Revocation::Credential, 401)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revocation_closes_the_stream_and_denies_reconnect(
	#[case] revocation: Revocation,
	#[case] reconnect: u16,
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	// Arrange
	let fixture = Fixture::new(&test_environment, Duration::from_secs(30)).await;
	let c = &fixture.c;
	let worker = Uuid::new_v4();
	c.lease(worker).await;
	let attempt = c.start(worker).await;
	let mut stream = fixture.stream(None, None).await;
	assert_eq!(stream.frame().await.id, Some(1));
	let operator = &c.f.config.api_token;

	// Act
	match revocation {
		Revocation::RunRead => {
			let mut denied = common::policy(&c.f.config.node_id);
			denied["policies"].as_array_mut().unwrap().push(json!({"id":"revoked","effect":"deny","subjects":{"any":true},"actions":["workspace.events"],"resources":{"kinds":["workspace"],"ids":[c.run.workspace_id]}}));
			let (status, value) = request(
				&c.app,
				operator,
				"POST",
				"/api/authorization/acme",
				json!({"expected_revision":1,"bundle":denied}),
			)
			.await;
			assert_eq!(status, 200, "{value}");
		}
		Revocation::Credential => {
			let (status, credentials) = request(
				&c.app,
				operator,
				"GET",
				"/api/authorization/acme/credentials",
				json!(null),
			)
			.await;
			assert_eq!(status, 200, "{credentials}");
			let listed = credentials
				.as_array()
				.or_else(|| credentials["items"].as_array())
				.expect("credential list");
			for credential in listed {
				let (status, value) = request(
					&c.app,
					operator,
					"POST",
					&format!(
						"/api/authorization/acme/credentials/{}/revoke",
						credential["id"].as_str().unwrap()
					),
					json!({}),
				)
				.await;
				assert_eq!(status, 200, "{value}");
			}
		}
	}
	// A pending attempt keeps producing frames; none may pass the new authority.
	c.text(worker, attempt, "after revocation").await;

	// Assert
	let mut closing = Vec::new();
	while let Some(frame) = stream.next().await {
		closing.push(frame);
	}
	assert!(
		closing.iter().all(|frame| frame.id.is_none()),
		"no progress after revocation: {closing:?}"
	);
	assert_eq!(fixture.open(None, None).await.status(), reconnect);
	fixture.finish().await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unread_stream_is_closed_by_backpressure(
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	// Arrange
	let fixture = Fixture::new(&test_environment, Duration::from_secs(1)).await;
	let c = &fixture.c;
	let worker = Uuid::new_v4();
	c.lease(worker).await;
	let attempt = c.start(worker).await;
	let text = "x".repeat(8 * 1024);
	for _ in 0..32 {
		c.text(worker, attempt, &text).await;
	}

	// Act
	let response = fixture.open(None, None).await;
	assert_eq!(response.status(), 200); // Intentionally never poll the body.

	// Assert
	until(|| {
		let snapshot = fixture.service.snapshot();
		snapshot.backpressure_disconnects == 1 && snapshot.registered_scopes == 0
	})
	.await;
	let mut healthy = fixture.stream(None, None).await;
	assert_eq!(healthy.frame().await.id, Some(1));
	drop((response, healthy));
	fixture.finish().await;
}
