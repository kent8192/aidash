//! Sign in with ChatGPT's OAuth endpoints and plan model list (#159). Token
//! responses carry Plan Tokens, so bodies are never logged or copied into
//! errors; only fixed reasons leave this module.
use aidash_application::{
	Error, Result,
	plan_connections::{Authorization, ExchangeRequest, Issued, RefreshFailure},
};
use aidash_domain::plan_connections::{
	self as domain, DEFAULT_CONTEXT_WINDOW, DEFAULT_MAX_OUTPUT_TOKENS, PlanModel,
};
use async_trait::async_trait;
use jsonwebtoken::{Algorithm, Validation, decode, decode_header};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};
use tokio::sync::OnceCell;

/// OpenAI's documented issuer; discovery must report exactly this.
pub const ISSUER: &str = "https://auth.openai.com";
const IO_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_MODELS_BYTES: usize = 2 * 1024 * 1024;
const MAX_MODELS: usize = 512;

#[derive(Deserialize)]
struct Discovery {
	issuer: String,
	token_endpoint: String,
	jwks_uri: String,
	revocation_endpoint: String,
}

/// OpenAI's endpoints, from its discovery document. Test builds may point
/// every endpoint at a fake provider.
pub struct ChatgptAuthorization {
	client: reqwest::Client,
	discovery_url: String,
	api_base: String,
	discovery: OnceCell<Arc<Discovery>>,
	keys: OnceCell<Arc<crate::gcip::HttpSigningKeys>>,
}
impl ChatgptAuthorization {
	pub fn new(client: reqwest::Client) -> Self {
		Self::with_urls(
			client,
			format!("{ISSUER}/.well-known/openid-configuration"),
			aidash_domain::provider_credentials::Provider::Chatgpt
				.base_url()
				.to_owned(),
		)
	}
	/// Points discovery and the API at a fake provider. Compiled only into
	/// tests and `test-support` builds, which no production dependency enables.
	#[cfg(any(test, feature = "test-support"))]
	pub fn at(client: reqwest::Client, discovery_url: String, api_base: String) -> Self {
		Self::with_urls(client, discovery_url, api_base)
	}
	fn with_urls(client: reqwest::Client, discovery_url: String, api_base: String) -> Self {
		Self {
			client,
			discovery_url,
			api_base,
			discovery: OnceCell::new(),
			keys: OnceCell::new(),
		}
	}
	async fn discovery(&self) -> Result<Arc<Discovery>> {
		self.discovery
			.get_or_try_init(|| async {
				let response = self
					.client
					.get(&self.discovery_url)
					.timeout(IO_TIMEOUT)
					.send()
					.await
					.map_err(|_| unavailable())?;
				if !response.status().is_success() {
					return Err(unavailable());
				}
				let discovery: Discovery = crate::response::json(response, 64 * 1024)
					.await
					.map_err(|_| unavailable())?;
				Ok(Arc::new(discovery))
			})
			.await
			.cloned()
	}
	async fn keys(&self) -> Result<Arc<crate::gcip::HttpSigningKeys>> {
		let discovery = self.discovery().await?;
		self.keys
			.get_or_try_init(|| async {
				Ok(Arc::new(crate::gcip::HttpSigningKeys::new(
					self.client.clone(),
					discovery.jwks_uri.clone(),
				)))
			})
			.await
			.cloned()
	}
	async fn token_request(&self, form: &[(&str, &str)]) -> Result<TokenReply> {
		let discovery = self.discovery().await?;
		let response = self
			.client
			.post(&discovery.token_endpoint)
			.timeout(IO_TIMEOUT)
			.form(form)
			.send()
			.await
			.map_err(|_| unavailable())?;
		let status = response.status().as_u16();
		let body = crate::response::bytes(response, MAX_TOKEN_RESPONSE_BYTES)
			.await
			.map_err(|_| unavailable())?;
		if (200..300).contains(&status) {
			let tokens: Tokens = serde_json::from_slice(&body).map_err(|_| unavailable())?;
			return Ok(TokenReply::Issued(tokens.into_issued()?));
		}
		let code = serde_json::from_slice::<OAuthError>(&body)
			.ok()
			.and_then(|error| error.error);
		Ok(TokenReply::Rejected { status, code })
	}
}

enum TokenReply {
	Issued(Issued),
	Rejected { status: u16, code: Option<String> },
}
#[derive(Deserialize)]
struct OAuthError {
	error: Option<String>,
}
#[derive(Deserialize)]
struct Tokens {
	access_token: String,
	refresh_token: String,
	#[serde(default)]
	id_token: Option<String>,
	#[serde(default)]
	token_type: Option<String>,
	expires_in: i64,
	#[serde(default)]
	scope: Option<String>,
}
impl Tokens {
	fn into_issued(self) -> Result<Issued> {
		if self
			.token_type
			.as_deref()
			.is_some_and(|kind| !kind.eq_ignore_ascii_case("bearer"))
		{
			return Err(unavailable());
		}
		Ok(Issued {
			access_token: self.access_token.into(),
			refresh_token: self.refresh_token.into(),
			id_token: self.id_token.map(SecretString::from),
			expires_in: self.expires_in,
			scope: self.scope,
		})
	}
}

fn unavailable() -> Error {
	Error::External("Sign in with ChatGPT is temporarily unavailable".into())
}
fn rejected() -> Error {
	Error::Invalid("Sign in with ChatGPT rejected the authorization".into())
}

#[derive(Deserialize)]
struct IdClaims {
	nonce: Option<String>,
	sub: String,
}

#[async_trait]
impl Authorization for ChatgptAuthorization {
	async fn exchange(&self, request: ExchangeRequest<'_>) -> Result<Issued> {
		if request.code.is_empty() || request.code.len() > 4096 {
			return Err(rejected());
		}
		let reply = self
			.token_request(&[
				("grant_type", "authorization_code"),
				("client_id", request.client_id),
				("code", request.code),
				("code_verifier", request.verifier.expose_secret()),
				("redirect_uri", request.redirect_uri),
				("resource", domain::RESOURCE),
			])
			.await?;
		match reply {
			TokenReply::Issued(issued) => Ok(issued),
			// An invalid, expired or replayed code, or a verifier mismatch.
			TokenReply::Rejected {
				status: 400 | 401 | 403,
				..
			} => Err(rejected()),
			TokenReply::Rejected { .. } => Err(unavailable()),
		}
	}

	async fn verify_id_token(
		&self,
		id_token: &SecretString,
		client_id: &str,
		nonce: &str,
	) -> Result<String> {
		let token = id_token.expose_secret();
		if token.len() > 16_384 {
			return Err(Error::Unauthorized);
		}
		let discovery = self.discovery().await?;
		if discovery.issuer != ISSUER {
			return Err(Error::Unauthorized);
		}
		let header = decode_header(token).map_err(|_| Error::Unauthorized)?;
		if header.alg != Algorithm::RS256 {
			return Err(Error::Unauthorized);
		}
		let kid = header
			.kid
			.filter(|kid| !kid.is_empty() && kid.len() <= 256)
			.ok_or(Error::Unauthorized)?;
		use crate::gcip::SigningKeys as _;
		let key = self
			.keys()
			.await?
			.key(&kid)
			.await
			.map_err(|error| match error {
				// The signing keys could not be read: retryable, never a verdict
				// on the authorization itself.
				Error::External(_) => unavailable(),
				_ => Error::Unauthorized,
			})?;
		let mut validation = Validation::new(Algorithm::RS256);
		validation.set_issuer(&[ISSUER]);
		validation.set_audience(&[client_id]);
		validation.set_required_spec_claims(&["iss", "aud", "exp", "sub"]);
		validation.leeway = 30;
		let claims = decode::<IdClaims>(token, &key, &validation)
			.map_err(|_| Error::Unauthorized)?
			.claims;
		if claims.sub.is_empty() || claims.nonce.as_deref() != Some(nonce) {
			return Err(Error::Unauthorized);
		}
		Ok(claims.sub)
	}

	async fn refresh(
		&self,
		client_id: &str,
		refresh_token: &SecretString,
	) -> std::result::Result<Issued, RefreshFailure> {
		// Omitting `scope` keeps the grant.
		let reply = self
			.token_request(&[
				("grant_type", "refresh_token"),
				("client_id", client_id),
				("refresh_token", refresh_token.expose_secret()),
				("resource", domain::RESOURCE),
			])
			.await
			.map_err(RefreshFailure::Unavailable)?;
		match reply {
			TokenReply::Issued(issued) => Ok(issued),
			TokenReply::Rejected { code, .. } if domain::refresh_is_terminal(code.as_deref()) => {
				Err(RefreshFailure::Terminal)
			}
			TokenReply::Rejected { code, .. } if code.as_deref() == Some("invalid_client") => {
				tracing::error!("Sign in with ChatGPT rejected a Plan Connection's client");
				Err(RefreshFailure::Unavailable(Error::Plan(
					domain::Failure::Misconfigured,
				)))
			}
			TokenReply::Rejected { .. } => Err(RefreshFailure::Unavailable(unavailable())),
		}
	}

	async fn revoke(&self, client_id: &str, refresh_token: &SecretString) -> Result<()> {
		let discovery = self.discovery().await?;
		// Retry transient failures while the refresh token is still at hand.
		for attempt in 0..3 {
			if attempt > 0 {
				tokio::time::sleep(Duration::from_millis(250 << attempt)).await;
			}
			let response = self
				.client
				.post(&discovery.revocation_endpoint)
				.timeout(IO_TIMEOUT)
				.form(&[
					("token", refresh_token.expose_secret()),
					("token_type_hint", "refresh_token"),
					("client_id", client_id),
				])
				.send()
				.await;
			match response {
				// An empty 200 also confirms an already-invalid token.
				Ok(response) if response.status().is_success() => return Ok(()),
				Ok(response) if response.status().is_client_error() => {
					return Err(Error::Invalid(
						"Sign in with ChatGPT refused the revocation".into(),
					));
				}
				_ => continue,
			}
		}
		Err(unavailable())
	}

	async fn models(&self, access_token: &SecretString) -> Result<Vec<PlanModel>> {
		let response = self
			.client
			.get(format!("{}/models", self.api_base.trim_end_matches('/')))
			.timeout(IO_TIMEOUT)
			.bearer_auth(access_token.expose_secret())
			.send()
			.await
			.map_err(crate::http_error)?;
		let status = response.status().as_u16();
		if !(200..300).contains(&status) {
			let body = crate::response::json::<serde_json::Value>(response, 16_384)
				.await
				.ok();
			return Err(crate::inference::responses::plan_rejection(status, body.as_ref()).error);
		}
		let list: ModelList = crate::response::json(response, MAX_MODELS_BYTES).await?;
		Ok(listed(list))
	}
}

#[derive(Deserialize)]
struct ModelList {
	models: Vec<ListedModel>,
}
#[derive(Deserialize)]
struct ListedModel {
	slug: String,
	display_name: Option<String>,
	visibility: Option<String>,
	#[serde(default)]
	context_window: Option<u64>,
	#[serde(default)]
	max_output_tokens: Option<u64>,
}

/// Models offered to pickers: `visibility == "list"` entries in the order
/// OpenAI returned them. Limits OpenAI does not report are filled with
/// conservative defaults and flagged for the administrator.
fn listed(list: ModelList) -> Vec<PlanModel> {
	list.models
		.into_iter()
		.filter(|model| model.visibility.as_deref() == Some("list"))
		.filter(|model| aidash_domain::provider_credentials::validate_model_id(&model.slug).is_ok())
		.take(MAX_MODELS)
		.map(|model| {
			let context_window = model
				.context_window
				.and_then(|tokens| usize::try_from(tokens).ok())
				.filter(|tokens| *tokens >= 2048);
			let max_output_tokens = model
				.max_output_tokens
				.and_then(|tokens| u32::try_from(tokens).ok())
				.filter(|tokens| *tokens > 0);
			let defaulted = context_window.is_none() || max_output_tokens.is_none();
			let context_window = context_window.unwrap_or(DEFAULT_CONTEXT_WINDOW);
			let max_output_tokens = max_output_tokens
				.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
				.min(u32::try_from(context_window).unwrap_or(u32::MAX));
			PlanModel {
				display_name: model
					.display_name
					.filter(|name| !name.trim().is_empty() && name.len() <= 256)
					.unwrap_or_else(|| model.slug.clone()),
				slug: model.slug,
				context_window,
				max_output_tokens,
				defaulted,
			}
		})
		.collect()
}

#[cfg(test)]
mod tests;
