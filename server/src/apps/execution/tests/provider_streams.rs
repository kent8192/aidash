//! Streamed OpenRouter completions against a scripted server-sent event fixture.
use aidash_application::ports::{InferenceProgressSink, ModelProvider, NoProgress};
use aidash_domain::provider::progress::InferenceProgress;
use aidash_server::{
	provider::{ModelRequest, ModelResponse, ToolSpec, provider},
	registry::{Entry, ModelConfig, validate},
};
use axum::{
	Json, Router,
	body::Body,
	http::header,
	response::{IntoResponse, Response},
	routing::post,
};
use rstest::fixture;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Notify, mpsc};

#[derive(Clone)]
enum Step {
	Send(String),
	Sleep(Duration),
	Wait(Arc<Notify>),
}

struct Fixture {
	endpoint: String,
	received: mpsc::UnboundedReceiver<Value>,
	server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
	fn drop(&mut self) {
		self.server.abort();
	}
}

/// Serve `steps` as a streamed completion and `whole` as the non-streamed one.
async fn serve(steps: Vec<Step>, whole: Value) -> Fixture {
	let (sender, received) = mpsc::unbounded_channel();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let app = Router::new().route(
		"/chat/completions",
		post(move |Json(body): Json<Value>| {
			let sender = sender.clone();
			let steps = steps.clone();
			let whole = whole.clone();
			async move { reply(sender, steps, whole, body) }
		}),
	);
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	Fixture {
		endpoint,
		received,
		server,
	}
}

fn reply(
	sender: mpsc::UnboundedSender<Value>,
	steps: Vec<Step>,
	whole: Value,
	body: Value,
) -> Response {
	let streaming = body["stream"] == true;
	sender.send(body).unwrap();
	if !streaming {
		return Json(whole).into_response();
	}
	let stream = async_stream::stream! {
		for step in steps {
			match step {
				Step::Send(text) => yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(text)),
				Step::Sleep(duration) => tokio::time::sleep(duration).await,
				Step::Wait(release) => release.notified().await,
			}
		}
	};
	(
		[(header::CONTENT_TYPE, "text/event-stream")],
		Body::from_stream(stream),
	)
		.into_response()
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
		context: json!({"task":"Read notes"}),
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
	}
}

struct Progress(mpsc::UnboundedSender<InferenceProgress>);

impl InferenceProgressSink for Progress {
	fn offer(&self, progress: InferenceProgress) {
		let _ = self.0.send(progress);
	}
}

fn model(endpoint: &str, streaming: bool) -> Arc<dyn ModelProvider> {
	provider(
		reqwest::Client::new(),
		model_config(endpoint, Some(streaming)),
	)
	.unwrap()
}

/// Answer every completion request with one whole body, ignoring `stream`.
async fn serve_whole(content_type: &'static str, body: String) -> Fixture {
	let (sender, received) = mpsc::unbounded_channel();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let app = Router::new().route(
		"/chat/completions",
		post(move |Json(request): Json<Value>| {
			let sender = sender.clone();
			let body = body.clone();
			async move {
				sender.send(request).unwrap();
				([(header::CONTENT_TYPE, content_type)], body).into_response()
			}
		}),
	);
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	Fixture {
		endpoint,
		received,
		server,
	}
}

async fn infer_stream(
	steps: Vec<Step>,
	config: ModelConfig,
) -> aidash_application::Result<ModelResponse> {
	let fixture = serve(steps, json!({})).await;
	let mut config = config;
	config.endpoint = fixture.endpoint.clone();
	let model = provider(reqwest::Client::new(), config).unwrap();
	model.infer(request(false), &NoProgress).await
}

#[rstest::rstest]
#[case::text(false)]
#[case::tool_calls(true)]
#[tokio::test]
async fn streamed_progress_precedes_a_response_equal_to_the_non_streamed_one(
	#[case] with_tools: bool,
) {
	// Arrange
	let release = Arc::new(Notify::new());
	let (steps, whole) = if with_tools {
		(
			vec![
				keepalive(),
				delta(json!({"role":"assistant","content":"Reading "})),
				delta(
					json!({"tool_calls":[{"index":0,"id":"call-1","type":"function",
					"function":{"name":"read","arguments":"{\"path\":"}}]}),
				),
				Step::Wait(release.clone()),
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
				Step::Wait(release.clone()),
				delta(json!({"reasoning":"private reasoning"})),
				delta(json!({"content":"leted"})),
				finish("stop"),
				usage(),
				done(),
			],
			json!({"choices":[{"finish_reason":"stop","message":{"content":"Completed"}}],
				"usage":{"prompt_tokens":12,"completion_tokens":7,"cost":0.01}}),
		)
	};
	let mut fixture = serve(steps, whole).await;
	let streamed = model(&fixture.endpoint, true);
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
	let whole = model(&fixture.endpoint, false)
		.infer(request(with_tools), &NoProgress)
		.await
		.unwrap();

	// Assert
	let sent = fixture.received.recv().await.unwrap();
	assert_eq!(sent["stream"], true);
	assert_eq!(sent["stream_options"], json!({"include_usage": true}));
	assert_eq!(sent["provider"]["zdr"], true);
	let non_streamed = fixture.received.recv().await.unwrap();
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
	#[case] steps: Vec<Step>,
	model_config: ModelConfig,
) {
	// Act
	let result = infer_stream(steps, model_config).await;

	// Assert
	assert!(
		matches!(result, Err(aidash_application::Error::External(_))),
		"{result:?}"
	);
}

#[rstest::rstest]
#[case::text(false)]
#[case::tool_calls(true)]
#[tokio::test]
async fn streamed_requests_accept_a_whole_json_completion_without_progress(
	#[case] with_tools: bool,
) {
	// Arrange
	let whole = if with_tools {
		json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"Reading ",
			"tool_calls":[{"id":"call-1","type":"function","function":{"name":"read",
			"arguments":"{\"path\":\"private-notes\"}"}}]}}],
			"usage":{"prompt_tokens":12,"completion_tokens":7}})
	} else {
		json!({"choices":[{"finish_reason":"stop","message":{"content":"Completed"}}],
			"usage":{"prompt_tokens":12,"completion_tokens":7}})
	};
	let mut fixture = serve_whole("application/json; charset=utf-8", whole.to_string()).await;
	let (sender, mut offered) = mpsc::unbounded_channel();

	// Act
	let streamed = model(&fixture.endpoint, true)
		.infer(request(with_tools), &Progress(sender))
		.await
		.unwrap();
	let non_streamed = model(&fixture.endpoint, false)
		.infer(request(with_tools), &NoProgress)
		.await
		.unwrap();

	// Assert
	assert_eq!(fixture.received.recv().await.unwrap()["stream"], true);
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
async fn streamed_requests_reject_other_content_types(#[case] content_type: &'static str) {
	// Arrange
	let body = json!({"choices":[{"finish_reason":"stop","message":{"content":"Completed"}}]});
	let fixture = serve_whole(content_type, body.to_string()).await;

	// Act
	let result = model(&fixture.endpoint, true)
		.infer(request(false), &NoProgress)
		.await;

	// Assert
	assert!(
		matches!(result, Err(aidash_application::Error::External(_))),
		"{result:?}"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn comment_only_keepalives_stall_the_stream(mut model_config: ModelConfig) {
	// Arrange
	model_config.stream_stall_timeout_secs = Some(1);
	let mut steps = vec![delta(json!({"content":"Hi"}))];
	for _ in 0..50 {
		steps.extend([keepalive(), Step::Sleep(Duration::from_millis(100))]);
	}
	steps.extend([finish("stop"), done()]);
	let started = std::time::Instant::now();

	// Act
	let result = infer_stream(steps, model_config).await;

	// Assert
	assert!(
		matches!(result, Err(aidash_application::Error::InferenceStalled)),
		"{result:?}"
	);
	assert!(started.elapsed() < Duration::from_secs(4));
}

#[rstest::rstest]
#[tokio::test]
async fn reasoning_chunks_keep_a_slow_stream_alive(mut model_config: ModelConfig) {
	// Arrange
	model_config.stream_stall_timeout_secs = Some(1);
	let mut steps = vec![];
	for _ in 0..6 {
		steps.extend([
			delta(json!({"reasoning":"thinking"})),
			Step::Sleep(Duration::from_millis(400)),
		]);
	}
	steps.extend([delta(json!({"content":"Done"})), finish("stop"), done()]);

	// Act
	let result = infer_stream(steps, model_config).await;

	// Assert
	assert_eq!(result.unwrap().text, "Done");
}

#[rstest::rstest]
#[tokio::test]
async fn oversized_streams_fail_like_oversized_bodies(model_config: ModelConfig) {
	// Arrange
	let piece = "x".repeat(64 * 1024);
	let mut steps: Vec<Step> = (0..17).map(|_| delta(json!({"content":piece}))).collect();
	steps.extend([finish("stop"), done()]);

	// Act
	let result = infer_stream(steps, model_config).await;

	// Assert
	assert!(matches!(
		result,
		Err(aidash_application::Error::External(message)) if message == "response exceeds 1048576 bytes"
	));
}

#[rstest::rstest]
#[tokio::test]
async fn mid_stream_errors_keep_a_safe_reason(model_config: ModelConfig) {
	// Arrange
	let steps = vec![
		delta(json!({"content":"Hi"})),
		data(
			json!({"error":{"code":503,"message":"token=private upstream detail"},
			"choices":[{"index":0,"delta":{"content":""},"finish_reason":"error"}]}),
		),
	];

	// Act
	let result = infer_stream(steps, model_config).await;

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
fn provider_rejects_a_zero_stall_timeout(mut model_config: ModelConfig) {
	// Arrange
	model_config.stream_stall_timeout_secs = Some(0);

	// Act
	let result = provider(reqwest::Client::new(), model_config);

	// Assert
	assert!(matches!(result, Err(aidash_server::Error::Invalid(_))));
}
