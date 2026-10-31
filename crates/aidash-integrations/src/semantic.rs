//! Explicit embedding adapter. PostgreSQL storage belongs to the native repository.
use crate::{Error, Result, keyless::Clients};
use aidash_application::ports::EmbeddingProvider;
use aidash_application::provider_access::{Context, Inference, Operation, ProviderAccess, Source};
use aidash_domain::semantic::{Embedding, EmbeddingConfig};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Own the connection pool with the node runtime. A process-global pool can
/// retain dispatch tasks from a runtime that has already shut down.
pub fn client() -> Result<reqwest::Client> {
	reqwest::Client::builder()
		.timeout(TIMEOUT)
		.connect_timeout(CONNECT_TIMEOUT)
		.redirect(reqwest::redirect::Policy::none())
		.build()
		.map_err(crate::http_error)
}
/// [`client`] paired with a public-only client for Keyless endpoints (ADR 0036).
pub fn clients() -> Result<Clients> {
	Clients::new(client()?, TIMEOUT, CONNECT_TIMEOUT)
}
fn request(
	client: &reqwest::Client,
	method: reqwest::Method,
	endpoint: &str,
	path: &str,
) -> Result<reqwest::RequestBuilder> {
	crate::validate_endpoint(endpoint)?;
	Ok(client.request(method, format!("{}{path}", endpoint.trim_end_matches('/'))))
}

fn embedding_model_missing(status: u16, body: Option<&Value>, raw: Option<&[u8]>) -> bool {
	if !matches!(status, 400 | 404) {
		return false;
	}
	let structured = body.is_some_and(|body| {
		let error = &body["error"];
		let code = error["code"].as_str().unwrap_or_default();
		let message = error["message"]
			.as_str()
			.unwrap_or_default()
			.to_ascii_lowercase();
		code.eq_ignore_ascii_case("model_not_found")
			|| (status == 404
				&& message.contains("model")
				&& (message.contains("not found") || message.contains("does not exist")))
	});
	structured
		|| (status == 404
			&& body.is_none()
			&& raw.is_some_and(|raw| {
				let message = String::from_utf8_lossy(raw).to_ascii_lowercase();
				message.contains("model")
					&& (message.contains("not found") || message.contains("does not exist"))
			}))
}
pub async fn embed(
	access: &dyn ProviderAccess,
	context: &Context,
	clients: &Clients,
	config: &EmbeddingConfig,
	text: &str,
) -> Result<Embedding> {
	embed_input(
		access,
		context,
		clients,
		config,
		aidash_domain::semantic::EmbeddingInput::Text(text),
	)
	.await
}

pub async fn embed_input(
	access: &dyn ProviderAccess,
	context: &Context,
	clients: &Clients,
	config: &EmbeddingConfig,
	input: aidash_domain::semantic::EmbeddingInput<'_>,
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
	input.validate(config)?;
	let request_input = match input {
		aidash_domain::semantic::EmbeddingInput::Text(text) => json!(text),
		aidash_domain::semantic::EmbeddingInput::Image { media_type, bytes } => {
			json!([{"content":[{"type":"image_url","image_url":{"url":format!("data:{media_type};base64,{}", STANDARD.encode(bytes))}}]}])
		}
	};
	let mut body = json!({"model":config.model,"input":request_input,"encoding_format":"float"});
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
		cache_breakpoints: false,
		call: None,
		hosts: vec![],
	});
	let access = access
		.resolve(
			&context,
			&config.endpoint,
			&Source::configured(&config.provider_credential, &None),
		)
		.await?;
	let mut call = request(
		clients.select(&access)?,
		reqwest::Method::POST,
		&access.endpoint,
		"/embeddings",
	)?
	.json(&body);
	if !access.bearer.expose_secret().is_empty() {
		call = call.bearer_auth(access.bearer.expose_secret());
	}

	let response = call.send().await.map_err(crate::keyless::send_error)?;
	if !response.status().is_success() {
		let status = response.status().as_u16();
		let raw = crate::response::within(response, 16_384)
			.await
			.ok()
			.and_then(|value| value.ok());
		let body = raw
			.as_deref()
			.and_then(|raw| serde_json::from_slice::<Value>(raw).ok());
		if config.provider_credential.is_some()
			&& let Some(error) = crate::response::capability_failure(status, body.as_ref())
		{
			return Err(error);
		}
		use aidash_domain::semantic::Failure;
		if embedding_model_missing(status, body.as_ref(), raw.as_deref()) {
			return Err(Error::RemoteSemantic(Failure::EmbeddingModelNotFound));
		}
		return Err(Error::RemoteSemantic(match status {
			408 | 429 | 500..=599 => Failure::Unavailable,
			401..=403 => Failure::Configuration,
			_ => Failure::ProviderContract,
		}));
	}
	// A body whose transfer fails stays an `External` outage, like a failed
	// send; a body over the bound or not the declared JSON breaks the contract.
	let body = crate::response::within(response, 1_048_576)
		.await?
		.map_err(|_| Error::RemoteSemantic(aidash_domain::semantic::Failure::ProviderContract))?;
	let value: Value = serde_json::from_slice(&body)
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
	pub clients: Clients,
	pub access: Arc<dyn ProviderAccess>,
	pub context: Context,
}
#[async_trait]
impl EmbeddingProvider for SemanticClient {
	async fn embed(
		&self,
		config: &EmbeddingConfig,
		input: aidash_domain::semantic::EmbeddingInput<'_>,
	) -> Result<Embedding> {
		embed_input(
			self.access.as_ref(),
			&self.context,
			&self.clients,
			config,
			input,
		)
		.await
	}
}

#[cfg(test)]
mod tests;
