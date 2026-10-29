//! OAuth operations in the Tenant Secret role. Only the broker reads tokens;
//! signed callers choose identities, never token or revocation endpoints.
use crate::tenant_secret::{
	AuditGuard, BROKER_REJECTED, BROKER_UNAVAILABLE, DESTINATION_REFUSED, RATE_LIMITED,
	SECRET_UNAVAILABLE, SecretCallAudit, TenantSecretBroker, UPSTREAM_UNREACHABLE, path_digest,
	subject_kind,
};
use aidash_application::{
	connection_calls::Receipt,
	plan_connections::RefreshFailure,
	ports::oauth::{AuthorizationServer, ClientCredentials, OAuthDiscovery, OAuthTokens},
};
use aidash_capability::{
	KeyMaterialError,
	connection::{
		ClientKind, ClientSecretRef, ConnectionClaims, ConnectionOperation, ConnectionStore,
		sealed::{self, Payload},
	},
};
use aidash_integrations::oauth::McpOAuth;
use axum::{
	body::to_bytes,
	extract::Request,
	http::{Method, StatusCode},
	response::Response,
};
use chrono::{Duration as ChronoDuration, Utc};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::{
	collections::HashMap,
	sync::{Arc, Mutex},
	time::Duration,
};
use tokio::time::Instant;
use uuid::Uuid;

pub struct ConnectionBroker {
	source: Arc<dyn ConnectionStore>,
	oauth: McpOAuth,
	metadata: Mutex<HashMap<String, (AuthorizationServer, Instant)>>,
}
impl ConnectionBroker {
	pub fn new(source: Arc<dyn ConnectionStore>) -> Result<Self, &'static str> {
		Ok(Self {
			source,
			oauth: McpOAuth::new().map_err(|_| "invalid OAuth HTTP client")?,
			metadata: Mutex::default(),
		})
	}
	#[cfg(test)]
	pub(crate) fn for_test(source: Arc<dyn ConnectionStore>) -> Self {
		Self {
			source,
			oauth: McpOAuth::for_test().unwrap(),
			metadata: Mutex::default(),
		}
	}
	pub(crate) async fn metadata(&self, issuer: &str) -> Result<AuthorizationServer, &'static str> {
		{
			let mut cache = self.metadata.lock().unwrap();
			cache.retain(|_, (_, since)| since.elapsed() < Duration::from_secs(300));
			if let Some((s, _)) = cache.get(issuer) {
				return Ok(s.clone());
			}
		}
		let server = self
			.oauth
			.authorization_server(issuer)
			.await
			.map_err(|_| UPSTREAM_UNREACHABLE)?;
		let mut cache = self.metadata.lock().unwrap();
		if cache.len() < 10000 {
			cache.insert(issuer.into(), (server.clone(), Instant::now()));
		}
		Ok(server)
	}
	fn resource(&self, broker: &TenantSecretBroker, id: Uuid) -> String {
		format!(
			"projects/{}/secrets/{}oauth-{id}",
			broker.config.byok_project_number,
			broker
				.config
				.secret_prefix
				.strip_suffix("tsec-")
				.expect("validated tsec prefix")
		)
	}
	async fn read(
		&self,
		broker: &TenantSecretBroker,
		id: Uuid,
		version: &str,
	) -> Result<Payload, &'static str> {
		let bytes = self
			.source
			.access(&self.resource(broker, id), version)
			.await
			.map_err(store_error)?;
		sealed::open(&bytes).map_err(|_| SECRET_UNAVAILABLE)
	}
	async fn credentials(
		&self,
		broker: &TenantSecretBroker,
		issuer: &str,
		client_id: &str,
		redirect: Option<&str>,
		reference: &Option<ClientSecretRef>,
	) -> Result<ClientCredentials, &'static str> {
		let secret = if let Some(c) = reference {
			let (_, bytes) = self
				.source
				.latest(&self.resource(broker, c.id))
				.await
				.map_err(store_error)?;
			let payload = sealed::open(&bytes).map_err(|_| SECRET_UNAVAILABLE)?;
			match (payload, &c.kind) {
				(
					Payload::RegisteredClient {
						issuer: i,
						client_id: id,
						client_secret,
					},
					ClientKind::Registered,
				) if i == issuer && id == client_id => Some(client_secret),
				(
					Payload::DynamicClient {
						issuer: i,
						client_id: id,
						redirect_uri,
						client_secret,
					},
					ClientKind::Dynamic,
				) if i == issuer
					&& id == client_id
					&& redirect.is_some_and(|r| r == redirect_uri) =>
				{
					Some(client_secret)
				}
				_ => return Err(DESTINATION_REFUSED),
			}
		} else {
			None
		};
		Ok(ClientCredentials {
			client_id: client_id.into(),
			client_secret: secret,
		})
	}
	async fn write(
		&self,
		broker: &TenantSecretBroker,
		id: Uuid,
		payload: Payload,
		expires_in: Option<u64>,
		scope: Option<String>,
		refresh: bool,
	) -> Result<Receipt, &'static str> {
		let bytes = sealed::seal(payload).map_err(|_| SECRET_UNAVAILABLE)?;
		let version = self
			.source
			.add_version(&self.resource(broker, id), &bytes)
			.await
			.map_err(store_error)?;
		if !aidash_capability::connection::version_valid(&version) {
			return Err(BROKER_UNAVAILABLE);
		}
		Ok(Receipt {
			version,
			expires_in,
			scope,
			refresh,
		})
	}
	async fn perform(
		&self,
		broker: &TenantSecretBroker,
		c: &ConnectionClaims,
		body: Option<ExchangeBody>,
		audit: &mut AuditGuard,
	) -> Result<Option<Receipt>, &'static str> {
		match &c.operation {
			ConnectionOperation::Exchange {
				issuer,
				resource,
				client_id,
				client,
				redirect_uri,
				..
			} => {
				let credentials = self
					.credentials(broker, issuer, client_id, Some(redirect_uri), client)
					.await?;
				let server = self.metadata(issuer).await?;
				audit.entry.host = server.token_endpoint.host_str().unwrap_or_default().into();
				audit.entry.path_sha256 = path_digest(server.token_endpoint.path());
				let body = body.ok_or(BROKER_REJECTED)?;
				let redirect = url::Url::parse(redirect_uri).map_err(|_| BROKER_REJECTED)?;
				let tokens = self
					.oauth
					.exchange(
						&server,
						&credentials,
						body.code.expose_secret(),
						&body.verifier,
						&redirect,
						resource,
					)
					.await
					.map_err(|_| UPSTREAM_UNREACHABLE)?;
				let expires_at = expiry(tokens.expires_in)?;
				let refresh = tokens.refresh_token.is_some();
				let scope = tokens.scope.clone();
				let expires_in = tokens.expires_in;
				let payload = Payload::Connection {
					issuer: issuer.clone(),
					resource: resource.clone(),
					client_id: client_id.clone(),
					client: client.clone(),
					redirect_uri: Some(redirect_uri.clone()),
					access_token: tokens.access_token,
					refresh_token: tokens.refresh_token,
					expires_at,
					scope: scope.clone(),
					previous: None,
				};
				self.write(broker, c.connection, payload, expires_in, scope, refresh)
					.await
					.map(Some)
			}
			ConnectionOperation::Refresh { version, .. } => {
				let (latest, bytes) = self
					.source
					.latest(&self.resource(broker, c.connection))
					.await
					.map_err(store_error)?;
				let payload = sealed::open(&bytes).map_err(|_| SECRET_UNAVAILABLE)?;
				if latest != *version {
					if let Payload::Connection {
						ref issuer,
						ref client_id,
						ref client,
						ref redirect_uri,
						previous: Some(ref previous),
						expires_at,
						scope,
						refresh_token,
						..
					} = payload && previous == version
					{
						self.credentials(
							broker,
							issuer,
							client_id,
							redirect_uri.as_deref(),
							client,
						)
						.await?;
						audit.entry.host = url::Url::parse(issuer)
							.ok()
							.and_then(|u| u.host_str().map(str::to_owned))
							.unwrap_or_default();
						let expires_in = expires_at
							.as_deref()
							.map(|t| {
								chrono::DateTime::parse_from_rfc3339(t)
									.map(|t| {
										t.signed_duration_since(Utc::now()).num_seconds().max(0)
											as u64
									})
									.map_err(|_| SECRET_UNAVAILABLE)
							})
							.transpose()?;
						return Ok(Some(Receipt {
							version: latest,
							expires_in,
							scope,
							refresh: refresh_token.is_some(),
						}));
					}
					return Err("connection_required");
				}
				let Payload::Connection {
					issuer,
					resource,
					client_id,
					client,
					redirect_uri,
					refresh_token,
					scope: old_scope,
					..
				} = payload
				else {
					return Err(SECRET_UNAVAILABLE);
				};
				let refresh_token = refresh_token.ok_or("connection_required")?;
				let credentials = self
					.credentials(
						broker,
						&issuer,
						&client_id,
						redirect_uri.as_deref(),
						&client,
					)
					.await?;
				let server = self.metadata(&issuer).await?;
				audit.entry.host = server.token_endpoint.host_str().unwrap_or_default().into();
				audit.entry.path_sha256 = path_digest(server.token_endpoint.path());
				let tokens = self
					.oauth
					.refresh(&server, &credentials, &refresh_token, &resource)
					.await
					.map_err(|e| match e {
						RefreshFailure::Terminal => "connection_required",
						_ => UPSTREAM_UNREACHABLE,
					})?;
				let expires_at = expiry(tokens.expires_in)?;
				let scope = tokens.scope.or(old_scope);
				let expires_in = tokens.expires_in;
				let payload = Payload::Connection {
					issuer,
					resource,
					client_id,
					client,
					redirect_uri,
					access_token: tokens.access_token,
					refresh_token: tokens.refresh_token.or(Some(refresh_token)),
					expires_at,
					scope: scope.clone(),
					previous: Some(version.clone()),
				};
				self.write(broker, c.connection, payload, expires_in, scope, true)
					.await
					.map(Some)
			}
			ConnectionOperation::Revoke { version } => {
				// Validate the latest client binding before revoking upstream.
				// AS failure is reported, and the caller still destroys local grant versions.
				let Payload::Connection {
					issuer,
					client_id,
					client,
					redirect_uri,
					access_token,
					refresh_token,
					..
				} = self.read(broker, c.connection, version).await?
				else {
					return Err(SECRET_UNAVAILABLE);
				};
				let credentials = self
					.credentials(
						broker,
						&issuer,
						&client_id,
						redirect_uri.as_deref(),
						&client,
					)
					.await?;
				let server = self.metadata(&issuer).await?;
				if let Some(endpoint) = &server.revocation_endpoint {
					audit.entry.host = endpoint.host_str().unwrap_or_default().into();
					audit.entry.path_sha256 = path_digest(endpoint.path());
				}
				self.oauth
					.revoke(
						&server,
						&credentials,
						refresh_token.as_ref().unwrap_or(&access_token),
					)
					.await
					.map_err(|_| UPSTREAM_UNREACHABLE)?;
				Ok(None)
			}
			ConnectionOperation::Forward { .. } => Err(BROKER_REJECTED),
		}
	}
	pub(crate) async fn bearer(
		&self,
		broker: &TenantSecretBroker,
		id: Uuid,
		version: &str,
		origin: &str,
		path: &str,
	) -> Result<SecretString, &'static str> {
		let Payload::Connection {
			resource,
			access_token,
			..
		} = self.read(broker, id, version).await?
		else {
			return Err(SECRET_UNAVAILABLE);
		};
		let resource = url::Url::parse(&resource).map_err(|_| SECRET_UNAVAILABLE)?;
		// Validate signed prefix segments too: a compromised minter can put
		// traversal, encoded separators or a second encoding in the prefix.
		for segment in path.split('/') {
			let decoded = percent_encoding::percent_decode_str(segment).collect::<Vec<_>>();
			if decoded == b"."
				|| decoded == b".."
				|| decoded
					.iter()
					.any(|b| matches!(b, b'/' | b'\\' | b'%') || b.is_ascii_control())
			{
				return Err(DESTINATION_REFUSED);
			}
		}
		let destination =
			url::Url::parse(&format!("{origin}{path}")).map_err(|_| DESTINATION_REFUSED)?;
		// A resource is an exact path or a subtree, never a sibling such as /mcp-evil.
		let base = resource.path().trim_end_matches('/');
		if resource.origin() != destination.origin()
			|| !(destination.path() == resource.path()
				|| destination.path().starts_with(&format!("{base}/")))
		{
			return Err(DESTINATION_REFUSED);
		}
		Ok(access_token)
	}
}
fn store_error(e: KeyMaterialError) -> &'static str {
	match e {
		KeyMaterialError::Unavailable => SECRET_UNAVAILABLE,
		KeyMaterialError::StoreUnavailable => BROKER_UNAVAILABLE,
	}
}
fn expiry(seconds: Option<u64>) -> Result<Option<String>, &'static str> {
	seconds
		.map(|v| {
			i64::try_from(v)
				.ok()
				.and_then(ChronoDuration::try_seconds)
				.and_then(|d| Utc::now().checked_add_signed(d))
				.map(|t| t.to_rfc3339())
				.ok_or(UPSTREAM_UNREACHABLE)
		})
		.transpose()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExchangeBody {
	code: SecretString,
	verifier: SecretString,
}
pub(crate) async fn operation(broker: Arc<TenantSecretBroker>, request: Request) -> Response {
	let started = Instant::now();
	let mut audit = AuditGuard::new(
		broker.audit.clone(),
		started,
		SecretCallAudit {
			tenant: String::new(),
			secret: Uuid::nil(),
			version: String::new(),
			host: String::new(),
			path_sha256: path_digest(request.uri().path()),
			method: request.method().to_string(),
			status: 499,
			bytes_in: 0,
			bytes_out: 0,
			jti: Uuid::nil(),
			subject: "unknown".into(),
			duration_ms: 0,
		},
	);
	let Some(token) = request
		.headers()
		.get("x-aidash-secret-call")
		.and_then(|v| v.to_str().ok())
	else {
		return audit.error(StatusCode::UNAUTHORIZED, BROKER_REJECTED, None);
	};
	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();
	let c = match broker.keys.verify_connection(
		token,
		&broker.config.issuers,
		&broker.config.audience,
		now,
	) {
		Ok(c) => c,
		Err(_) => return audit.error(StatusCode::UNAUTHORIZED, BROKER_REJECTED, None),
	};
	audit.entry.tenant = c.tenant.clone();
	audit.entry.secret = c.connection;
	audit.entry.jti = c.jti;
	let (path, sub, version) = match &c.operation {
		ConnectionOperation::Exchange { sub, .. } => ("/v1/connections/exchange", Some(sub), ""),
		ConnectionOperation::Refresh { sub, version } => {
			("/v1/connections/refresh", Some(sub), version.as_str())
		}
		ConnectionOperation::Revoke { version } => {
			("/v1/connections/revoke", None, version.as_str())
		}
		_ => return audit.error(StatusCode::UNAUTHORIZED, BROKER_REJECTED, Some(c.jti)),
	};
	audit.entry.version = version.into();
	audit.entry.subject = sub.map(subject_kind).unwrap_or("operation").into();
	if request.method() != Method::POST
		|| request.uri().path() != path
		|| request.uri().query().is_some()
	{
		return audit.error(StatusCode::UNAUTHORIZED, BROKER_REJECTED, Some(c.jti));
	}
	if !crate::take_token(
		&broker.buckets,
		(c.tenant.clone(), c.connection),
		broker.config.requests_per_second,
		broker.config.burst,
	) {
		return audit.error(StatusCode::TOO_MANY_REQUESTS, RATE_LIMITED, Some(c.jti));
	}
	let Some(connections) = &broker.connections else {
		return audit.error(
			StatusCode::SERVICE_UNAVAILABLE,
			BROKER_UNAVAILABLE,
			Some(c.jti),
		);
	};
	let body = if matches!(c.operation, ConnectionOperation::Exchange { .. }) {
		let bytes = match to_bytes(request.into_body(), 65536).await {
			Ok(v) => v,
			Err(_) => return audit.error(StatusCode::UNAUTHORIZED, BROKER_REJECTED, Some(c.jti)),
		};
		audit
			.bytes_in
			.store(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
		match serde_json::from_slice(&bytes) {
			Ok(b) => Some(b),
			Err(_) => return audit.error(StatusCode::UNAUTHORIZED, BROKER_REJECTED, Some(c.jti)),
		}
	} else {
		None
	};
	match connections.perform(&broker, &c, body, &mut audit).await {
		Ok(Some(receipt)) => {
			audit.entry.version = receipt.version.clone();
			let body = serde_json::to_vec(&receipt).expect("receipt JSON");
			audit.entry.status = 200;
			audit
				.bytes_out
				.store(body.len() as u64, std::sync::atomic::Ordering::Relaxed);
			Response::builder()
				.status(200)
				.header("content-type", "application/json")
				.header("cache-control", "no-store")
				.body(axum::body::Body::from(body))
				.unwrap()
		}
		Ok(None) => {
			audit.entry.status = 204;
			Response::builder()
				.status(204)
				.body(axum::body::Body::empty())
				.unwrap()
		}
		Err(code) => {
			let status = match code {
				BROKER_REJECTED => StatusCode::UNAUTHORIZED,
				DESTINATION_REFUSED => StatusCode::FORBIDDEN,
				SECRET_UNAVAILABLE => StatusCode::NOT_FOUND,
				BROKER_UNAVAILABLE => StatusCode::SERVICE_UNAVAILABLE,
				"connection_required" => StatusCode::UNAUTHORIZED,
				_ => StatusCode::BAD_GATEWAY,
			};
			audit.error(status, code, Some(c.jti))
		}
	}
}

#[cfg(test)]
mod tests;
