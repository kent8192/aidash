//! TypeSafe System One transport for fast-jev-compaction's probability questions.
use crate::{Error, Result, config};
use async_trait::async_trait;
use serde_json::{Map, Value, json};

pub type Questions = Map<String, Value>;

#[async_trait]
pub trait JevAsker: Send + Sync {
	async fn ask(&self, state: &Value, questions: &Questions) -> Result<Value>;
}

pub struct JevClient {
	client: reqwest::Client,
	endpoint: String,
	model: String,
	credential_env: String,
	max_request_bytes: usize,
	max_questions: usize,
	max_response_bytes: usize,
}

impl JevClient {
	pub fn from_env(client: reqwest::Client) -> Result<Self> {
		Self::new(
			client,
			std::env::var("AIDASH_JEV_ENDPOINT")
				.unwrap_or_else(|_| "https://api.typesafe.ai/v1/systemone".into()),
			std::env::var("AIDASH_JEV_MODEL").unwrap_or_else(|_| "jev-latest".into()),
			"AIDASH_SECRET_JEV".into(),
		)
	}

	pub fn new(
		client: reqwest::Client,
		endpoint: String,
		model: String,
		credential_env: String,
	) -> Result<Self> {
		config::validate_endpoint(&endpoint)?;
		if model.trim().is_empty() || model.len() > 128 {
			return Err(Error::Invalid(
				"Jev model must contain 1 to 128 bytes".into(),
			));
		}
		Ok(Self {
			client,
			endpoint,
			model,
			credential_env,
			max_request_bytes: 1_048_576,
			max_questions: 1024,
			max_response_bytes: 1_048_576,
		})
	}

	pub fn approved(config: crate::registry::CompactorConfig) -> Result<Self> {
		config.validate()?;
		let client = reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.connect_timeout(std::time::Duration::from_secs(5))
			.timeout(std::time::Duration::from_secs(30))
			.build()?;
		let mut transport =
			Self::new(client, config.endpoint, config.model, config.credential_env)?;
		transport.max_request_bytes = config.max_request_bytes;
		transport.max_questions = config.max_questions;
		transport.max_response_bytes = config.max_response_bytes;
		Ok(transport)
	}
	pub(crate) fn check_request(&self, state: &Value, questions: &Questions) -> Result<usize> {
		let bytes =
			serde_json::to_vec(&json!({"model":self.model,"state":state,"questions":questions}))?
				.len();
		if bytes > self.max_request_bytes
			|| questions.is_empty()
			|| questions.len() > self.max_questions
		{
			return Err(Error::Invalid(
				"compaction request exceeds approved bounds".into(),
			));
		}
		Ok(bytes)
	}

	fn request(&self, state: &Value, questions: &Questions, key: &str) -> reqwest::RequestBuilder {
		self.client
			.post(&self.endpoint)
			.bearer_auth(key)
			.json(&json!({
				"model": self.model, "state": state, "questions": questions
			}))
	}
	async fn send(&self, state: &Value, questions: &Questions, key: &str) -> Result<Value> {
		self.check_request(state, questions)?;
		let mut response = self.request(state, questions, key).send().await?;
		if !response.status().is_success() {
			// Provider error bodies may include private history or credentials.
			return Err(Error::External(format!(
				"Jev provider returned {}",
				response.status()
			)));
		}
		let mut bytes = Vec::new();
		while let Some(chunk) = response.chunk().await? {
			if bytes.len().saturating_add(chunk.len()) > self.max_response_bytes {
				return Err(Error::External(
					"compaction response exceeds approved bounds".into(),
				));
			}
			bytes.extend_from_slice(&chunk);
		}
		let value: Value = serde_json::from_slice(&bytes)
			.map_err(|_| Error::External("invalid compaction response JSON".into()))?;
		if !value["answers"].is_object() {
			return Err(Error::External("Jev response is missing answers".into()));
		}
		Ok(value)
	}
}

#[async_trait]
impl JevAsker for JevClient {
	async fn ask(&self, state: &Value, questions: &Questions) -> Result<Value> {
		// Resolve lazily: short runs never need a Jev key or a network request.
		let key = config::secret(&self.credential_env)?;
		if key.trim().is_empty() {
			return Err(Error::Invalid("Jev credential must not be empty".into()));
		}
		self.send(state, questions, &key).await
	}
}

pub(super) fn probability(response: &Value, name: &str) -> Result<f64> {
	let answer = &response["answers"][name];
	let value = answer["noul"]
		.as_f64()
		.filter(|v| v.is_finite() && (0.0..=1.0).contains(v));
	if answer.get("type").is_some_and(|kind| kind != "noul") || value.is_none() {
		return Err(Error::External(format!("invalid Jev answer for {name}")));
	}
	Ok(value.unwrap())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn system_one_request_has_bearer_auth_and_probability_questions() {
		let client = JevClient::new(
			reqwest::Client::new(),
			"https://api.typesafe.ai/v1/systemone".into(),
			"jev-latest".into(),
			"AIDASH_SECRET_JEV".into(),
		)
		.unwrap();
		let state = json!({"goal":"finish the task"});
		let questions = json!({"call_t1":{"type":"noul","instructions":"Keep this call?"}})
			.as_object()
			.unwrap()
			.clone();
		let request = client
			.request(&state, &questions, "fixture-key")
			.build()
			.unwrap();
		assert_eq!(
			request.url().as_str(),
			"https://api.typesafe.ai/v1/systemone"
		);
		assert_eq!(request.method(), reqwest::Method::POST);
		assert_eq!(request.headers()["authorization"], "Bearer fixture-key");
		let body: Value =
			serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
		assert_eq!(
			body,
			json!({"model":"jev-latest","state":state,"questions":questions})
		);
	}

	#[test]
	fn malformed_missing_and_out_of_range_probabilities_are_rejected() {
		for answer in [
			json!(null),
			json!({}),
			json!({"noul":"0.5"}),
			json!({"noul":-0.1}),
			json!({"noul":1.1}),
			json!({"type":"choice","noul":0.5}),
		] {
			assert!(probability(&json!({"answers":{"q":answer}}), "q").is_err());
		}
		assert!(probability(&json!({"answers":{}}), "q").is_err());
		for value in [0.0, 0.5, 1.0] {
			assert_eq!(
				probability(&json!({"answers":{"q":{"noul":value}}}), "q").unwrap(),
				value
			);
		}
	}

	#[tokio::test]
	async fn system_one_http_contract_and_failure_redaction() {
		use axum::{
			Json, Router,
			http::{HeaderMap, StatusCode},
			response::IntoResponse,
			routing::post,
		};
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("http://{}/systemone", listener.local_addr().unwrap());
		let app = Router::new().route(
			"/systemone",
			post(|headers: HeaderMap, Json(body): Json<Value>| async move {
				assert_eq!(headers["authorization"], "Bearer fixture-key");
				assert_eq!(body["model"], "jev-test");
				assert_eq!(body["questions"]["q"]["type"], "noul");
				match body["state"].as_str().unwrap() {
					"http-error" => {
						(StatusCode::TOO_MANY_REQUESTS, "private provider detail").into_response()
					}
					"malformed" => (StatusCode::OK, "not json").into_response(),
					"missing" => Json(json!({"other":{}})).into_response(),
					_ => Json(json!({"answers":{"q":{"noul":0.75}}})).into_response(),
				}
			}),
		);
		let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
		let client = JevClient::new(
			reqwest::Client::builder()
				.timeout(std::time::Duration::from_secs(5))
				.build()
				.unwrap(),
			endpoint,
			"jev-test".into(),
			"AIDASH_SECRET_JEV".into(),
		)
		.unwrap();
		let questions = json!({"q":{"type":"noul","instructions":"keep?"}})
			.as_object()
			.unwrap()
			.clone();
		let response = client
			.send(&json!("ok"), &questions, "fixture-key")
			.await
			.unwrap();
		assert_eq!(probability(&response, "q").unwrap(), 0.75);
		for state in ["http-error", "malformed", "missing"] {
			let error = client
				.send(&json!(state), &questions, "fixture-key")
				.await
				.unwrap_err();
			assert!(!error.to_string().contains("private provider detail"));
			assert!(!error.to_string().contains("fixture-key"));
		}
		server.abort();
	}
	#[tokio::test]
	async fn bounded_transport_rejects_oversize_requests_responses_and_redirects() {
		use axum::{Router, response::IntoResponse, routing::post};
		use std::sync::{
			Arc,
			atomic::{AtomicUsize, Ordering},
		};
		let requests = Arc::new(AtomicUsize::new(0));
		let seen = requests.clone();
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("http://{}", listener.local_addr().unwrap());
		let app = Router::new()
			.route(
				"/oversize",
				post(move || {
					let seen = seen.clone();
					async move {
						seen.fetch_add(1, Ordering::SeqCst);
						"x".repeat(129)
					}
				}),
			)
			.route(
				"/redirect",
				post(|| async {
					(
						axum::http::StatusCode::TEMPORARY_REDIRECT,
						[("location", "/oversize")],
					)
						.into_response()
				}),
			);
		let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
		let client = reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.build()
			.unwrap();
		let mut transport = JevClient::new(
			client,
			format!("{endpoint}/oversize"),
			"fixture".into(),
			"AIDASH_SECRET_TEST".into(),
		)
		.unwrap();
		transport.max_request_bytes = 1024;
		transport.max_response_bytes = 128;
		transport.max_questions = 1;
		let questions = json!({"q":{"type":"noul"}}).as_object().unwrap().clone();
		assert!(
			transport
				.send(&json!("x".repeat(1024)), &questions, "fixture")
				.await
				.unwrap_err()
				.to_string()
				.contains("request exceeds")
		);
		let mut excess = questions.clone();
		excess.insert("another".into(), json!({"type":"noul"}));
		assert!(
			transport
				.send(&json!({}), &excess, "fixture")
				.await
				.is_err()
		);
		assert_eq!(requests.load(Ordering::SeqCst), 0);
		assert!(
			transport
				.send(&json!({}), &questions, "fixture")
				.await
				.unwrap_err()
				.to_string()
				.contains("response exceeds")
		);
		assert_eq!(requests.load(Ordering::SeqCst), 1);
		transport.endpoint = format!("{endpoint}/redirect");
		assert!(
			transport
				.send(&json!({}), &questions, "fixture")
				.await
				.unwrap_err()
				.to_string()
				.contains("307")
		);
		assert_eq!(
			requests.load(Ordering::SeqCst),
			1,
			"unapproved redirect target must not receive history"
		);
		server.abort();
	}
}
