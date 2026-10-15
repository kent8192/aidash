//! A worker's streamed provider reply reaches the Run stream while the
//! provider is still sending, and is then accepted exactly once.
use super::{
	common,
	stream::{Frame, Stream, until},
};
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::{domain::qualified_agent, federation::Federation, harness::Harness, sse};
use common::upstream_fixtures::{reply, upstream};
use common::{bootstrap, cleanup, request};
use futures_util::FutureExt;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use reinhardt::test::fixtures::server::TestServerGuard;
use reinhardt::{Response, ServerRouter};
use rstest::fixture;
use serde_json::{Value, json};
use std::{
	sync::{Arc, Mutex},
	time::Duration,
};
use tokio::{sync::oneshot, task::JoinSet, time::timeout};
use uuid::Uuid;

const REPLY: &str = "Hello world";

/// One streamed `/v1/chat/completions` reply that holds its completion until
/// released: a role chunk and two text deltas, then (after the release) the
/// `stop` chunk with usage and `[DONE]`.
struct Provider {
	endpoint: String,
	/// The provider request body, sent once the worker reaches the provider.
	entered: oneshot::Receiver<Value>,
	release: Option<oneshot::Sender<()>>,
	_server: TestServerGuard,
}

type Gates = (oneshot::Sender<Value>, oneshot::Receiver<()>);

/// Provider-side gates and the test-side ends that observe and release them.
#[derive(Clone)]
struct ProviderSignals {
	gates: Arc<Mutex<Option<Gates>>>,
	entered: Arc<Mutex<Option<oneshot::Receiver<Value>>>>,
	release: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

#[fixture]
fn provider_signals() -> ProviderSignals {
	let (entered_tx, entered) = oneshot::channel();
	let (release, released) = oneshot::channel();
	ProviderSignals {
		gates: Arc::new(Mutex::new(Some((entered_tx, released)))),
		entered: Arc::new(Mutex::new(Some(entered))),
		release: Arc::new(Mutex::new(Some(release))),
	}
}

#[fixture]
fn provider_router(provider_signals: ProviderSignals) -> Arc<ServerRouter> {
	let gates = provider_signals.gates;
	Arc::new(
		reinhardt::test::stub::StubRouter::new()
			.route(
				"/v1/chat/completions",
				http::Method::POST,
				reply(move |request: reinhardt::Request| {
					let body = request.json::<Value>().unwrap();
					let gates = gates
						.lock()
						.unwrap()
						.take()
						.expect("exactly one provider call");
					async move { streamed_reply(body, gates) }
				}),
			)
			.into_server_router(),
	)
}

#[fixture]
async fn provider(
	provider_signals: ProviderSignals,
	#[from(provider_router)]
	#[with(provider_signals.clone())]
	_router: Arc<ServerRouter>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
) -> Provider {
	Provider {
		endpoint: server.url.clone(),
		entered: provider_signals.entered.lock().unwrap().take().unwrap(),
		release: provider_signals.release.lock().unwrap().take(),
		_server: server,
	}
}

#[fixture]
fn long_lease_runtime(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> common::RuntimeFuture {
	async move {
		let mut runtime = runtime.await;
		// Inference must not wait for or race a lease heartbeat.
		runtime.federation.config.lease_seconds = 300;
		runtime
	}
	.boxed()
	.shared()
}

type Chunk = Result<bytes::Bytes, Box<dyn std::error::Error + Send + Sync>>;

fn chunk(value: Value) -> Chunk {
	Ok(bytes::Bytes::from(format!("data: {value}\n\n")))
}

fn delta(delta: Value) -> Chunk {
	chunk(json!({"choices":[{"index":0,"delta":delta,"finish_reason":null}]}))
}

fn streamed_reply(body: Value, (entered, released): Gates) -> Response {
	entered.send(body).unwrap();
	let stream = async_stream::stream! {
		yield delta(json!({"role":"assistant"}));
		yield delta(json!({"content":"Hello "}));
		yield delta(json!({"content":"world"}));
		released.await.unwrap();
		yield chunk(json!({
			"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
			"usage":{"prompt_tokens":12,"completion_tokens":2}
		}));
		yield Ok(bytes::Bytes::from_static(b"data: [DONE]\n\n"));
	};
	Response::ok()
		.with_stream(stream)
		.with_header("Content-Type", "text/event-stream")
}

/// Admit `research@1.0.1`, whose model `model@1.0.1` streams its replies, as
/// a subject of the bootstrap `policy`.
async fn admit_streaming_agent(f: &Federation, app: &common::TestApplication, mut policy: Value) {
	let operator = &f.config.api_token;
	let subject = qualified_agent(&f.config.node_id, "research", "1.0.1");
	policy["subjects"][subject] = json!({"kind":"agent"});
	let (status, body) = request(
		app,
		operator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let mut model = f.registry.get("model", "1.0.0").await.unwrap();
	model.version = "1.0.1".into();
	model.config["streaming"] = json!(true);
	let mut agent = f.registry.get("research", "1.0.0").await.unwrap();
	agent.version = "1.0.1".into();
	agent.binding_normalization = None;
	agent.config["model"]["version"] = json!("1.0.1");
	for (id, entry) in [("model", json!(model)), ("research", json!(agent))] {
		let (status, body) = request(app, operator, "POST", "/api/registry", entry).await;
		assert_eq!(status, 200, "{body}");
		let (status, body) = request(
			app,
			operator,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":id,"version":"1.0.1"},"expected_revision":0,"enabled":true}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
}

fn text(frames: &[Frame]) -> String {
	frames
		.iter()
		.map(|frame| frame.data["data"]["item"]["text"].as_str().unwrap())
		.collect()
}

fn is_accepted(frame: &Frame) -> bool {
	frame.event == "inference.outcome"
		&& frame.data["data"]["item"] == json!({"outcome":"accepted"})
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamed_reply_is_shown_live_and_accepted_once(
	#[from(super::progress_service)] service: sse::Service,
	#[from(long_lease_runtime)] _runtime: common::RuntimeFuture,
	#[future(awt)]
	#[from(common::native_application)]
	#[with(aidash_server::http::Settings::default(), service.clone(), Arc::new(|router| router), _runtime.clone())]
	fixture: common::ApplicationFixture,
	#[future(awt)] provider: Provider,
) {
	// Arrange
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let mut provider = provider;
	let (policy, token, task_id) = bootstrap(&f, &app, &provider.endpoint).await;
	admit_streaming_agent(&f, &app, policy).await;
	let (status, claimed) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.1"}}),
	)
	.await;
	assert_eq!(status, 200, "{claimed}");
	let harness = Harness {
		federation: f.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	assert_eq!(run.phase().as_str(), "THINKING");
	let mut workers = JoinSet::new();
	let worker = harness.clone();
	workers.spawn(async move { worker.worker_once().await });
	let sent = timeout(Duration::from_secs(10), &mut provider.entered)
		.await
		.expect("worker must reach the provider")
		.unwrap();
	assert_eq!(sent["stream"], true);

	// Act
	let mut live = Stream::open(&app, &token, run.id, None, None).await;
	let started = live.frame().await;
	// Text deltas may be coalesced into fewer rows; read until the reply is whole.
	let mut deltas = Vec::new();
	while text(&deltas) != REPLY {
		let frame = live.frame().await;
		assert_eq!(frame.event, "inference.delta", "{frame:?}");
		deltas.push(frame);
		assert!(REPLY.starts_with(&text(&deltas)), "{deltas:?}");
	}
	provider.release.take().unwrap().send(()).unwrap();
	let stepped = timeout(Duration::from_secs(10), workers.join_next())
		.await
		.expect("worker must finish once the provider completes")
		.unwrap()
		.unwrap()
		.unwrap();
	let outcome = live.frame().await;
	let first_delta = deltas[0].id.unwrap();
	let mut resumed = Stream::open(&app, &token, run.id, None, Some(first_delta)).await;
	let mut replay = Vec::new();
	while replay.last().is_none_or(|frame| !is_accepted(frame)) {
		replay.push(resumed.frame().await);
	}
	resumed.quiet(Duration::from_millis(750)).await;

	// Assert
	let attempts: Vec<(Uuid, Option<String>)> = sqlx::query_as(
		&Query::select()
			.columns([Alias::new("id"), Alias::new("outcome")])
			.from(Alias::new("inference_attempts"))
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(f.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(attempts.len(), 1, "{attempts:?}");
	let (attempt, accepted) = attempts[0].clone();
	assert_eq!(accepted.as_deref(), Some("accepted"));
	assert_eq!(started.event, "inference.outcome");
	assert_eq!(started.data["data"]["kind"], "started");
	assert_eq!(started.data["data"]["attempt_id"], json!(attempt));
	assert!(started.id < deltas[0].id);
	assert!(
		deltas
			.iter()
			.all(|frame| frame.data["data"]["attempt_id"] == json!(attempt))
	);
	assert!(stepped);
	let current = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(current.phase().as_str(), "TOOL_CALL");
	let response = &json!(current.state)["data"]["response"];
	assert_eq!(response["text"], REPLY);
	assert_eq!(response["tool_calls"], json!([]));
	assert_eq!(
		[&response["input_tokens"], &response["output_tokens"]],
		[&json!(12), &json!(2)]
	);
	assert!(is_accepted(&outcome), "{outcome:?}");
	assert_eq!(outcome.data["data"]["attempt_id"], json!(attempt));
	assert!(outcome.id > deltas.last().unwrap().id);
	let mut markers: Vec<String> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("kind"))
			.from(Alias::new("events"))
			.and_where(Expr::cust_with_values(
				"data->>'attempt_id' = ?",
				[attempt.to_string()],
			))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(f.store.pool.driver())
	.await
	.unwrap();
	markers.sort();
	assert_eq!(markers, ["inference.accepted", "inference.started"]);
	assert!(
		replay.iter().all(|frame| frame.id > Some(first_delta)),
		"{replay:?}"
	);
	let replayed: Vec<Frame> = replay
		.into_iter()
		.filter(|frame| frame.event == "inference.delta")
		.collect();
	assert_eq!(text(&deltas[..1]) + &text(&replayed), REPLY);

	drop((live, resumed));
	service.shutdown();
	until(|| service.snapshot().registered_scopes == 0).await;
	cleanup(f, &url, &schema).await;
}
