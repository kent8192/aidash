use crate::provider_fixtures::{CompletionFixture, delayed_server};
use aidash_server::{
	Error, Result,
	provider::{ModelRequest, ModelResponse, provider},
	registry::{Entry, ModelConfig, validate},
};
use rstest::fixture;
use serde_json::{Value, json};
use std::time::Duration;

#[fixture]
fn model_config(#[default("https://openrouter.ai/api/v1")] endpoint: &str
) -> Value {
	json!({
		"provider":"openrouter", "model_id":"vendor/fixture-model",
		"endpoint":endpoint, "credential_env":null,
		"context_window":32768, "max_output_tokens":4096,
		"modalities":["text"], "cost":{}
	})
}

#[fixture]
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

async fn infer_after(
	mut server: CompletionFixture,
	timeout_secs: Option<u32>,
	delay_secs: u64,
	client: reqwest::Client,
) -> (Result<ModelResponse>, Duration) {
	let mut config = model_config(&server.server.url);
	if let Some(timeout) = timeout_secs {
		config["request_timeout_secs"] = json!(timeout);
	}
	let model = provider(client, serde_json::from_value(config).unwrap()).unwrap();
	let result = server
		.respond_after(
			model.infer(ModelRequest {
				instructions: "test".into(),
				context: json!({}).into(),
				tools: vec![],
				max_output_tokens: 512,
				content_parts: vec![],
				cache_scope: None,
				cache_breakpoints: false,
			}),
			delay_secs,
		)
		.await;
	assert!(
		server.received.try_recv().is_err(),
		"inference must not retry"
	);
	(result.0.map_err(Into::into), result.1)
}

#[rstest::rstest]
#[tokio::test]
async fn provider_timeout_above_120_seconds_allows_a_508_second_response(
	#[future] delayed_server: CompletionFixture,
	shared_client: reqwest::Client,
) {
	let (response, elapsed) =
		infer_after(delayed_server.await, Some(900), 508, shared_client).await;
	let response = response.unwrap();
	assert_eq!(response.text, "Completed");
	assert_eq!((response.input_tokens, response.output_tokens), (12, 7));
	assert_eq!(elapsed.as_secs(), 508);
}

#[rstest::rstest]
#[tokio::test]
async fn provider_timeout_is_not_capped_by_the_900_second_default(
	#[future] delayed_server: CompletionFixture,
	shared_client: reqwest::Client,
) {
	let (response, elapsed) =
		infer_after(delayed_server.await, Some(1200), 1000, shared_client).await;
	assert_eq!(response.unwrap().text, "Completed");
	assert_eq!(elapsed.as_secs(), 1000);
}

#[rstest::rstest]
#[tokio::test]
async fn omitted_provider_timeout_allows_a_508_second_response(
	#[future] delayed_server: CompletionFixture,
	shared_client: reqwest::Client,
) {
	let (response, _) = infer_after(delayed_server.await, None, 508, shared_client).await;
	assert_eq!(response.unwrap().text, "Completed");
}

#[rstest::rstest]
#[tokio::test]
async fn shorter_provider_timeout_is_enforced(
	#[future] delayed_server: CompletionFixture,
	shared_client: reqwest::Client,
) {
	let (response, elapsed) = infer_after(delayed_server.await, Some(30), 60, shared_client).await;
	assert!(matches!(response, Err(Error::External(_))));
	assert!(
		(29..=31).contains(&elapsed.as_secs()),
		"elapsed: {elapsed:?}"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn omitted_provider_timeout_expires_at_900_seconds(
	#[future] delayed_server: CompletionFixture,
	shared_client: reqwest::Client,
) {
	let (response, elapsed) = infer_after(delayed_server.await, None, 1000, shared_client).await;
	assert!(matches!(response, Err(Error::External(_))));
	assert!(
		(899..=901).contains(&elapsed.as_secs()),
		"elapsed: {elapsed:?}"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn non_inference_requests_keep_the_shared_client_timeout(
	#[future] delayed_server: CompletionFixture,
	shared_client: reqwest::Client,
) {
	let mut server = delayed_server.await;
	let request = shared_client
		.post(format!("{}/chat/completions", server.server.url))
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
fn provider_rejects_zero_timeout_before_sending_a_request(shared_client: reqwest::Client) {
	let mut value = model_config("http://127.0.0.1:1");
	value["request_timeout_secs"] = json!(0);
	let config: ModelConfig = serde_json::from_value(value).unwrap();
	assert!(matches!(
		provider(shared_client, config),
		Err(Error::Invalid(_))
	));
}
