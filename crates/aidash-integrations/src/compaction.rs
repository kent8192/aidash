//! TypeSafe System One transport for fast-jev-compaction's probability questions.
use crate::{Error, Result};
use aidash_application::ports::{CompactionClassifier, Credentials};
use async_trait::async_trait;
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub type Questions = Map<String, Value>;
use aidash_application::context::probability;

pub struct JevClient {
	credentials: Arc<dyn Credentials>,
	client: reqwest::Client,
	endpoint: String,
	model: String,
	credential_env: String,
	max_request_bytes: usize,
	max_questions: usize,
	max_response_bytes: usize,
}

impl JevClient {
	pub fn new(
		client: reqwest::Client,
		endpoint: String,
		model: String,
		credential_env: String,
		credentials: Arc<dyn Credentials>,
	) -> Result<Self> {
		validate_endpoint(&endpoint)?;
		if model.trim().is_empty() || model.len() > 128 {
			return Err(Error::Invalid(
				"Jev model must contain 1 to 128 bytes".into(),
			));
		}
		Ok(Self {
			credentials,
			client,
			endpoint,
			model,
			credential_env,
			max_request_bytes: 1_048_576,
			max_questions: 1024,
			max_response_bytes: 1_048_576,
		})
	}

	pub fn check_request(&self, state: &Value, questions: &Questions) -> Result<usize> {
		let bytes =
			serde_json::to_vec(&json!({"model":self.model,"state":state,"questions":questions}))
				.map_err(|error| Error::External(error.to_string()))?
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
		self.send_checked(state, questions, key, false).await
	}
}

#[async_trait]
impl CompactionClassifier for JevClient {
	async fn ask(&self, state: &Value, questions: &Questions) -> Result<Value> {
		// Resolve lazily: short runs never need a Jev key or a network request.
		let key = self.credentials.resolve(&self.credential_env)?;
		if key.trim().is_empty() {
			return Err(Error::Invalid("Jev credential must not be empty".into()));
		}
		self.send(state, questions, &key).await
	}
}

#[cfg(test)]
mod tests;

impl JevClient {
	pub fn check_remote_credential(&self) -> Result<()> {
		let key = self
			.credentials
			.resolve(&self.credential_env)
			.map_err(|_| Error::RemoteSemantic(aidash_domain::semantic::Failure::Configuration))?;
		if key.trim().is_empty() {
			return Err(Error::RemoteSemantic(
				aidash_domain::semantic::Failure::Configuration,
			));
		}
		Ok(())
	}
}

impl JevClient {
	/// Keep remote retries and provider-contract failures on the same durable
	/// semantic pause path as embedding and inference, without exposing bodies.
	pub async fn ask_remote(&self, state: &Value, questions: &Questions) -> Result<Value> {
		use aidash_domain::semantic::Failure;
		let key = self
			.credentials
			.resolve(&self.credential_env)
			.map_err(|_| Error::RemoteSemantic(Failure::Configuration))?;
		if key.trim().is_empty() {
			return Err(Error::RemoteSemantic(Failure::Configuration));
		}
		self.send_checked(state, questions, &key, true).await
	}
}

impl JevClient {
	async fn send_checked(
		&self,
		state: &Value,
		questions: &Questions,
		key: &str,
		remote: bool,
	) -> Result<Value> {
		use aidash_domain::semantic::Failure;
		let failure = |reason, error| {
			if remote {
				Error::RemoteSemantic(reason)
			} else {
				error
			}
		};
		self.check_request(state, questions)?;
		let mut response = self
			.request(state, questions, key)
			.send()
			.await
			.map_err(|error| failure(Failure::Unavailable, crate::http_error(error)))?;
		if !response.status().is_success() {
			// Provider error bodies may include private history or credentials.
			let status = response.status();
			let reason =
				if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
					Failure::Unavailable
				} else if matches!(status.as_u16(), 401 | 403) {
					Failure::Configuration
				} else {
					Failure::ProviderContract
				};
			return Err(failure(
				reason,
				Error::External(format!("Jev provider returned {}", status)),
			));
		}
		let mut bytes = Vec::new();
		while let Some(chunk) = response
			.chunk()
			.await
			.map_err(|error| failure(Failure::Unavailable, crate::http_error(error)))?
		{
			if bytes.len().saturating_add(chunk.len()) > self.max_response_bytes {
				return Err(failure(
					Failure::ProviderContract,
					Error::External("compaction response exceeds approved bounds".into()),
				));
			}
			bytes.extend_from_slice(&chunk);
		}
		let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
			failure(
				Failure::ProviderContract,
				Error::External("invalid compaction response JSON".into()),
			)
		})?;
		if !value["answers"].is_object() {
			return Err(failure(
				Failure::ProviderContract,
				Error::External("Jev response is missing answers".into()),
			));
		}
		if remote {
			for name in questions.keys() {
				probability(&value, name)
					.map_err(|_| Error::RemoteSemantic(Failure::ProviderContract))?;
			}
		}
		Ok(value)
	}
}
/// The approved bounds are supplied by the process composition root.
impl JevClient {
	pub fn with_bounds(mut self, request: usize, questions: usize, response: usize) -> Self {
		self.max_request_bytes = request;
		self.max_questions = questions;
		self.max_response_bytes = response;
		self
	}
}

pub(crate) fn validate_endpoint(endpoint: &str) -> Result<()> {
	let url =
		reqwest::Url::parse(endpoint).map_err(|_| Error::Invalid("invalid endpoint URL".into()))?;
	if !matches!(url.scheme(), "http" | "https")
		|| url.host_str().is_none()
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.query().is_some()
		|| url.fragment().is_some()
	{
		return Err(Error::Invalid(
			"endpoint requires HTTP(S) without inline credentials, query or fragment".into(),
		));
	}
	Ok(())
}
