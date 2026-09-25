use aidash::{
	Error, Result,
	provider::{ModelRequest, ModelResponse, provider},
	registry::{Entry, ModelConfig, validate},
};
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::sync::{Notify, mpsc};

fn model_config(endpoint: &str) -> Value {
	json!({
		"provider":"openrouter", "model_id":"vendor/fixture-model",
		"endpoint":endpoint, "credential_env":null,
		"context_window":32768, "max_output_tokens":4096,
		"modalities":["text"], "cost":{}
	})
}

fn shared_client() -> reqwest::Client {
	// Match the shared client's production deadlines. Inference must override
	// the total timeout, not require an increase for every outbound request.
	reqwest::Client::builder()
		.timeout(Duration::from_secs(120))
		.connect_timeout(Duration::from_secs(10))
		.redirect(reqwest::redirect::Policy::none())
		.no_proxy()
		.build()
		.unwrap()
}

struct DelayedServer {
	endpoint: String,
	received: mpsc::UnboundedReceiver<Value>,
	release: Arc<Notify>,
	task: tokio::task::JoinHandle<()>,
}

impl DelayedServer {
	async fn start() -> Self {
		let (tx, received) = mpsc::unbounded_channel();
		let release = Arc::new(Notify::new());
		let respond = release.clone();
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("http://{}", listener.local_addr().unwrap());
		let app = Router::new().route(
			"/chat/completions",
			post(move |Json(body): Json<Value>| {
				let tx = tx.clone();
				let respond = respond.clone();
				async move {
					tx.send(body).unwrap();
					respond.notified().await;
					Json(json!({
						"choices":[{"finish_reason":"stop","message":{"content":"Completed"}}],
						"usage":{"prompt_tokens":12,"completion_tokens":7}
					}))
				}
			}),
		);
		let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
		Self {
			endpoint,
			received,
			release,
			task,
		}
	}

	async fn respond_after<T>(
		&mut self,
		request: impl Future<Output = T>,
		delay_secs: u64,
	) -> (T, Duration) {
		tokio::pin!(request);
		// Establish the connection with real time first. Pausing before network
		// I/O is ready can auto-advance past the connect timeout in CI.
		tokio::time::timeout(Duration::from_secs(5), async {
			tokio::select! {
				_ = &mut request => panic!("request completed before the server received it"),
				body = self.received.recv() => {
					let body = body.expect("server must receive the request");
					assert!(body.get("request_timeout_secs").is_none());
				}
			}
		})
		.await
		.expect("local request must reach the server");

		tokio::time::pause();
		let started = tokio::time::Instant::now();
		tokio::select! {
			result = &mut request => {
				let elapsed = started.elapsed();
				tokio::time::resume();
				(result, elapsed)
			}
			() = tokio::time::sleep(Duration::from_secs(delay_secs)) => {
				let elapsed = started.elapsed();
				// Resume before releasing network I/O so the virtual clock cannot
				// jump to the request deadline while the response is in transit.
				tokio::time::resume();
				self.release.notify_one();
				let result = tokio::time::timeout(Duration::from_secs(5), &mut request)
					.await
					.expect("released response must complete");
				(result, elapsed)
			}
		}
	}
}

impl Drop for DelayedServer {
	fn drop(&mut self) {
		self.release.notify_one();
		self.task.abort();
	}
}

async fn infer_after(
	timeout_secs: Option<u32>,
	delay_secs: u64,
) -> (Result<ModelResponse>, Duration) {
	let mut server = DelayedServer::start().await;
	let mut config = model_config(&server.endpoint);
	if let Some(timeout) = timeout_secs {
		config["request_timeout_secs"] = json!(timeout);
	}
	let model = provider(shared_client(), serde_json::from_value(config).unwrap()).unwrap();
	let result = server
		.respond_after(
			model.infer(ModelRequest {
				instructions: "test".into(),
				context: json!({}),
				tools: vec![],
				max_output_tokens: 512,
			}),
			delay_secs,
		)
		.await;
	assert!(
		server.received.try_recv().is_err(),
		"inference must not retry"
	);
	result
}

#[rstest::rstest]
#[tokio::test]
async fn provider_timeout_above_120_seconds_allows_a_508_second_response() {
	let (response, elapsed) = infer_after(Some(900), 508).await;
	let response = response.unwrap();
	assert_eq!(response.text, "Completed");
	assert_eq!((response.input_tokens, response.output_tokens), (12, 7));
	assert_eq!(elapsed.as_secs(), 508);
}

#[rstest::rstest]
#[tokio::test]
async fn provider_timeout_is_not_capped_by_the_900_second_default() {
	let (response, elapsed) = infer_after(Some(1200), 1000).await;
	assert_eq!(response.unwrap().text, "Completed");
	assert_eq!(elapsed.as_secs(), 1000);
}

#[rstest::rstest]
#[tokio::test]
async fn omitted_provider_timeout_allows_a_508_second_response() {
	let (response, _) = infer_after(None, 508).await;
	assert_eq!(response.unwrap().text, "Completed");
}

#[rstest::rstest]
#[tokio::test]
async fn shorter_provider_timeout_is_enforced() {
	let (response, elapsed) = infer_after(Some(30), 60).await;
	assert!(matches!(response, Err(Error::External(_))));
	assert!(
		(29..=31).contains(&elapsed.as_secs()),
		"elapsed: {elapsed:?}"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn omitted_provider_timeout_expires_at_900_seconds() {
	let (response, elapsed) = infer_after(None, 1000).await;
	assert!(matches!(response, Err(Error::External(_))));
	assert!(
		(899..=901).contains(&elapsed.as_secs()),
		"elapsed: {elapsed:?}"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn non_inference_requests_keep_the_shared_client_timeout() {
	let mut server = DelayedServer::start().await;
	let request = shared_client()
		.post(format!("{}/chat/completions", server.endpoint))
		.json(&json!({}))
		.send();
	let (response, elapsed) = server.respond_after(request, 508).await;
	assert!(response.unwrap_err().is_timeout());
	assert!(
		(119..=121).contains(&elapsed.as_secs()),
		"elapsed: {elapsed:?}"
	);
}

#[rstest::rstest]
fn registry_validates_provider_timeout_values() {
	let mut entry: Entry = serde_json::from_value(json!({
		"id":"timeout-model", "version":"1.0.0", "kind":"model",
		"name":{"en":"Timeout model"}, "description":{"en":"Test model"},
		"config":model_config("https://openrouter.ai/api/v1")
	}))
	.unwrap();
	validate(&entry).unwrap();
	for timeout in [json!(1), json!(30), json!(900), json!(1200), Value::Null] {
		entry.config["request_timeout_secs"] = timeout;
		validate(&entry).unwrap();
	}
	for timeout in [
		json!(0),
		json!(-1),
		json!(1.5),
		json!("900"),
		json!(u64::MAX),
	] {
		entry.config["request_timeout_secs"] = timeout.clone();
		assert!(validate(&entry).is_err(), "accepted timeout: {timeout}");
	}
}

#[rstest::rstest]
fn legacy_and_null_timeouts_remain_optional_when_serialized() {
	let mut value = model_config("https://openrouter.ai/api/v1");
	for explicit_null in [false, true] {
		if explicit_null {
			value["request_timeout_secs"] = Value::Null;
		}
		let config: ModelConfig = serde_json::from_value(value.clone()).unwrap();
		assert!(
			serde_json::to_value(config)
				.unwrap()
				.get("request_timeout_secs")
				.is_none()
		);
	}
}

#[rstest::rstest]
fn provider_rejects_zero_timeout_before_sending_a_request() {
	let mut value = model_config("http://127.0.0.1:1");
	value["request_timeout_secs"] = json!(0);
	let config: ModelConfig = serde_json::from_value(value).unwrap();
	assert!(matches!(
		provider(shared_client(), config),
		Err(Error::Invalid(_))
	));
}
