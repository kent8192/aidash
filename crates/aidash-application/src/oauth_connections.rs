//! Per-person MCP OAuth grants (ADR 0100). Tokens never enter metadata responses.
pub use crate::plan_connections::Owner;
use crate::{
	Error, Result,
	connection_calls::{ConnectionCalls, ExchangeGrant, Receipt},
	plan_connections::RefreshFailure,
	ports::{oauth::*, registry::DefinitionLookup},
	provider_access::KeyMaterialReader,
	provider_authorization::{challenge, random_secret},
	provider_credentials::{ClientBinding, Store},
	tenant_secrets::{SecretCall, SecretRoute},
};
use aidash_domain::oauth_connections::{
	self as domain, ClientRef, ConnectionState, OAuthConnection,
};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use url::Url;
use uuid::Uuid;

pub const LIFETIME_MINUTES: i64 = 15;
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ConnectionToolReference")]
pub struct ToolRef {
	pub id: String,
	pub version: String,
}
/// Private pending state. The server adds its browser/session binding before reserving it.
#[derive(Serialize, Deserialize)]
pub struct Pending {
	pub owner_tenant: String,
	pub owner_subject: String,
	pub resource: String,
	pub issuer: String,
	pub client: ClientRef,
	pub scopes: Vec<String>,
	pub pkce_verifier: String,
	pub redirect_uri: String,
	pub return_to: String,
	/// A disconnected/replaced connection must never be resurrected by its old callback.
	pub connection: Uuid,
	pub revision: i64,
}
pub struct Begin {
	pub authorization_url: String,
	pub state: SecretString,
	pub pending: Pending,
}
#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ConnectionAuthorizationOutcome")]
pub enum Outcome {
	Connected,
	Reconnected,
	Denied,
	Failed,
}
impl Outcome {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Connected => "connected",
			Self::Reconnected => "reconnected",
			Self::Denied => "denied",
			Self::Failed => "failed",
		}
	}
}
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "OAuthConnectionMetadata")]
pub struct Metadata {
	pub id: Uuid,
	pub tenant: String,
	pub subject: String,
	pub resource: String,
	pub issuer: String,
	pub scopes: Vec<String>,
	pub state: ConnectionState,
	pub access_expires_at: Option<DateTime<Utc>>,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
	pub revision: i64,
}
impl From<OAuthConnection> for Metadata {
	fn from(r: OAuthConnection) -> Self {
		Self {
			id: r.id,
			tenant: r.tenant,
			subject: r.subject,
			resource: r.resource,
			issuer: r.issuer,
			scopes: r.scopes,
			state: r.state,
			access_expires_at: r.access_expires_at,
			created_at: r.created_at,
			updated_at: r.updated_at,
			revision: r.revision,
		}
	}
}
pub struct Bearer {
	pub connection: Uuid,
	pub access_token: SecretString,
}
#[derive(Serialize, schemars::JsonSchema)]
#[schemars(rename = "ConnectionDisconnection")]
pub struct Disconnection {
	pub revoked: bool,
}
#[derive(Serialize, schemars::JsonSchema)]
#[schemars(rename = "ConnectionProbe")]
pub struct Probe {
	pub oauth: bool,
	pub scopes_supported: Option<Vec<String>>,
	pub issuer: Option<String>,
}
#[derive(Clone)]
pub struct DynamicClient {
	pub id: Uuid,
	pub tenant: String,
	pub issuer: String,
	pub redirect_uri: String,
	pub client_id: String,
	pub secret_resource: String,
	pub pinned_version: String,
}
/// Every mutation and token refresh holds one owner's advisory transaction lock.
#[async_trait]
pub trait Scope: Send {
	async fn list(&mut self) -> Result<Vec<OAuthConnection>>;
	async fn save(&mut self, row: &OAuthConnection) -> Result<()>;
	async fn delete(&mut self, id: Uuid) -> Result<()>;
	async fn cancel_attempts(&mut self, resource: Option<&str>) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait Repository: Send + Sync {
	async fn begin(&self, owner: &Owner) -> Result<Box<dyn Scope>>;
	async fn owners(&self, tenant: &str) -> Result<Vec<Owner>>;
	async fn dynamic_client(
		&self,
		tenant: &str,
		issuer: &str,
		redirect_uri: &str,
	) -> Result<Option<DynamicClient>>;
	/// Returns the winner of a concurrent registration. The loser destroys its unused resource.
	async fn cache_client(&self, client: &DynamicClient) -> Result<DynamicClient>;
	async fn dynamic_clients(&self, tenant: &str) -> Result<Vec<DynamicClient>>;
	async fn delete_dynamic_client(&self, tenant: &str, id: Uuid) -> Result<()>;
	async fn answer_requests(&self, owner: &Owner, resource: &str) -> Result<()>;
}
#[derive(Serialize, Deserialize)]
struct Stored {
	access_token: String,
	refresh_token: Option<String>,
	expires_at: Option<DateTime<Utc>>,
	scope: Vec<String>,
	redirect_uri: String,
	#[serde(default)]
	replacement: Option<Intent>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Intent {
	connection: Uuid,
	revision: i64,
	pin: String,
}
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Journal {
	Tokens(Stored),
	Intent { refresh_intent: Intent },
}
fn seal<T: Serialize>(v: &T) -> Result<SecretString> {
	Ok(serde_json::to_string(v)?.into())
}
fn unavailable() -> Error {
	Error::External(domain::CONNECTION_UNAVAILABLE.into())
}
fn required() -> Error {
	Error::Invalid(domain::CONNECTION_REQUIRED.into())
}
fn gone(r: Result<()>) -> Result<()> {
	match r {
		Err(Error::NotFound(_)) => Ok(()),
		r => r,
	}
}

pub struct Service {
	pub repository: Arc<dyn Repository>,
	pub store: Arc<dyn Store>,
	pub reader: Option<Arc<dyn KeyMaterialReader>>,
	pub calls: Option<Arc<dyn ConnectionCalls>>,
	pub discovery: Arc<dyn OAuthDiscovery>,
	pub tokens: Arc<dyn OAuthTokens>,
	pub policy: Arc<dyn ConnectionPolicy>,
}
impl Service {
	fn reader(&self) -> Result<&dyn KeyMaterialReader> {
		self.reader.as_deref().ok_or_else(unavailable)
	}

	pub async fn list(&self, owner: &Owner) -> Result<Vec<Metadata>> {
		let mut s = self.repository.begin(owner).await?;
		let rows = s.list().await?;
		s.commit().await?;
		Ok(rows.into_iter().map(Into::into).collect())
	}
	/// `endpoint` is the configured transport URL. Discovery initializes that exact URL: a
	/// server mounted only at `/mcp/` may not answer its canonical form `/mcp`.
	pub async fn probe(&self, endpoint: &str) -> Result<Probe> {
		domain::canonical_resource(endpoint)?;
		let prm = self.discovery.protected_resource(endpoint).await?;
		Ok(match prm {
			None => Probe {
				oauth: false,
				scopes_supported: None,
				issuer: None,
			},
			Some(p) => Probe {
				oauth: true,
				scopes_supported: p.scopes_supported,
				issuer: p.authorization_servers.first().cloned(),
			},
		})
	}
	// Keep the browser-bound fields explicit alongside the registry read port.
	#[allow(clippy::too_many_arguments)]
	pub async fn begin(
		&self,
		owner: &Owner,
		resource: &str,
		requested: &[String],
		tool: Option<&ToolRef>,
		lookup: &mut dyn DefinitionLookup,
		redirect_uri: &Url,
		return_to: &str,
	) -> Result<Begin> {
		// The canonical form is the Connection's identity and resource indicator; discovery
		// probes the configured transport URL itself (see `probe`).
		let endpoint = resource;
		let resource = domain::canonical_resource(endpoint)?;
		let declared = if let Some(tool) = tool {
			let entry = lookup.definition(&tool.id, &tool.version).await?;
			if entry.kind != "tool" {
				return Err(Error::Invalid(
					"connection tool must name an MCP tool".into(),
				));
			}
			let config = match aidash_domain::tool::legacy_config(&entry.config)? {
				Some(config) => Some(config),
				None => {
					serde_json::from_value::<aidash_domain::tool::providers::ToolDescriptor>(
						entry.config,
					)?
					.transport
				}
			};
			match config {
				Some(aidash_domain::tool::ToolConfig::Mcp {
					endpoint, scopes, ..
				}) if domain::canonical_resource(&endpoint)? == resource => scopes,
				_ => {
					return Err(Error::Invalid(
						"connection tool does not name this MCP resource".into(),
					));
				}
			}
		} else {
			None
		};
		let prm = self
			.discovery
			.protected_resource(endpoint)
			.await?
			.ok_or_else(|| Error::Invalid("this MCP server does not require OAuth".into()))?;
		if domain::canonical_resource(&prm.resource)? != resource {
			return Err(Error::Invalid("OAuth resource metadata mismatch".into()));
		}
		let issuer = prm.authorization_servers.first().ok_or_else(|| {
			Error::Invalid("MCP server advertises no authorization server".into())
		})?;
		let server = self.discovery.authorization_server(issuer).await?;
		if !server
			.code_challenge_methods_supported
			.iter()
			.any(|m| m == "S256")
		{
			return Err(Error::Invalid(
				"authorization server must support PKCE S256".into(),
			));
		}
		let ceiling = self.policy.scope_ceiling(&owner.tenant, &resource).await?;
		let scopes = domain::admissible_scopes(
			requested,
			declared.as_deref(),
			prm.scopes_supported.as_deref(),
			ceiling.as_deref(),
		)?;
		let (client_ref, credentials) = self.pick_client(owner, &server, redirect_uri).await?;
		let mut scope = self.repository.begin(owner).await?;
		let existing = scope
			.list()
			.await?
			.into_iter()
			.find(|r| r.resource == resource);
		let row = match existing {
			Some(r) => r,
			None => {
				let id = Uuid::now_v7();
				let now = Utc::now();
				self.store.create_connection(&owner.tenant, id).await?;
				OAuthConnection {
					id,
					tenant: owner.tenant.clone(),
					subject: owner.subject.clone(),
					resource: resource.clone(),
					issuer: issuer.clone(),
					client: client_ref.clone(),
					scopes: scopes.clone(),
					secret_resource: self.store.connection_resource(id),
					pinned_version: String::new(),
					state: ConnectionState::Pending,
					access_expires_at: None,
					refresh_expires_at: None,
					created_at: now,
					updated_at: now,
					revision: 1,
				}
			}
		};
		scope.save(&row).await?;
		scope.commit().await?;
		let state = SecretString::from(random_secret());
		let verifier = SecretString::from(random_secret());
		let mut url = server.authorization_endpoint;
		url.query_pairs_mut()
			.append_pair("response_type", "code")
			.append_pair("client_id", &credentials.client_id)
			.append_pair("redirect_uri", redirect_uri.as_str())
			.append_pair("state", state.expose_secret())
			.append_pair("code_challenge", &challenge(&verifier))
			.append_pair("code_challenge_method", "S256")
			.append_pair("resource", &resource)
			.append_pair("scope", &scopes.join(" "));
		Ok(Begin {
			authorization_url: url.into(),
			state,
			pending: Pending {
				owner_tenant: owner.tenant.clone(),
				owner_subject: owner.subject.clone(),
				resource,
				issuer: issuer.clone(),
				client: client_ref,
				scopes,
				pkce_verifier: verifier.expose_secret().into(),
				redirect_uri: redirect_uri.to_string(),
				return_to: return_to.into(),
				connection: row.id,
				revision: row.revision,
			},
		})
	}
	async fn pick_client(
		&self,
		owner: &Owner,
		server: &AuthorizationServer,
		redirect_uri: &Url,
	) -> Result<(ClientRef, ClientCredentials)> {
		let (person, tenant) = self
			.policy
			.registered_clients(&owner.tenant, &owner.subject, &server.issuer)
			.await?;
		if let Some(client) = domain::pick_client(person.as_ref(), tenant.as_ref()) {
			let secret = match (&client.secret_resource, &client.pinned_version) {
				(Some(r), Some(v)) if self.calls.is_none() => {
					Some(self.reader()?.read(&owner.tenant, r, v).await?)
				}
				(Some(_), Some(_)) => None,
				(None, None) => None,
				_ => return Err(unavailable()),
			};
			return Ok((
				ClientRef::Registered { client: client.id },
				ClientCredentials {
					client_id: client.client_id.clone(),
					client_secret: secret,
				},
			));
		}
		let cached = self
			.repository
			.dynamic_client(&owner.tenant, &server.issuer, redirect_uri.as_str())
			.await?;
		let cached = if let Some(cached) = cached {
			cached
		} else {
			let issued = self
				.discovery
				.register(server, redirect_uri, "Aidash")
				.await?;
			let id = Uuid::now_v7();
			let resource = self.store.connection_resource(id);
			let pin = if issued.client_secret.is_some() {
				self.store.create_connection(&owner.tenant, id).await?;
				let payload = seal(
					&serde_json::json!({"client_secret":issued.client_secret.as_ref().map(ExposeSecret::expose_secret)}),
				)?;
				self.store
					.add_client_version(
						&owner.tenant,
						&resource,
						ClientBinding {
							issuer: server.issuer.clone(),
							client_id: issued.client_id.clone(),
							redirect_uri: Some(redirect_uri.to_string()),
						},
						&payload,
					)
					.await?
			} else {
				String::new()
			};
			let candidate = DynamicClient {
				id,
				tenant: owner.tenant.clone(),
				issuer: server.issuer.clone(),
				redirect_uri: redirect_uri.to_string(),
				client_id: issued.client_id,
				secret_resource: resource.clone(),
				pinned_version: pin,
			};
			let winner = match self.repository.cache_client(&candidate).await {
				Ok(winner) => winner,
				Err(e) => {
					let _ = self.store.delete(&owner.tenant, &resource).await;
					return Err(e);
				}
			};
			if winner.id != id {
				gone(self.store.delete(&owner.tenant, &resource).await)?;
			}
			winner
		};
		let client_secret = if self.calls.is_none() && !cached.pinned_version.is_empty() {
			let secret = self
				.reader()?
				.read(
					&owner.tenant,
					&cached.secret_resource,
					&cached.pinned_version,
				)
				.await?;
			let payload: serde_json::Value =
				serde_json::from_str(secret.expose_secret()).map_err(|_| unavailable())?;
			payload["client_secret"]
				.as_str()
				.map(|s| SecretString::from(s.to_owned()))
		} else {
			None
		};
		Ok((
			ClientRef::Dynamic {
				client_id: cached.client_id.clone(),
			},
			ClientCredentials {
				client_id: cached.client_id,
				client_secret,
			},
		))
	}
	async fn exchange_grant(&self, owner: &Owner, p: &Pending) -> Result<ExchangeGrant> {
		use aidash_capability::connection::{ClientKind, ClientSecretRef};
		let (client_id, client) = match &p.client {
			ClientRef::Registered { client: id } => {
				let (person, tenant) = self
					.policy
					.registered_clients(&owner.tenant, &owner.subject, &p.issuer)
					.await?;
				let c = person
					.into_iter()
					.chain(tenant)
					.find(|c| c.id == *id)
					.ok_or_else(required)?;
				let reference = match (c.secret_resource, c.pinned_version) {
					(Some(_), Some(_)) => Some(ClientSecretRef {
						kind: ClientKind::Registered,
						id: c.id,
					}),
					(None, None) => None,
					_ => return Err(unavailable()),
				};
				(c.client_id, reference)
			}
			ClientRef::Dynamic { client_id } => {
				let c = self
					.repository
					.dynamic_client(&owner.tenant, &p.issuer, &p.redirect_uri)
					.await?
					.filter(|c| &c.client_id == client_id)
					.ok_or_else(required)?;
				let reference = if c.pinned_version.is_empty() {
					None
				} else {
					Some(ClientSecretRef {
						kind: ClientKind::Dynamic,
						id: c.id,
					})
				};
				(c.client_id, reference)
			}
		};
		Ok(ExchangeGrant {
			issuer: p.issuer.clone(),
			resource: p.resource.clone(),
			client_id,
			client,
			redirect_uri: p.redirect_uri.clone(),
		})
	}
	/// Metadata-only resolution for Cloud. Every route rechecks the current pin.
	pub async fn brokered_connection(&self, owner: &Owner, resource: &str) -> Result<Uuid> {
		let resource = domain::canonical_resource(resource)?;
		let (scope, row) = self.active_scope(owner, &resource).await?;
		scope.commit().await?;
		Ok(row.id)
	}
	pub async fn brokered_route(
		&self,
		owner: &Owner,
		resource: &str,
		call: SecretCall,
	) -> Result<SecretRoute> {
		self.brokered_route_inner(owner, resource, None, call).await
	}
	/// A cached session must never switch to a replacement connection between
	/// its identity check and token minting. Check under the same owner lock.
	pub async fn brokered_route_for(
		&self,
		owner: &Owner,
		resource: &str,
		connection: Uuid,
		call: SecretCall,
	) -> Result<SecretRoute> {
		self.brokered_route_inner(owner, resource, Some(connection), call)
			.await
	}
	async fn brokered_route_inner(
		&self,
		owner: &Owner,
		resource: &str,
		expected: Option<Uuid>,
		call: SecretCall,
	) -> Result<SecretRoute> {
		let calls = self.calls.as_ref().ok_or_else(unavailable)?;
		let resource = domain::canonical_resource(resource)?;
		let (mut scope, mut row) = self.active_scope(owner, &resource).await?;
		if expected.is_some_and(|id| id != row.id) {
			return Err(required());
		}
		if row
			.access_expires_at
			.is_some_and(|t| t <= Utc::now() + Duration::seconds(60))
		{
			match calls
				.refresh(&row.tenant, row.id, numeric_pin(&row.pinned_version)?)
				.await
			{
				Ok(receipt) => {
					let requested = row.scopes.clone();
					apply_receipt(&mut row, &receipt, &requested)?;
					scope.save(&row).await?;
				}
				Err(RefreshFailure::Terminal) => {
					row.state = ConnectionState::NeedsReconnection;
					row.updated_at = Utc::now();
					row.revision += 1;
					scope.save(&row).await?;
					scope.commit().await?;
					return Err(required());
				}
				// A persisted broker receipt makes replay safe after a lost
				// response. If the AS response itself was lost, invalid_grant
				// on a later retry will require reconnection.
				Err(RefreshFailure::Ambiguous(_)) => return Err(unavailable()),
				Err(RefreshFailure::Unavailable(e)) => return Err(e),
			}
		}
		let route = calls
			.route(&row.tenant, row.id, numeric_pin(&row.pinned_version)?, call)
			.await?;
		scope.commit().await?;
		self.prune(&row).await?;
		Ok(route)
	}

	async fn credentials(
		&self,
		owner: &Owner,
		issuer: &str,
		client: &ClientRef,
		redirect_uri: &str,
	) -> Result<ClientCredentials> {
		match client {
			ClientRef::Registered { client: id } => {
				let (person, tenant) = self
					.policy
					.registered_clients(&owner.tenant, &owner.subject, issuer)
					.await?;
				let c = person
					.into_iter()
					.chain(tenant)
					.find(|c| c.id == *id)
					.ok_or_else(required)?;
				Ok(ClientCredentials {
					client_id: c.client_id,
					client_secret: match (c.secret_resource, c.pinned_version) {
						(Some(r), Some(v)) => {
							Some(self.reader()?.read(&owner.tenant, &r, &v).await?)
						}
						(None, None) => None,
						_ => return Err(unavailable()),
					},
				})
			}
			ClientRef::Dynamic { client_id } => {
				let c = self
					.repository
					.dynamic_client(&owner.tenant, issuer, redirect_uri)
					.await?
					.filter(|c| c.client_id == *client_id)
					.ok_or_else(required)?;
				// A public Dynamic client pins no version, and the broker holds a
				// Cloud client's secret: both mirror the cached-client path above.
				let client_secret = if self.calls.is_none() && !c.pinned_version.is_empty() {
					let secret = self
						.reader()?
						.read(&owner.tenant, &c.secret_resource, &c.pinned_version)
						.await?;
					let payload: serde_json::Value =
						serde_json::from_str(secret.expose_secret()).map_err(|_| unavailable())?;
					payload["client_secret"]
						.as_str()
						.map(|s| SecretString::from(s.to_owned()))
				} else {
					None
				};
				Ok(ClientCredentials {
					client_id: c.client_id,
					client_secret,
				})
			}
		}
	}
	/// The transport consumes the pending state once and verifies its browser/session and live actor first.
	pub async fn complete(
		&self,
		pending: Pending,
		code: Option<&str>,
		error: Option<&str>,
	) -> Result<Outcome> {
		if error.is_some() || code.is_none() {
			return Ok(if error == Some("access_denied") {
				Outcome::Denied
			} else {
				Outcome::Failed
			});
		}
		let owner = Owner {
			tenant: pending.owner_tenant.clone(),
			subject: pending.owner_subject.clone(),
		};
		let mut scope = self.repository.begin(&owner).await?;
		let mut row = scope
			.list()
			.await?
			.into_iter()
			.find(|r| {
				r.id == pending.connection
					&& r.resource == pending.resource
					&& r.revision == pending.revision
			})
			.ok_or_else(|| Error::Conflict("connection changed while authorizing".into()))?;
		if let Some(calls) = &self.calls {
			let grant = self.exchange_grant(&owner, &pending).await?;
			let receipt = calls
				.exchange(
					&owner.tenant,
					row.id,
					grant,
					code.unwrap_or_default(),
					&SecretString::from(pending.pkce_verifier.clone()),
				)
				.await?;
			let previous = row.state != ConnectionState::Pending;
			if let Err(error) = apply_receipt(&mut row, &receipt, &pending.scopes) {
				let _ = calls.revoke(&owner.tenant, row.id, &receipt.version).await;
				let _ = self
					.store
					.destroy(
						&row.tenant,
						&format!("{}/versions/{}", row.secret_resource, receipt.version),
					)
					.await;
				return Err(error);
			}
			row.client = pending.client;
			row.issuer = pending.issuer;
			row.state = ConnectionState::Active;
			let saved = async {
				scope.save(&row).await?;
				scope.commit().await
			}
			.await;
			if let Err(e) = saved {
				let _ = calls.revoke(&owner.tenant, row.id, &receipt.version).await;
				return Err(e);
			}
			self.prune(&row).await?;
			self.repository
				.answer_requests(&owner, &row.resource)
				.await?;
			return Ok(if previous {
				Outcome::Reconnected
			} else {
				Outcome::Connected
			});
		}
		let server = self.discovery.authorization_server(&pending.issuer).await?;
		let client = self
			.credentials(
				&owner,
				&pending.issuer,
				&pending.client,
				&pending.redirect_uri,
			)
			.await?;
		let redirect = Url::parse(&pending.redirect_uri).map_err(|_| unavailable())?;
		let issued = self
			.tokens
			.exchange(
				&server,
				&client,
				code.unwrap_or_default(),
				&SecretString::from(pending.pkce_verifier),
				&redirect,
				&pending.resource,
			)
			.await?;
		let previous = row.state != ConnectionState::Pending;
		let mut stored = match stored(&issued, None, &pending.scopes, None) {
			Ok(v) => v,
			Err(e) => {
				let token = issued
					.refresh_token
					.as_ref()
					.unwrap_or(&issued.access_token);
				let _ = self.tokens.revoke(&server, &client, token).await;
				return Err(e);
			}
		};
		stored.redirect_uri = pending.redirect_uri;
		let written = async {
			let pin = self
				.store
				.add_version(&owner.tenant, &row.secret_resource, &seal(&stored)?)
				.await?;
			row.pinned_version = pin;
			row.client = pending.client;
			row.issuer = pending.issuer;
			row.scopes = stored.scope.clone();
			row.access_expires_at = stored.expires_at;
			row.state = ConnectionState::Active;
			row.updated_at = Utc::now();
			row.revision += 1;
			scope.save(&row).await?;
			scope.commit().await
		}
		.await;
		if let Err(e) = written {
			let token = SecretString::from(
				stored
					.refresh_token
					.clone()
					.unwrap_or(stored.access_token.clone()),
			);
			let _ = self.tokens.revoke(&server, &client, &token).await;
			return Err(e);
		}
		self.prune(&row).await?;
		self.repository
			.answer_requests(&owner, &row.resource)
			.await?;
		Ok(if previous {
			Outcome::Reconnected
		} else {
			Outcome::Connected
		})
	}
	async fn read(&self, row: &OAuthConnection, pin: &str) -> Result<Journal> {
		let secret = self
			.reader()?
			.read(&row.tenant, &row.secret_resource, pin)
			.await
			.map_err(|_| unavailable())?;
		serde_json::from_str(secret.expose_secret()).map_err(|_| unavailable())
	}
	async fn prune(&self, row: &OAuthConnection) -> Result<()> {
		let owner = Owner {
			tenant: row.tenant.clone(),
			subject: row.subject.clone(),
		};
		let mut scope = self.repository.begin(&owner).await?;
		if !scope
			.list()
			.await?
			.iter()
			.any(|r| r.id == row.id && r.revision == row.revision)
		{
			return scope.commit().await;
		}
		let versions = self
			.store
			.versions(&row.tenant, &row.secret_resource)
			.await?;
		if self.calls.is_some() {
			let current = numeric_pin(&row.pinned_version)?;
			for pin in versions {
				let candidate = numeric_pin(&pin)?;
				// A newer broker version can be the receipt of a refresh whose
				// database commit was lost. A delayed cleanup must retain it.
				if candidate.len() < current.len()
					|| (candidate.len() == current.len() && candidate < current)
				{
					gone(self.store.destroy(&row.tenant, &pin).await)?;
				}
			}
			return scope.commit().await;
		}
		let base = Intent {
			connection: row.id,
			revision: row.revision,
			pin: row.pinned_version.clone(),
		};
		// A later caller may have refreshed this same pin after our commit.
		// Never let delayed cleanup erase that caller's uncertainty evidence.
		for pin in &versions {
			if pin == &row.pinned_version {
				continue;
			}
			match self.read(row, pin).await? {
				Journal::Intent { refresh_intent } if refresh_intent == base => {
					return scope.commit().await;
				}
				Journal::Tokens(s) if s.replacement.as_ref() == Some(&base) => {
					return scope.commit().await;
				}
				_ => {}
			}
		}
		for pin in versions {
			if pin != row.pinned_version {
				gone(self.store.destroy(&row.tenant, &pin).await)?;
			}
		}
		scope.commit().await
	}
	/// A durable intent is never replayed after an uncertain refresh. A receipt
	/// written before a database commit can instead finish that exact rotation.
	async fn recover(&self, scope: &mut dyn Scope, row: &mut OAuthConnection) -> Result<bool> {
		if self.calls.is_some() {
			return Ok(false);
		}
		let base = Intent {
			connection: row.id,
			revision: row.revision,
			pin: row.pinned_version.clone(),
		};
		let mut intent = false;
		let mut receipt = None;
		for pin in self
			.store
			.versions(&row.tenant, &row.secret_resource)
			.await?
		{
			if pin == row.pinned_version {
				continue;
			}
			match self.read(row, &pin).await? {
				Journal::Intent { refresh_intent } if refresh_intent == base => intent = true,
				Journal::Tokens(s) if s.replacement.as_ref() == Some(&base) => {
					receipt = Some((pin, s))
				}
				_ => {}
			}
		}
		let changed = receipt.is_some() || intent;
		if let Some((pin, s)) = receipt {
			row.pinned_version = pin;
			row.access_expires_at = s.expires_at;
			row.scopes = s.scope;
			row.updated_at = Utc::now();
			row.revision += 1;
			scope.save(row).await?;
		} else if intent {
			row.state = ConnectionState::NeedsReconnection;
			row.updated_at = Utc::now();
			row.revision += 1;
			scope.save(row).await?;
		}
		Ok(changed)
	}
	async fn active_scope(
		&self,
		owner: &Owner,
		resource: &str,
	) -> Result<(Box<dyn Scope>, OAuthConnection)> {
		loop {
			let mut scope = self.repository.begin(owner).await?;
			let mut row = scope
				.list()
				.await?
				.into_iter()
				.find(|r| r.resource == resource)
				.ok_or_else(required)?;
			if row.state != ConnectionState::Active {
				return Err(required());
			}
			let recovered = self.recover(scope.as_mut(), &mut row).await?;
			if recovered {
				scope.commit().await?;
				continue;
			}
			return Ok((scope, row));
		}
	}
	/// Forces the next call to refresh after a resource rejects its bearer.
	pub async fn invalidate(&self, owner: &Owner, resource: &str) -> Result<()> {
		let resource = domain::canonical_resource(resource)?;
		let (mut scope, mut row) = self.active_scope(owner, &resource).await?;
		row.access_expires_at = Some(Utc::now());
		row.updated_at = Utc::now();
		row.revision += 1;
		scope.save(&row).await?;
		scope.commit().await
	}
	pub async fn bearer(&self, owner: &Owner, resource: &str) -> Result<Bearer> {
		self.bearer_inner(owner, resource)
			.await
			.map_err(|e| match e {
				Error::Invalid(ref code) if code == domain::CONNECTION_REQUIRED => e,
				_ => unavailable(),
			})
	}
	async fn bearer_inner(&self, owner: &Owner, resource: &str) -> Result<Bearer> {
		if self.calls.is_some() {
			return Err(unavailable());
		}
		let resource = domain::canonical_resource(resource)?;
		let (mut scope, mut row) = self.active_scope(owner, &resource).await?;
		let Journal::Tokens(old) = self.read(&row, &row.pinned_version).await? else {
			return Err(unavailable());
		};
		if row
			.access_expires_at
			.is_none_or(|t| t > Utc::now() + Duration::seconds(60))
		{
			scope.commit().await?;
			self.prune(&row).await?;
			return Ok(Bearer {
				connection: row.id,
				access_token: old.access_token.into(),
			});
		}
		let Some(refresh) = old.refresh_token.as_ref() else {
			row.state = ConnectionState::NeedsReconnection;
			row.updated_at = Utc::now();
			row.revision += 1;
			scope.save(&row).await?;
			scope.commit().await?;
			return Err(required());
		};
		let server = self
			.discovery
			.authorization_server(&row.issuer)
			.await
			.map_err(|_| unavailable())?;
		// The callback URI is persisted with the encrypted token payload's client cache key.
		let redirect = self.redirect_for(&row).await?;
		let client = self
			.credentials(owner, &row.issuer, &row.client, &redirect)
			.await?;
		let intent = Intent {
			connection: row.id,
			revision: row.revision,
			pin: row.pinned_version.clone(),
		};
		let intent_pin = self
			.store
			.add_version(
				&row.tenant,
				&row.secret_resource,
				&seal(&Journal::Intent {
					refresh_intent: intent.clone(),
				})?,
			)
			.await?;
		let issued = match self
			.tokens
			.refresh(
				&server,
				&client,
				&SecretString::from(refresh.clone()),
				&row.resource,
			)
			.await
		{
			Ok(v) => v,
			Err(RefreshFailure::Unavailable(_)) => {
				gone(self.store.destroy(&row.tenant, &intent_pin).await)?;
				scope.commit().await?;
				return Err(unavailable());
			}
			Err(RefreshFailure::Terminal) => {
				row.state = ConnectionState::NeedsReconnection;
				row.updated_at = Utc::now();
				row.revision += 1;
				scope.save(&row).await?;
				scope.commit().await?;
				return Err(required());
			}
			Err(RefreshFailure::Ambiguous(_)) => {
				scope.commit().await?;
				return Err(unavailable());
			}
		};
		let stored = match stored(&issued, Some(&old), &row.scopes, Some(intent)) {
			Ok(stored) => stored,
			Err(error) => {
				let _ = self
					.tokens
					.revoke(
						&server,
						&client,
						issued
							.refresh_token
							.as_ref()
							.unwrap_or(&issued.access_token),
					)
					.await;
				return Err(error);
			}
		};
		let pin = match self
			.store
			.add_version(&row.tenant, &row.secret_resource, &seal(&stored)?)
			.await
		{
			Ok(pin) => pin,
			Err(error) => {
				let _ = self
					.tokens
					.revoke(
						&server,
						&client,
						issued
							.refresh_token
							.as_ref()
							.unwrap_or(&issued.access_token),
					)
					.await;
				return Err(error);
			}
		};
		row.pinned_version = pin;
		row.access_expires_at = stored.expires_at;
		row.scopes = stored.scope.clone();
		row.updated_at = Utc::now();
		row.revision += 1;
		scope.save(&row).await?;
		scope.commit().await?;
		self.prune(&row).await?;
		Ok(Bearer {
			connection: row.id,
			access_token: stored.access_token.into(),
		})
	}
	async fn redirect_for(&self, row: &OAuthConnection) -> Result<String> {
		// Dynamic caches may contain multiple redirect URIs; the exact grant key is
		// kept in its private Store envelope, alongside tokens.
		let secret = self
			.reader()?
			.read(&row.tenant, &row.secret_resource, &row.pinned_version)
			.await?;
		let value: serde_json::Value = serde_json::from_str(secret.expose_secret())?;
		value["redirect_uri"]
			.as_str()
			.map(str::to_owned)
			.ok_or_else(unavailable)
	}
	pub async fn disconnect(&self, owner: &Owner, id: Uuid) -> Result<Disconnection> {
		let mut scope = self.repository.begin(owner).await?;
		let mut row = scope
			.list()
			.await?
			.into_iter()
			.find(|r| r.id == id)
			.ok_or_else(|| Error::NotFound("OAuth Connection".into()))?;
		scope.cancel_attempts(Some(&row.resource)).await?;
		// Commit access closure before deleting anything outside this transaction.
		row.state = ConnectionState::NeedsReconnection;
		row.updated_at = Utc::now();
		row.revision += 1;
		scope.save(&row).await?;
		scope.commit().await?;
		let mut scope = self.repository.begin(owner).await?;
		if !scope
			.list()
			.await?
			.iter()
			.any(|r| r.id == id && r.revision == row.revision)
		{
			return Err(Error::Conflict(
				"connection changed while disconnecting".into(),
			));
		}
		if let Some(calls) = &self.calls {
			let mut revoked = true;
			for pin in self
				.store
				.versions(&row.tenant, &row.secret_resource)
				.await?
			{
				if calls
					.revoke(&row.tenant, row.id, numeric_pin(&pin)?)
					.await
					.is_err()
				{
					revoked = false
				}
				gone(self.store.destroy(&row.tenant, &pin).await)?;
			}
			gone(self.store.delete(&row.tenant, &row.secret_resource).await)?;
			scope.delete(id).await?;
			scope.commit().await?;
			return Ok(Disconnection { revoked });
		}
		let mut revoked = true;
		let server = self.discovery.authorization_server(&row.issuer).await;
		let redirect = self.redirect_for(&row).await;
		let client = match redirect {
			Ok(ref r) => self.credentials(owner, &row.issuer, &row.client, r).await,
			Err(_) => Err(unavailable()),
		};
		for pin in self
			.store
			.versions(&row.tenant, &row.secret_resource)
			.await?
		{
			match self.read(&row, &pin).await {
				Ok(Journal::Tokens(t)) => {
					let token = SecretString::from(t.refresh_token.unwrap_or(t.access_token));
					match (&server, &client) {
						(Ok(s), Ok(c)) => {
							if self.tokens.revoke(s, c, &token).await.is_err() {
								revoked = false;
							}
						}
						_ => revoked = false,
					}
				}
				Ok(Journal::Intent { .. }) => {}
				Err(_) => revoked = false,
			}
			gone(self.store.destroy(&row.tenant, &pin).await)?;
		}
		gone(self.store.delete(&row.tenant, &row.secret_resource).await)?;
		scope.delete(id).await?;
		scope.commit().await?;
		Ok(Disconnection { revoked })
	}
	pub async fn force_disconnect(&self, owner: &Owner, id: Uuid) -> Result<Disconnection> {
		self.disconnect(owner, id).await
	}
	pub async fn retire_subject(&self, tenant: &str, subject: &str) -> Result<()> {
		let owner = Owner {
			tenant: tenant.into(),
			subject: subject.into(),
		};
		let mut scope = self.repository.begin(&owner).await?;
		scope.cancel_attempts(None).await?;
		let rows = scope.list().await?;
		scope.commit().await?;
		for row in rows {
			self.disconnect(&owner, row.id).await?;
		}
		Ok(())
	}
	pub async fn retire_tenant(&self, tenant: &str) -> Result<()> {
		for owner in self.repository.owners(tenant).await? {
			self.retire_subject(tenant, &owner.subject).await?;
		}
		for client in self.repository.dynamic_clients(tenant).await? {
			for pin in self.store.versions(tenant, &client.secret_resource).await? {
				gone(self.store.destroy(tenant, &pin).await)?;
			}
			gone(self.store.delete(tenant, &client.secret_resource).await)?;
			self.repository
				.delete_dynamic_client(tenant, client.id)
				.await?;
		}
		Ok(())
	}
}
fn numeric_pin(pin: &str) -> Result<&str> {
	let v = pin.rsplit('/').next().ok_or_else(unavailable)?;
	if !aidash_capability::connection::version_valid(v) {
		return Err(unavailable());
	}
	Ok(v)
}
fn apply_receipt(row: &mut OAuthConnection, receipt: &Receipt, requested: &[String]) -> Result<()> {
	if !aidash_capability::connection::version_valid(&receipt.version) {
		return Err(unavailable());
	}
	let scopes = receipt
		.scope
		.as_ref()
		.map(|s| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
		.unwrap_or_else(|| requested.to_vec());
	row.scopes = domain::admissible_scopes(&scopes, Some(requested), None, None)?;
	row.access_expires_at = receipt
		.expires_in
		.map(|n| {
			i64::try_from(n)
				.ok()
				.and_then(Duration::try_seconds)
				.and_then(|d| Utc::now().checked_add_signed(d))
				.ok_or_else(unavailable)
		})
		.transpose()?;
	row.pinned_version = format!("{}/versions/{}", row.secret_resource, receipt.version);
	row.updated_at = Utc::now();
	row.revision += 1;
	Ok(())
}

fn stored(
	tokens: &Tokens,
	old: Option<&Stored>,
	requested: &[String],
	replacement: Option<Intent>,
) -> Result<Stored> {
	if tokens.access_token.expose_secret().is_empty() {
		return Err(unavailable());
	}
	let expires_at = match tokens.expires_in {
		Some(n) => Some(
			Utc::now()
				.checked_add_signed(
					Duration::try_seconds(i64::try_from(n).map_err(|_| unavailable())?)
						.ok_or_else(unavailable)?,
				)
				.ok_or_else(unavailable)?,
		),
		None => None,
	};
	let scope = tokens
		.scope
		.as_ref()
		.map(|s| s.split_whitespace().map(str::to_owned).collect())
		.unwrap_or_else(|| requested.to_vec());
	let scope = domain::admissible_scopes(&scope, Some(requested), None, None)?;
	Ok(Stored {
		access_token: tokens.access_token.expose_secret().into(),
		refresh_token: tokens
			.refresh_token
			.as_ref()
			.map(|s| s.expose_secret().to_owned())
			.or_else(|| old.and_then(|o| o.refresh_token.clone())),
		expires_at,
		scope,
		redirect_uri: old.map(|o| o.redirect_uri.clone()).unwrap_or_default(),
		replacement,
	})
}

#[cfg(test)]
mod tests;
