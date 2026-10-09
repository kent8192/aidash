//! Explicit embedding adapter. PostgreSQL storage belongs to the native repository.
use crate::{Error, Result};
use aidash_application::ports::EmbeddingProvider;
use aidash_application::provider_access::{Context, Inference, Operation, ProviderAccess, Source};
use aidash_domain::semantic::{Embedding, EmbeddingConfig};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// Own the connection pool with the node runtime. A process-global pool can
/// retain dispatch tasks from a runtime that has already shut down.
pub fn client() -> Result<reqwest::Client> {
	reqwest::Client::builder()
		.timeout(Duration::from_secs(20))
		.connect_timeout(Duration::from_secs(3))
		.redirect(reqwest::redirect::Policy::none())
		.build()
		.map_err(crate::http_error)
}
fn request(
	client: &reqwest::Client,
	method: reqwest::Method,
	endpoint: &str,
	path: &str,
) -> Result<reqwest::RequestBuilder> {
	crate::compaction::validate_endpoint(endpoint)?;
	Ok(client.request(method, format!("{}{path}", endpoint.trim_end_matches('/'))))
}
pub async fn embed(
	access: &dyn ProviderAccess,
	context: &Context,
	client: &reqwest::Client,
	config: &EmbeddingConfig,
	text: &str,
) -> Result<Embedding> {
	#[derive(Deserialize)]
	struct EmbeddingDatum {
		index: usize,
		embedding: Vec<f32>,
	}
	#[derive(Deserialize)]
	struct Embeddings {
		model: String,
		data: Vec<EmbeddingDatum>,
	}
	config.validate_parameters()?;
	let mut body = json!({"model":config.model,"input":text,"encoding_format":"float"});
	if config.provider == "openrouter" {
		// Gemini defaults to 3072 dimensions unless the approved size is sent.
		body["dimensions"] = json!(config.dimensions);
		body["provider"] = json!({"zdr":true});
	}
	let mut context = context.clone();
	context.inference = Some(Inference {
		model: config.model.clone(),
		operations: vec![Operation::Embeddings],
		max_output_tokens: 1,
	});
	let access = access
		.resolve(
			&context,
			&config.endpoint,
			&Source::configured(&config.credential_env, &config.provider_credential),
		)
		.await?;
	let mut call = request(
		client,
		reqwest::Method::POST,
		&access.endpoint,
		"/embeddings",
	)?
	.json(&body);
	if !access.bearer.expose_secret().is_empty() {
		call = call.bearer_auth(access.bearer.expose_secret());
	}

	let response = call.send().await.map_err(crate::http_error)?;
	if !response.status().is_success() {
		let status = response.status().as_u16();
		if config.provider_credential.is_some() {
			let body = crate::response::json::<Value>(response, 16_384).await.ok();
			if let Some(error) = crate::response::capability_failure(status, body.as_ref()) {
				return Err(error);
			}
		}
		use aidash_domain::semantic::Failure;
		return Err(Error::RemoteSemantic(match status {
			408 | 429 | 500..=599 => Failure::Unavailable,
			401..=403 => Failure::Configuration,
			_ => Failure::ProviderContract,
		}));
	}
	let value: Value = crate::response::json(response, 1_048_576)
		.await
		.map_err(|_| Error::RemoteSemantic(aidash_domain::semantic::Failure::ProviderContract))?;
	let prompt = value["usage"]["prompt_tokens"].as_u64();
	let total = value["usage"]["total_tokens"].as_u64();
	let tokens = prompt.filter(|count| *count > 0 && Some(*count) == total);
	let output: Embeddings = serde_json::from_value(value)
		.map_err(|_| Error::RemoteSemantic(aidash_domain::semantic::Failure::ProviderContract))?;
	if output.model != config.model || output.data.len() != 1 || output.data[0].index != 0 {
		return Err(Error::RemoteSemantic(
			aidash_domain::semantic::Failure::ProviderContract,
		));
	}
	let vector = output
		.data
		.into_iter()
		.next()
		.expect("validated length")
		.embedding;
	let magnitude: f64 = vector.iter().map(|x| f64::from(*x).powi(2)).sum();
	if vector.len() != config.dimensions
		|| vector.iter().any(|x| !x.is_finite())
		|| !magnitude.is_finite()
		|| magnitude <= 0.0
	{
		return Err(Error::RemoteSemantic(
			aidash_domain::semantic::Failure::ProviderContract,
		));
	}
	Ok(Embedding { vector, tokens })
}
#[derive(Clone)]
pub struct SemanticClient {
	pub client: reqwest::Client,
	pub access: Arc<dyn ProviderAccess>,
	pub context: Context,
}
#[async_trait]
impl EmbeddingProvider for SemanticClient {
	async fn embed(&self, config: &EmbeddingConfig, text: &str) -> Result<Embedding> {
		embed(
			self.access.as_ref(),
			&self.context,
			&self.client,
			config,
			text,
		)
		.await
	}
}

#[cfg(test)]
mod tests;
