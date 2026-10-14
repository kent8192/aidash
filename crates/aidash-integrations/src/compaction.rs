//! Jev (TypeSafe System One) through OpenRouter's System One API for
//! fast-jev-compaction's probability questions. Every request is a Decision
//! Attempt of a Run that uses the Tenant's current OpenRouter Provider
//! Credential (ADR 0006, ADR 0041); no endpoint or key is configured here.
use crate::{Error, Result};
use aidash_application::{
	ports::CompactionClassifier,
	provider_access::{Context, Inference, Operation, ProviderAccess, Source},
};
use aidash_domain::provider_credentials::Provider;
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use uuid::Uuid;

pub type Questions = Map<String, Value>;
use aidash_application::context::probability;

/// OpenRouter's System One path under the catalog (or Credential Broker) base URL.
const SYSTEM_ONE_PATH: &str = "systemone";

pub struct JevClient {
	access: Arc<dyn ProviderAccess>,
	context: Context,
	client: reqwest::Client,
	model: String,
	max_request_bytes: usize,
	max_questions: usize,
	max_response_bytes: usize,
}

impl JevClient {
	/// `context` names the Run and its Tenant; the Provider Credential is the one
	/// that Tenant's OpenRouter binding names at each call.
	pub fn new(
		client: reqwest::Client,
		model: String,
		access: Arc<dyn ProviderAccess>,
		context: Context,
	) -> Result<Self> {
		aidash_domain::decision::validate_model(&model)?;
		if context.tenant.is_empty()
			|| context.run.is_none_or(|run| run.is_nil())
			|| context.maintenance.is_some()
		{
			return Err(Error::Invalid(
				"Jev calls are provider calls of one Run in its Tenant".into(),
			));
		}
		Ok(Self {
			access,
			context,
			client,
			model,
			max_request_bytes: 1_048_576,
			max_questions: 1024,
			max_response_bytes: 1_048_576,
		})
	}

	fn body(&self, state: &Value, questions: &Questions) -> Value {
		// Zero data retention is required on every OpenRouter call.
		json!({
			"model": self.model, "state": state, "questions": questions,
			"provider": {"zdr": true}
		})
	}

	pub fn check_request(&self, state: &Value, questions: &Questions) -> Result<usize> {
		let bytes = serde_json::to_vec(&self.body(state, questions))
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

	/// The call scope of one Decision Attempt.
	fn scope(&self, attempt: Option<Uuid>) -> Context {
		let mut context = self.context.clone();
		context.provider_credential_id = None;
		context.inference = Some(Inference {
			model: self.model.clone(),
			operations: vec![Operation::Decision],
			max_output_tokens: 0,
			cache_breakpoints: false,
			call: attempt,
		});
		context
	}

	fn source() -> Source {
		Source::Tenant {
			provider: Provider::Openrouter.id().into(),
		}
	}

	/// Fails before any allowance is reserved when the Tenant has no usable
	/// OpenRouter binding; it neither mints a token nor reads Key Material.
	pub async fn check_credential(&self) -> Result<()> {
		self.access
			.check(
				&self.scope(None),
				Provider::Openrouter.base_url(),
				&Self::source(),
			)
			.await
			.map_err(Into::into)
	}

	async fn send(&self, state: &Value, questions: &Questions) -> Result<Value> {
		self.send_checked(state, questions, false).await
	}
}

#[async_trait]
impl CompactionClassifier for JevClient {
	async fn ask(&self, state: &Value, questions: &Questions) -> Result<Value> {
		self.send(state, questions).await
	}
}

#[cfg(test)]
mod tests;

impl JevClient {
	pub async fn check_remote_credential(&self) -> Result<()> {
		self.check_credential()
			.await
			.map_err(|_| Error::RemoteSemantic(aidash_domain::semantic::Failure::Configuration))
	}

	/// Keep remote retries and provider-contract failures on the same durable
	/// semantic pause path as embedding and inference, without exposing bodies.
	pub async fn ask_remote(&self, state: &Value, questions: &Questions) -> Result<Value> {
		self.send_checked(state, questions, true).await
	}
}

impl JevClient {
	async fn send_checked(
		&self,
		state: &Value,
		questions: &Questions,
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
		let attempt = Uuid::now_v7();
		let access = self
			.access
			.resolve(
				&self.scope(Some(attempt)),
				Provider::Openrouter.base_url(),
				&Self::source(),
			)
			.await
			.map_err(|error| failure(Failure::Configuration, error.into()))?;
		let mut request = self
			.client
			.post(format!(
				"{}/{SYSTEM_ONE_PATH}",
				access.endpoint.trim_end_matches('/')
			))
			.json(&self.body(state, questions));
		if !access.bearer.expose_secret().is_empty() {
			request = request.bearer_auth(access.bearer.expose_secret());
		}
		let mut response = request
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
		let served = value["model"].as_str().unwrap_or_default();
		if !aidash_domain::decision::served_by(&self.model, served) {
			return Err(failure(
				Failure::ProviderContract,
				Error::External("Jev response model differs from the pinned version".into()),
			));
		}
		if remote {
			for name in questions.keys() {
				probability(&value, name)
					.map_err(|_| Error::RemoteSemantic(Failure::ProviderContract))?;
			}
		}
		// Until Decision Evidence keeps it (#108), cost is observable only here.
		let input_tokens = value.pointer("/usage/input_tokens").and_then(Value::as_u64);
		let cost_usd = value.pointer("/usage/cost").and_then(Value::as_f64);
		tracing::info!(
			decision_attempt = %attempt,
			served_model = served,
			input_tokens,
			cost_usd,
			"Jev Decision Attempt answered"
		);
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
