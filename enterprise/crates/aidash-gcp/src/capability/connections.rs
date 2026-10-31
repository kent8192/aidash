//! OAuth broker adapters: a write-only server client, and the broker's narrowly
//! scoped Secret Manager reader/version writer. Neither can reach Key Material.
use super::secret_issuer::SecretIssuerConfiguration;
use super::{
	AccessTokenSource, KmsTokenSigner, MetadataTokenSource, SecretManagerTenantSecretSource,
	bounded_json,
};
use aidash_application::{
	Error, Result,
	connection_calls::{ConnectionCalls, ExchangeGrant, Receipt},
	plan_connections::RefreshFailure,
	tenant_secrets::{SecretCall, SecretRoute},
};
use aidash_capability::{
	KeyMaterialError, SignError, TOKEN_TTL_SECS, TokenSigner,
	connection::{ConnectionClaims, ConnectionOperation, ConnectionStore, mint_connection, sealed},
	tenant_secret::{SealedSecretSource, SecretSubject},
};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::{
	sync::Arc,
	time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
use zeroize::Zeroizing;

pub struct SecretManagerConnectionStore {
	source: SecretManagerTenantSecretSource,
}
impl SecretManagerConnectionStore {
	/// Derive the OAuth prefix from the same validated environment as -tsec-.
	pub fn new(
		project: &str,
		tenant_prefix: &str,
		tokens: Arc<dyn AccessTokenSource>,
	) -> std::result::Result<Self, SignError> {
		let mut source = SecretManagerTenantSecretSource::new(project, tenant_prefix, tokens)?;
		source.resource_prefix = format!(
			"{}oauth-",
			source
				.resource_prefix
				.strip_suffix("tsec-")
				.ok_or(SignError)?
		);
		Ok(Self { source })
	}
}
#[async_trait]
impl ConnectionStore for SecretManagerConnectionStore {
	async fn access(
		&self,
		resource: &str,
		version: &str,
	) -> std::result::Result<Zeroizing<Vec<u8>>, KeyMaterialError> {
		self.source.access(resource, version).await
	}
	async fn latest(
		&self,
		resource: &str,
	) -> std::result::Result<(String, Zeroizing<Vec<u8>>), KeyMaterialError> {
		self.source.access_version(resource, "latest", true).await
	}
	async fn add_version(
		&self,
		resource: &str,
		payload: &[u8],
	) -> std::result::Result<String, KeyMaterialError> {
		let id = resource
			.strip_prefix(&self.source.resource_prefix)
			.and_then(|v| Uuid::parse_str(v).ok());
		if id.is_none_or(|v| v.get_version_num() != 7)
			|| !matches!(
				sealed::open(payload),
				Ok(sealed::Payload::Connection { .. })
			) {
			return Err(KeyMaterialError::Unavailable);
		}
		let token = self
			.source
			.tokens
			.token()
			.await
			.map_err(|_| KeyMaterialError::StoreUnavailable)?;
		let response = self
			.source
			.client
			.post(format!("{}/{resource}:addVersion", self.source.base))
			.bearer_auth(token.expose_secret())
			.json(&serde_json::json!({"payload":{"data":STANDARD.encode(payload)}}))
			.send()
			.await
			.map_err(|_| KeyMaterialError::StoreUnavailable)?;
		#[derive(Deserialize)]
		struct Added {
			name: String,
		}
		let added: Added = bounded_json(response, 16384)
			.await
			.map_err(|_| KeyMaterialError::StoreUnavailable)?;
		let version = added
			.name
			.strip_prefix(&format!("{resource}/versions/"))
			.filter(|v| super::numeric_version(v))
			.ok_or(KeyMaterialError::StoreUnavailable)?;
		Ok(version.into())
	}
}

pub struct ConnectionCallsAdapter {
	base: reqwest::Url,
	configuration: SecretIssuerConfiguration,
	signer: Arc<dyn TokenSigner>,
	client: reqwest::Client,
}
impl ConnectionCallsAdapter {
	pub fn new(endpoint: &str, configuration: SecretIssuerConfiguration) -> Result<Self> {
		let signer = Arc::new(
			KmsTokenSigner::new(
				configuration.kid.clone(),
				Arc::new(MetadataTokenSource::new().map_err(|_| unavailable())?),
			)
			.map_err(|_| unavailable())?,
		);
		Self::with_signer(endpoint, configuration, signer)
	}
	pub fn with_signer(
		endpoint: &str,
		configuration: SecretIssuerConfiguration,
		signer: Arc<dyn TokenSigner>,
	) -> Result<Self> {
		configuration.validate()?;
		if configuration.kid != signer.kid() {
			return Err(Error::Invalid("OAuth broker signing key mismatch".into()));
		}
		let mut base = super::secret_calls::forward_base(endpoint)?;
		base.set_path("/");
		let client = reqwest::Client::builder()
			.no_proxy()
			.redirect(reqwest::redirect::Policy::none())
			.timeout(Duration::from_secs(30))
			.build()
			.map_err(|_| unavailable())?;
		Ok(Self {
			base,
			configuration,
			signer,
			client,
		})
	}
	async fn token(
		&self,
		tenant: &str,
		connection: Uuid,
		operation: ConnectionOperation,
	) -> Result<SecretString> {
		let now = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.map_err(|_| unavailable())?
			.as_secs();
		let c = ConnectionClaims {
			iss: self.configuration.issuer.clone(),
			aud: self.configuration.audience.clone(),
			iat: now,
			exp: now + TOKEN_TTL_SECS,
			jti: Uuid::new_v4(),
			kid: self.configuration.kid.clone(),
			tenant: tenant.into(),
			connection,
			operation,
		};
		c.validate()
			.map_err(|_| Error::Invalid("invalid connection claims".into()))?;
		mint_connection(&c, self.signer.as_ref())
			.await
			.map_err(|_| unavailable())
	}
	async fn post(
		&self,
		path: &str,
		tenant: &str,
		connection: Uuid,
		operation: ConnectionOperation,
		body: Option<serde_json::Value>,
	) -> Result<Option<Receipt>> {
		let token = self.token(tenant, connection, operation).await?;
		let mut request = self
			.client
			.post(self.base.join(path).map_err(|_| unavailable())?)
			.header("x-aidash-secret-call", token.expose_secret());
		if let Some(body) = body {
			request = request.json(&body)
		}
		let response = request
			.send()
			.await
			.map_err(|_| Error::BrokerUnavailable { correlation: None })?;
		let correlation = response
			.headers()
			.get("x-aidash-correlation")
			.and_then(|v| v.to_str().ok())
			.map(str::to_owned);
		if let Some(code) = response
			.headers()
			.get("x-aidash-broker-error")
			.and_then(|v| v.to_str().ok())
		{
			tracing::warn!(
				code,
				correlation = correlation.as_deref().unwrap_or(""),
				"OAuth broker refused an operation"
			);
			return Err(match code {
				"connection_required" | "secret_unavailable" | "secret_destination_refused" => {
					Error::Invalid(code.into())
				}
				"broker_rejected" => Error::BrokerRejected { correlation },
				"upstream_unreachable" | "rate_limited" => unavailable(),
				_ => Error::BrokerUnavailable { correlation },
			});
		}
		if !response.status().is_success() {
			return Err(Error::BrokerUnavailable { correlation });
		}

		if response.status() == reqwest::StatusCode::NO_CONTENT {
			return Ok(None);
		}
		let receipt: Receipt = bounded_json(response, 65536)
			.await
			.map_err(|_| unavailable())?;
		if !aidash_capability::connection::version_valid(&receipt.version) {
			return Err(unavailable());
		}
		Ok(Some(receipt))
	}
}
fn unavailable() -> Error {
	Error::External("connection_unavailable".into())
}
fn subject(tenant: &str) -> SecretSubject {
	SecretSubject::Operation {
		operation: Uuid::new_v4(),
		tenant: tenant.into(),
	}
}
#[async_trait]
impl ConnectionCalls for ConnectionCallsAdapter {
	async fn exchange(
		&self,
		tenant: &str,
		connection: Uuid,
		grant: ExchangeGrant,
		code: &str,
		verifier: &SecretString,
	) -> Result<Receipt> {
		self.post(
			"v1/connections/exchange",
			tenant,
			connection,
			ConnectionOperation::Exchange {
				issuer: grant.issuer,
				resource: grant.resource,
				client_id: grant.client_id,
				client: grant.client,
				redirect_uri: grant.redirect_uri,
				sub: subject(tenant),
			},
			Some(serde_json::json!({"code":code,"verifier":verifier.expose_secret()})),
		)
		.await?
		.ok_or_else(unavailable)
	}
	async fn refresh(
		&self,
		tenant: &str,
		connection: Uuid,
		version: &str,
	) -> std::result::Result<Receipt, RefreshFailure> {
		match self
			.post(
				"v1/connections/refresh",
				tenant,
				connection,
				ConnectionOperation::Refresh {
					version: version.into(),
					sub: subject(tenant),
				},
				None,
			)
			.await
		{
			Ok(Some(r)) => Ok(r),
			Err(Error::Invalid(c)) if c == "connection_required" => Err(RefreshFailure::Terminal),
			// A broker write followed by a lost reply is recovered via latest.previous.
			// An uncertain AS response leaves V; a rotated AS token will reject
			// the next attempt with invalid_grant, requiring reconnection.
			Err(e) => Err(RefreshFailure::Unavailable(e)),
			Ok(None) => Err(RefreshFailure::Unavailable(unavailable())),
		}
	}
	async fn revoke(&self, tenant: &str, connection: Uuid, version: &str) -> Result<()> {
		self.post(
			"v1/connections/revoke",
			tenant,
			connection,
			ConnectionOperation::Revoke {
				version: version.into(),
			},
			None,
		)
		.await
		.map(|_| ())
	}
	async fn route(
		&self,
		tenant: &str,
		connection: Uuid,
		version: &str,
		call: SecretCall,
	) -> Result<SecretRoute> {
		let token = self
			.token(
				tenant,
				connection,
				ConnectionOperation::Forward {
					version: version.into(),
					destination: call.destination,
					methods: call.methods,
					sub: call.subject,
					max_response_bytes: call.max_response_bytes,
				},
			)
			.await?;
		Ok(SecretRoute {
			base: self.base.join("v1/forward").map_err(|_| unavailable())?,
			token,
		})
	}
}

#[cfg(test)]
mod tests;
