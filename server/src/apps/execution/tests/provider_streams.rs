//! Streamed OpenRouter completions against a scripted server-sent event fixture.
use crate::provider_fixtures::upstream_fixtures::{reply, upstream};
use aidash_application::ports::{InferenceProgressSink, ModelProvider, NoProgress};
use aidash_domain::provider::progress::InferenceProgress;
use aidash_server::{
	provider::{ModelRequest, ModelResponse, ToolSpec, provider},
	registry::{Entry, ModelConfig, validate},
};
use reinhardt::test::fixtures::server::TestServerGuard;
use reinhardt::{Response, ServerRouter};
use rstest::fixture;
use serde_json::{Value, json};
use std::{
	sync::{Arc, Mutex},
	time::Duration,
};
use tokio::sync::{Notify, mpsc};

#[derive(Clone)]
enum Step {
	Send(String),
	Sleep(Duration),
	Wait(Arc<Notify>),
}

/// Provider requests observed by a scripted completion route.
#[derive(Clone)]
struct Completions {
	sender: mpsc::UnboundedSender<Value>,
	received: Arc<Mutex<Option<mpsc::UnboundedReceiver<Value>>>>,
}

impl Completions {
	fn received(&self) -> mpsc::UnboundedReceiver<Value> {
		self.received.lock().unwrap().take().unwrap()
	}
	/// Log one provider request. Routes call this on the whole log, so their
	/// closures own the receiver too and sending succeeds even when a test
	/// never reads the log.
	fn record(&self, request: Value) {
		self.sender.send(request).unwrap();
	}
}

#[fixture]
fn completions() -> Completions {
	let (sender, received) = mpsc::unbounded_channel();
	Completions {
		sender,
		received: Arc::new(Mutex::new(Some(received))),
	}
}

/// Serve `steps` as a streamed completion and `whole` as the non-streamed one.
#[fixture]
fn stream_router(
	#[default(Vec::new())] steps: Vec<Step>,
	#[default(json!({}))] whole: Value,
	completions: Completions,
) -> Arc<ServerRouter> {
	Arc::new(
		reinhardt::test::stub::StubRouter::new()
			.route(
				"/chat/completions",
				http::Method::POST,
				reply(move |request: reinhardt::Request| {
					let body = request.json::<Value>().unwrap();
					completions.record(body.clone());
					let steps = steps.clone();
					let whole = whole.clone();
					async move { scripted(steps, whole, body) }
				}),
			)
			.into_server_router(),
	)
}

fn scripted(steps: Vec<Step>, whole: Value, body: Value) -> Response {
	let streaming = body["stream"] == true;
	if !streaming {
		return Response::ok().with_json(&whole).unwrap();
	}
	// Each step reaches the provider client as its own chunk while later steps wait.
	let stream = async_stream::stream! {
		for step in steps {
			match step {
				Step::Send(text) => yield Ok::<_, Box<dyn std::error::Error + Send + Sync>>(bytes::Bytes::from(text)),
				Step::Sleep(duration) => tokio::time::sleep(duration).await,
				Step::Wait(release) => release.notified().await,
			}
		}
	};
	Response::ok()
		.with_stream(stream)
		.with_header("Content-Type", "text/event-stream")
}

/// Answer every completion request with one whole body, ignoring `stream`.
#[fixture]
fn whole_router(
	#[default("application/json")] content_type: &'static str,
	#[default(String::new())] body: String,
	completions: Completions,
) -> Arc<ServerRouter> {
	Arc::new(
		reinhardt::test::stub::StubRouter::new()
			.route(
				"/chat/completions",
				http::Method::POST,
				reply(move |request: reinhardt::Request| {
					completions.record(request.json::<Value>().unwrap());
					let body = body.clone();
					async move {
						Response::ok()
							.with_header("Content-Type", content_type)
							.with_body(body)
					}
				}),
			)
			.into_server_router(),
	)
}

#[fixture]
fn streaming_client() -> reqwest::Client {
	// reinhardt-web#6689: the native HTTP client has a 10s total timeout.
	// Streamed completions keep the provider stall deadline as their only bound.
	reqwest::Client::builder().build().unwrap()
}

fn data(chunk: Value) -> Step {
	Step::Send(format!("data: {chunk}\n\n"))
}

fn done() -> Step {
	Step::Send("data: [DONE]\n\n".into())
}

fn keepalive() -> Step {
	Step::Send(": OPENROUTER PROCESSING\n\n".into())
}

fn delta(delta: Value) -> Step {
	data(json!({"choices":[{"index":0,"delta":delta,"finish_reason":null}]}))
}

fn finish(reason: &str) -> Step {
	data(json!({"choices":[{"index":0,"delta":{},"finish_reason":reason}]}))
}

fn usage() -> Step {
	data(json!({"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":7,"cost":0.01}}))
}

#[fixture]
fn model_config(
	#[default("http://127.0.0.1:1")] endpoint: &str,
	#[default(Some(true))] streaming: Option<bool>,
) -> ModelConfig {
	let mut config = json!({
		"provider":"openrouter", "model_id":"vendor/fixture-model",
		"endpoint":endpoint, "credential_env":null,
		"context_window":32768, "max_output_tokens":4096,
		"modalities":["text"], "cost":{}
	});
	if let Some(streaming) = streaming {
		config["streaming"] = json!(streaming);
	}
	serde_json::from_value(config).unwrap()
}

fn request(with_tools: bool) -> ModelRequest {
	ModelRequest {
		instructions: "Follow the task".into(),
		context: json!({"task":"Read notes"}).into(),
		tools: if with_tools {
			vec![ToolSpec {
				name: "read".into(),
				description: "Read notes".into(),
				parameters: json!({"type":"object","properties":{"path":{"type":"string"}}}),
			}]
		} else {
			vec![]
		},
		max_output_tokens: 512,
		content_parts: vec![],
		cache_scope: None,
		cache_breakpoints: false,
	}
}

struct Progress(mpsc::UnboundedSender<InferenceProgress>);

impl InferenceProgressSink for Progress {
	fn offer(&self, progress: InferenceProgress) {
		let _ = self.0.send(progress);
	}
}

fn model(client: &reqwest::Client, endpoint: &str, streaming: bool) -> Arc<dyn ModelProvider> {
	provider(client.clone(), model_config(endpoint, Some(streaming))).unwrap()
}

async fn infer_stream(
	server: &TestServerGuard,
	config: ModelConfig,
	client: reqwest::Client,
) -> aidash_application::Result<ModelResponse> {
	let mut config = config;
	config.endpoint = server.url.clone();
	let model = provider(client, config).unwrap();
	model.infer(request(false), &NoProgress).await
}

#[fixture]
fn release() -> Arc<Notify> {
	Arc::new(Notify::new())
}

/// A streamed completion that holds at `release`, and its whole equivalent.
#[fixture]
fn progress_script(#[default(false)] with_tools: bool, release: Arc<Notify>) -> (Vec<Step>, Value) {
	if with_tools {
		(
			vec![
				keepalive(),
				delta(json!({"role":"assistant","content":"Reading "})),
				delta(
					json!({"tool_calls":[{"index":0,"id":"call-1","type":"function",
					"function":{"name":"read","arguments":"{\"path\":"}}]}),
				),
				Step::Wait(release),
				delta(
					json!({"tool_calls":[{"index":0,"function":{"arguments":"\"private-notes\"}"}}]}),
				),
				finish("tool_calls"),
				usage(),
				done(),
			],
			json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"Reading ",
				"tool_calls":[{"id":"call-1","type":"function","function":{"name":"read",
				"arguments":"{\"path\":\"private-notes\"}"}}]}}],
				"usage":{"prompt_tokens":12,"completion_tokens":7,"cost":0.01}}),
		)
	} else {
		(
			vec![
				keepalive(),
				delta(json!({"role":"assistant","content":"Comp"})),
				Step::Wait(release),
				delta(json!({"reasoning":"private reasoning"})),
				delta(json!({"content":"leted"})),
				finish("stop"),
				usage(),
				done(),
			],
			json!({"choices":[{"finish_reason":"stop","message":{"content":"Completed"}}],
				"usage":{"prompt_tokens":12,"completion_tokens":7,"cost":0.01}}),
		)
	}
}

#[rstest::rstest]
#[case::text(false)]
#[case::tool_calls(true)]
#[tokio::test]
async fn streamed_progress_precedes_a_response_equal_to_the_non_streamed_one(
	#[case] with_tools: bool,
	release: Arc<Notify>,
	completions: Completions,
	#[from(progress_script)]
	#[with(with_tools, release.clone())]
	_script: (Vec<Step>, Value),
	#[from(stream_router)]
	#[with(_script.0.clone(), _script.1.clone(), completions.clone())]
	_router: Arc<ServerRouter>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
	streaming_client: reqwest::Client,
) {
	// Arrange
	let mut received = completions.received();
	let streamed = model(&streaming_client, &server.url, true);
	let (sender, mut offered) = mpsc::unbounded_channel();
	let sink = Progress(sender);

	// Act: progress must be observable while the response is still incomplete.
	let inference = streamed.infer(request(with_tools), &sink);
	tokio::pin!(inference);
	let first = tokio::select! {
		result = &mut inference => panic!("inference returned before progress: {result:?}"),
		first = offered.recv() => first.unwrap(),
	};
	release.notify_one();
	let response = tokio::time::timeout(Duration::from_secs(5), inference)
		.await
		.unwrap()
		.unwrap();
	let whole = model(&streaming_client, &server.url, false)
		.infer(request(with_tools), &NoProgress)
		.await
		.unwrap();

	// Assert
	let sent = received.recv().await.unwrap();
	assert_eq!(sent["stream"], true);
	assert_eq!(sent["stream_options"], json!({"include_usage": true}));
	assert_eq!(sent["provider"]["zdr"], true);
	let non_streamed = received.recv().await.unwrap();
	assert!(non_streamed.get("stream").is_none());
	assert!(non_streamed.get("stream_options").is_none());
	assert!(matches!(first, InferenceProgress::Text { .. }));
	assert_eq!(
		serde_json::to_value(&response).unwrap(),
		serde_json::to_value(&whole).unwrap()
	);
	assert_eq!((response.input_tokens, response.output_tokens), (12, 7));
	let mut progress = vec![first];
	while let Ok(item) = offered.try_recv() {
		progress.push(item);
	}
	let disclosed = serde_json::to_string(&progress).unwrap();
	for hidden in ["private-notes", "path", "private reasoning"] {
		assert!(!disclosed.contains(hidden), "disclosed {hidden}");
	}
	if with_tools {
		assert!(progress.contains(&InferenceProgress::ToolCall {
			index: 0,
			id: Some("call-1".into()),
			name: Some("read".into()),
			argument_bytes: "{\"path\":\"private-notes\"}".len() as u64,
		}));
	}
}

#[rstest::rstest]
#[case::missing_done(vec![
	delta(json!({"tool_calls":[{"index":0,"id":"call-1","function":{"name":"read","arguments":"{}"}}]})),
	finish("tool_calls"),
])]
#[case::missing_finish_reason(vec![
	delta(json!({"tool_calls":[{"index":0,"id":"call-1","function":{"name":"read","arguments":"{}"}}]})),
	done(),
])]
#[case::duplicate_call_ids(vec![
	delta(json!({"tool_calls":[{"index":0,"id":"same","function":{"name":"read","arguments":"{}"}}]})),
	delta(json!({"tool_calls":[{"index":1,"id":"same","function":{"name":"read","arguments":"{}"}}]})),
	finish("tool_calls"),
	done(),
])]
#[case::length(vec![delta(json!({"content":"partial"})), finish("length"), done()])]
#[case::refusal(vec![delta(json!({"refusal":"I cannot help"})), finish("stop"), done()])]
#[tokio::test]
async fn incomplete_or_invalid_streams_produce_no_response(
	#[case] _steps: Vec<Step>,
	#[from(stream_router)]
	#[with(_steps.clone())]
	_router: Arc<ServerRouter>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
	model_config: ModelConfig,
	streaming_client: reqwest::Client,
) {
	// Act
	let result = infer_stream(&server, model_config, streaming_client).await;

	// Assert
	assert!(
		matches!(result, Err(aidash_application::Error::External(_))),
		"{result:?}"
	);
}

#[fixture]
fn whole_completion(#[default(false)] with_tools: bool) -> Value {
	if with_tools {
		json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"Reading ",
			"tool_calls":[{"id":"call-1","type":"function","function":{"name":"read",
			"arguments":"{\"path\":\"private-notes\"}"}}]}}],
			"usage":{"prompt_tokens":12,"completion_tokens":7}})
	} else {
		json!({"choices":[{"finish_reason":"stop","message":{"content":"Completed"}}],
			"usage":{"prompt_tokens":12,"completion_tokens":7}})
	}
}

#[rstest::rstest]
#[case::text(false)]
#[case::tool_calls(true)]
#[tokio::test]
async fn streamed_requests_accept_a_whole_json_completion_without_progress(
	#[case] with_tools: bool,
	completions: Completions,
	#[from(whole_completion)]
	#[with(with_tools)]
	_whole: Value,
	#[from(whole_router)]
	#[with("application/json; charset=utf-8", _whole.to_string(), completions.clone())]
	_router: Arc<ServerRouter>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
	streaming_client: reqwest::Client,
) {
	// Arrange
	let mut received = completions.received();
	let (sender, mut offered) = mpsc::unbounded_channel();

	// Act
	let streamed = model(&streaming_client, &server.url, true)
		.infer(request(with_tools), &Progress(sender))
		.await
		.unwrap();
	let non_streamed = model(&streaming_client, &server.url, false)
		.infer(request(with_tools), &NoProgress)
		.await
		.unwrap();

	// Assert
	assert_eq!(received.recv().await.unwrap()["stream"], true);
	assert_eq!(
		serde_json::to_value(&streamed).unwrap(),
		serde_json::to_value(&non_streamed).unwrap()
	);
	assert_eq!((streamed.input_tokens, streamed.output_tokens), (12, 7));
	assert!(offered.try_recv().is_err());
}

#[rstest::rstest]
#[case::plain_text("text/plain")]
#[case::html("text/html")]
#[tokio::test]
async fn streamed_requests_reject_other_content_types(
	#[case] _content_type: &'static str,
	#[from(whole_router)]
	#[with(_content_type, json!({"choices":[{"finish_reason":"stop","message":{"content":"Completed"}}]}).to_string())]
	_router: Arc<ServerRouter>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
	streaming_client: reqwest::Client,
) {
	// Act
	let result = model(&streaming_client, &server.url, true)
		.infer(request(false), &NoProgress)
		.await;

	// Assert
	assert!(
		matches!(result, Err(aidash_application::Error::External(_))),
		"{result:?}"
	);
}

/// Comment-only keepalives that outlast the one-second stall window.
#[fixture]
fn keepalive_steps() -> Vec<Step> {
	let mut steps = vec![delta(json!({"content":"Hi"}))];
	for _ in 0..50 {
		steps.extend([keepalive(), Step::Sleep(Duration::from_millis(100))]);
	}
	steps.extend([finish("stop"), done()]);
	steps
}

#[rstest::rstest]
#[tokio::test]
async fn comment_only_keepalives_stall_the_stream(
	mut model_config: ModelConfig,
	#[from(stream_router)]
	#[with(keepalive_steps())]
	_router: Arc<ServerRouter>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
	streaming_client: reqwest::Client,
) {
	// Arrange
	model_config.stream_stall_timeout_secs = Some(1);
	let started = std::time::Instant::now();

	// Act
	let result = infer_stream(&server, model_config, streaming_client).await;

	// Assert
	assert!(
		matches!(result, Err(aidash_application::Error::InferenceStalled)),
		"{result:?}"
	);
	assert!(started.elapsed() < Duration::from_secs(4));
}

/// Reasoning chunks spaced inside the one-second stall window.
#[fixture]
fn reasoning_steps() -> Vec<Step> {
	let mut steps = vec![];
	for _ in 0..6 {
		steps.extend([
			delta(json!({"reasoning":"thinking"})),
			Step::Sleep(Duration::from_millis(400)),
		]);
	}
	steps.extend([delta(json!({"content":"Done"})), finish("stop"), done()]);
	steps
}

#[rstest::rstest]
#[tokio::test]
async fn reasoning_chunks_keep_a_slow_stream_alive(
	mut model_config: ModelConfig,
	#[from(stream_router)]
	#[with(reasoning_steps())]
	_router: Arc<ServerRouter>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
	streaming_client: reqwest::Client,
) {
	// Arrange
	model_config.stream_stall_timeout_secs = Some(1);

	// Act
	let result = infer_stream(&server, model_config, streaming_client).await;

	// Assert
	assert_eq!(result.unwrap().text, "Done");
}

/// Seventeen 64 KiB deltas: more than the 1 MiB response bound.
#[fixture]
fn oversized_steps() -> Vec<Step> {
	let piece = "x".repeat(64 * 1024);
	let mut steps: Vec<Step> = (0..17).map(|_| delta(json!({"content":piece}))).collect();
	steps.extend([finish("stop"), done()]);
	steps
}

#[rstest::rstest]
#[tokio::test]
async fn oversized_streams_fail_like_oversized_bodies(
	model_config: ModelConfig,
	#[from(stream_router)]
	#[with(oversized_steps())]
	_router: Arc<ServerRouter>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
	streaming_client: reqwest::Client,
) {
	// Act
	let result = infer_stream(&server, model_config, streaming_client).await;

	// Assert
	assert!(matches!(
		result,
		Err(aidash_application::Error::External(message)) if message == "response exceeds 1048576 bytes"
	));
}

/// A text delta followed by an in-band provider error.
#[fixture]
fn mid_stream_error_steps() -> Vec<Step> {
	vec![
		delta(json!({"content":"Hi"})),
		data(
			json!({"error":{"code":503,"message":"token=private upstream detail"},
			"choices":[{"index":0,"delta":{"content":""},"finish_reason":"error"}]}),
		),
	]
}

#[rstest::rstest]
#[tokio::test]
async fn mid_stream_errors_keep_a_safe_reason(
	model_config: ModelConfig,
	#[from(stream_router)]
	#[with(mid_stream_error_steps())]
	_router: Arc<ServerRouter>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
	streaming_client: reqwest::Client,
) {
	// Act
	let result = infer_stream(&server, model_config, streaming_client).await;

	// Assert
	assert!(matches!(
		result,
		Err(aidash_application::Error::ProviderRejected { status: 503, reason })
			if reason == "upstream rejected the request"
	));
}

#[rstest::rstest]
fn registry_validates_streaming_settings() {
	// Arrange
	let mut entry: Entry = serde_json::from_value(json!({
		"id":"stream-model", "version":"1.0.0", "kind":"model",
		"name":{"en":"Stream model"}, "description":{"en":"Test model"},
		"config":serde_json::to_value(model_config("https://openrouter.ai/api/v1", Some(false))).unwrap()
	}))
	.unwrap();

	// Act / Assert
	for streaming in [json!(true), json!(false), Value::Null] {
		entry.config["streaming"] = streaming;
		validate(&entry).unwrap();
	}
	for streaming in [json!("true"), json!(1)] {
		entry.config["streaming"] = streaming.clone();
		assert!(validate(&entry).is_err(), "accepted streaming: {streaming}");
	}
	entry.config["streaming"] = json!(true);
	for stall in [json!(1), json!(120), json!(u32::MAX), Value::Null] {
		entry.config["stream_stall_timeout_secs"] = stall;
		validate(&entry).unwrap();
	}
	for stall in [
		json!(0),
		json!(-1),
		json!(1.5),
		json!("120"),
		json!(u64::MAX),
	] {
		entry.config["stream_stall_timeout_secs"] = stall.clone();
		assert!(validate(&entry).is_err(), "accepted stall timeout: {stall}");
	}
}

#[rstest::rstest]
fn omitted_streaming_settings_stay_unserialized_and_default_on() {
	// Arrange
	let config = model_config("https://openrouter.ai/api/v1", None);

	// Act
	let serialized = serde_json::to_value(&config).unwrap();

	// Assert
	assert!(serialized.get("streaming").is_none());
	assert!(serialized.get("stream_stall_timeout_secs").is_none());
	assert!(config.streaming());
	assert_eq!(
		config.stream_stall_timeout().unwrap(),
		Duration::from_secs(120)
	);
}

#[rstest::rstest]
fn provider_rejects_a_zero_stall_timeout(
	mut model_config: ModelConfig,
	streaming_client: reqwest::Client,
) {
	// Arrange
	model_config.stream_stall_timeout_secs = Some(0);

	// Act
	let result = provider(streaming_client, model_config);

	// Assert
	assert!(matches!(result, Err(aidash_server::Error::Invalid(_))));
}
