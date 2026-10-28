//! Tenant Secret lifecycle (ADR 0051). A value is written to the Secret Store
//! and never returned; metadata carries only a Fingerprint, the last four
//! characters and the hosts the value may be sent to.
use crate::{Error, Result, provider_access::KeyMaterialReader, provider_credentials::Store};
use aidash_capability::tenant_secret::{Attach, Destination, SecretSubject};
use aidash_domain::tenant_secrets::{self, State, TenantSecret, UNAVAILABLE};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use std::{
	collections::HashMap,
	sync::{Arc, Mutex},
};
use tokio::time::Instant;
use url::Url;
use uuid::Uuid;

pub mod readiness;

/// Every mutating scope serializes one Tenant, including quota and name races.
#[async_trait]
pub trait Scope: Send {
	/// The Tenant's Tenant Secret of that name that is not deleted.
	async fn named(&mut self, name: &str) -> Result<Option<TenantSecret>>;
	/// A Tenant Secret in any state, for lifecycle work.
	async fn by_id(&mut self, id: Uuid) -> Result<Option<TenantSecret>>;
	async fn list(&mut self) -> Result<Vec<TenantSecret>>;
	async fn count(&mut self) -> Result<usize>;
	async fn insert(&mut self, value: &TenantSecret) -> Result<()>;
	/// Writes the row and appends the lifecycle event. The event carries no value.
	async fn save(&mut self, value: &TenantSecret, event: &str, actor: &str) -> Result<()>;
	/// Removes the row of a Tenant Secret whose Secret Store contents are gone.
	async fn remove(&mut self, id: Uuid) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait Repository: Send + Sync {
	async fn begin(&self, tenant: &str) -> Result<Box<dyn Scope>>;
	/// A bounded UUID keyset page of rows whose Secret Store contents may need
	/// cleanup: pending rows and deleted rows that still carry a pin.
	async fn reconciliation_candidates(
		&self,
		after: Option<Uuid>,
		limit: usize,
	) -> Result<Vec<TenantSecret>>;
	/// Every Tenant Secret of a Tenant, whatever its state, for its purge.
	async fn all(&self, tenant: &str) -> Result<Vec<TenantSecret>>;
}

/// One Tenant Secret call as the broker must authorize it. The Service fills
/// the Tenant Secret id and its pinned version from the current metadata.
#[derive(Clone)]
pub struct SecretCallGrant {
	pub tenant: String,
	pub secret: Uuid,
	/// The numeric version of the pinned Secret Store resource, as the broker
	/// reads `versions/{version}`.
	pub version: String,
	pub destination: Destination,
	pub methods: Vec<String>,
	pub attach: Attach,
	pub subject: SecretSubject,
	pub max_response_bytes: Option<u64>,
}

/// Where a brokered call goes and the token that authorizes it. `base` is the
/// broker's `{endpoint}/v1/forward`; the token is a Tenant Secret Capability
/// Token (ADR 0095).
#[derive(Clone)]
pub struct SecretRoute {
	pub base: Url,
	pub token: SecretString,
}

/// The Tenant Secret one call names, for a metadata-only record. It never
/// carries the value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretIdentity {
	pub id: Uuid,
	/// The numeric Secret Store version the broker reads.
	pub version: String,
}

/// A resolution and the Tenant Secret it used, so a transport can record the
/// id and version without a second metadata read.
pub struct Resolution {
	pub resolved: Resolved,
	pub secret: Option<SecretIdentity>,
}

/// How long a minted broker route is reused. The broker token lives 60 s, so
/// a route is re-minted before it expires even while an operation still runs.
pub const ROUTE_TTL: std::time::Duration = std::time::Duration::from_secs(50);

/// One broker route kept for less than its token's lifetime.
#[derive(Clone)]
pub struct CachedRoute {
	pub base: Url,
	pub token: SecretString,
	pub secret: SecretIdentity,
}

/// Short-lived broker routes keyed by operation and External service. One
/// gateway serves many requests at once, so lookups and inserts are safe
/// across them; a race mints a second token but never reuses a stale one.
#[derive(Default)]
pub struct RouteCache {
	entries: Mutex<HashMap<(Uuid, String), (CachedRoute, Instant)>>,
}
impl RouteCache {
	/// The route for this operation and service, unless it is 50 seconds old
	/// or more. Expired entries are dropped as they are seen.
	pub fn get(&self, operation: Uuid, service: &str) -> Option<CachedRoute> {
		let mut entries = self.entries.lock().expect("Tenant Secret route cache lock");
		entries.retain(|_, (_, since)| since.elapsed() < ROUTE_TTL);
		entries
			.get(&(operation, service.to_owned()))
			.map(|(route, _)| route.clone())
	}
	pub fn insert(&self, operation: Uuid, service: &str, route: CachedRoute) {
		let mut entries = self.entries.lock().expect("Tenant Secret route cache lock");
		entries.retain(|_, (_, since)| since.elapsed() < ROUTE_TTL);
		// A gateway is long-lived; keep one operation's routes from growing it.
		if entries.len() < 10_000 {
			entries.insert((operation, service.to_owned()), (route, Instant::now()));
		}
	}
}

/// The Tenant Secret broker of a Cloud or Dedicated Deployment. A
/// self-hosted Deployment configures none and reads the value instead.
#[async_trait]
pub trait SecretCalls: Send + Sync {
	async fn route(&self, grant: SecretCallGrant) -> Result<SecretRoute>;
}

/// What a transport knows about one call, before the Service looks the named
/// Tenant Secret up. It is only needed when a broker is configured.
#[derive(Clone)]
pub struct SecretCall {
	pub destination: Destination,
	pub methods: Vec<String>,
	pub attach: Attach,
	pub subject: SecretSubject,
	pub max_response_bytes: Option<u64>,
}

/// How a call's Tenant Secret was resolved. A brokered call never reads the
/// value: the transport sends it to [`SecretRoute::base`] with [`SecretRoute::token`].
pub enum Resolved {
	/// A self-hosted node reads the pinned version to make the call.
	Value(SecretString),
	/// A Cloud or Dedicated node forwards through the Tenant Secret broker.
	Brokered(SecretRoute),
}

/// The narrowest path prefix that still contains `endpoint`: the endpoint
/// path's directory, up to and including its last `/`. The broker signs this
/// prefix alone, and each call appends the remaining path to it. For
/// `https://api.example.com/v1/tools/run` the prefix is `/v1/tools/` and the
/// remainder is `run`; for `https://api.example.com/run` it is `/` and `run`.
fn endpoint_path_parts(endpoint: &str) -> Result<(String, String)> {
	let url = Url::parse(endpoint)
		.map_err(|_| Error::Invalid("invalid Tenant Secret endpoint URL".into()))?;
	let path = url.path();
	let cut = path.rfind('/').map_or(0, |index| index + 1);
	let prefix = if cut == 0 { "/" } else { &path[..cut] };
	Ok((prefix.into(), path[cut..].into()))
}

/// The directory of a URL path, up to and including its last `/`: what the
/// broker signs for an External service's base URL. `/v1/` stays `/v1/`,
/// and an endpoint-style `/v1/invoke` signs `/v1/` so the forwarded path keeps
/// `invoke` exactly, without a trailing slash.
pub fn path_directory(path: &str) -> &str {
	match path.rfind('/') {
		Some(index) => &path[..=index],
		None => "/",
	}
}

/// The destination a tool or test endpoint signs. `path_prefix` is the
/// endpoint path's directory, so the token never covers a sibling path.
pub fn endpoint_destination(endpoint: &str) -> Result<Destination> {
	let url = Url::parse(endpoint)
		.map_err(|_| Error::Invalid("invalid Tenant Secret endpoint URL".into()))?;
	if url.host_str().is_none() {
		return Err(Error::Invalid("invalid Tenant Secret endpoint URL".into()));
	}
	Ok(Destination {
		origin: url.origin().ascii_serialization(),
		path_prefix: endpoint_path_parts(endpoint)?.0,
	})
}

/// The part of the endpoint path outside its signed [`endpoint_destination`]
/// prefix. The transport appends it to the broker's `{endpoint}/v1/forward`.
pub fn forward_rest(endpoint: &str) -> Result<String> {
	Ok(endpoint_path_parts(endpoint)?.1)
}

/// What an invocation records about the Tenant Secret it carried. The value
/// itself is never recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRecord {
	pub id: Uuid,
	/// The pinned Secret Store version, as the broker reads it.
	pub version: String,
	pub host: String,
}

/// One call's Tenant Secret. A brokered credential can mint a new token for
/// the next request of a long-lived MCP session; a self-hosted one carries the
/// value read for this call alone.
pub enum CallSecret {
	Value {
		record: SecretRecord,
		value: SecretString,
	},
	Brokered {
		record: SecretRecord,
		source: Arc<dyn SecretRouteSource>,
		/// The endpoint path outside its signed prefix.
		rest: String,
	},
}
impl CallSecret {
	pub fn record(&self) -> &SecretRecord {
		match self {
			Self::Value { record, .. } | Self::Brokered { record, .. } => record,
		}
	}
}

/// Mints another route for the same call. A transport that keeps one MCP
/// session open past a token's lifetime calls this rather than reading a value
/// or calling the tool endpoint directly.
#[async_trait]
pub trait SecretRouteSource: Send + Sync {
	async fn route(&self) -> Result<SecretRoute>;
}

pub struct Service {
	pub repository: Arc<dyn Repository>,
	pub store: Arc<dyn Store>,
	/// Reads one pinned version, only on a self-hosted node; the value is read
	/// to make the call and goes nowhere else. `None` where the application
	/// host is write-only (ADR 0002).
	pub reader: Option<Arc<dyn KeyMaterialReader>>,
	/// Routes calls through the Tenant Secret broker on Cloud and Dedicated.
	/// `None` on self-hosted, where [`Service::reader`] reads instead.
	pub calls: Option<Arc<dyn SecretCalls>>,
	pub fingerprint_key: SecretString,
	pub max_per_tenant: usize,
}

/// What a caller may learn about a Tenant Secret.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
#[schemars(rename = "TenantSecretMetadata")]
pub struct Metadata {
	pub id: Uuid,
	pub tenant: String,
	pub name: String,
	pub hosts: Vec<String>,
	pub fingerprint: String,
	pub last4: String,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
	pub revision: i64,
}
impl From<TenantSecret> for Metadata {
	fn from(v: TenantSecret) -> Self {
		Self {
			id: v.id,
			tenant: v.tenant,
			name: v.name,
			hosts: v.hosts,
			fingerprint: v.fingerprint,
			last4: v.last4,
			created_at: v.created_at,
			updated_at: v.updated_at,
			revision: v.revision,
		}
	}
}

/// A store, replace or host change. `expected_revision` is 0 for a new name.
pub struct Put {
	pub expected_revision: i64,
	pub value: Option<SecretString>,
	pub hosts: Option<Vec<String>>,
}

pub const RECONCILIATION_BATCH_SIZE: usize = 25;
/// A Tenant Secret whose value never arrived expires after this long.
const PENDING_EXPIRY: Duration = Duration::minutes(5);

/// Each supervised pass advances even when one candidate's cleanup fails.
#[derive(Debug)]
pub struct Reconciliation {
	pub cleaned: usize,
	pub failed: usize,
	pub next: Option<Uuid>,
}

fn validate_value(value: &SecretString) -> Result<()> {
	let value = value.expose_secret();
	if value.is_empty()
		|| value.len() > 4096
		|| !value.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
		|| value.trim() != value
	{
		return Err(Error::Invalid(
			"a Tenant Secret value is 1 to 4096 printable ASCII characters without surrounding spaces".into(),
		));
	}
	Ok(())
}
fn unavailable() -> Error {
	Error::Invalid(UNAVAILABLE.into())
}

/// A capability token names `versions/{version}`, so only the numeric part of
/// the Store's pinned resource is a claim.
fn numeric_version(version: &str) -> Result<String> {
	let (_, number) = version
		.rsplit_once("/versions/")
		.ok_or_else(|| Error::Invalid("Tenant Secret has an invalid pinned version".into()))?;
	if number.is_empty() || number.starts_with('0') || !number.bytes().all(|b| b.is_ascii_digit()) {
		return Err(Error::Invalid(
			"Tenant Secret has an invalid pinned version".into(),
		));
	}
	Ok(number.into())
}

/// Registration, installation and test profiles refuse any definition that
/// would send a Tenant Secret to a host it does not record. `hosts` is what
/// the Tenant's active Tenant Secret `name` records, `None` if there is none.
pub fn admit(name: &str, endpoint: &str, hosts: Option<&[String]>) -> Result<()> {
	let hosts =
		hosts.ok_or_else(|| Error::Invalid(format!("Tenant Secret {name} does not exist")))?;
	if !tenant_secrets::permits(hosts, endpoint)? {
		return Err(Error::Invalid(format!(
			"Tenant Secret {name} does not record the host of {endpoint}"
		)));
	}
	Ok(())
}

impl Service {
	fn fingerprint(&self, tenant: &str, value: &SecretString) -> (String, String) {
		// Independent Tenant keys, and a domain separator of their own, so a
		// Tenant Secret never shares a Fingerprint with Key Material.
		let mut tenant_key =
			Hmac::<Sha256>::new_from_slice(self.fingerprint_key.expose_secret().as_bytes())
				.expect("HMAC accepts arbitrary keys");
		tenant_key.update(b"aidash-tenant-secret-fingerprint-v1\0");
		tenant_key.update(tenant.as_bytes());
		let tenant_key = tenant_key.finalize().into_bytes();
		let mut digest =
			Hmac::<Sha256>::new_from_slice(&tenant_key).expect("HMAC accepts derived key");
		digest.update(value.expose_secret().as_bytes());
		let bytes = digest.finalize().into_bytes();
		let fingerprint = bytes[..8].iter().map(|b| format!("{b:02x}")).collect();
		let last4 = value
			.expose_secret()
			.chars()
			.rev()
			.take(4)
			.collect::<Vec<_>>()
			.into_iter()
			.rev()
			.collect();
		(fingerprint, last4)
	}

	pub async fn list(&self, tenant: &str) -> Result<Vec<Metadata>> {
		let mut scope = self.repository.begin(tenant).await?;
		let rows = scope.list().await?;
		scope.commit().await?;
		Ok(rows.into_iter().map(Metadata::from).collect())
	}

	/// The Tenant's active Tenant Secrets, for readiness facts. A pending or
	/// deleted name is not usable, so it is left out.
	pub async fn active(&self, tenant: &str) -> Result<Vec<Metadata>> {
		let mut scope = self.repository.begin(tenant).await?;
		let rows = scope.list().await?;
		scope.commit().await?;
		Ok(rows
			.into_iter()
			.filter(|row| row.state == State::Active)
			.map(Metadata::from)
			.collect())
	}
	pub async fn get(&self, tenant: &str, name: &str) -> Result<Metadata> {
		tenant_secrets::validate_name(name)?;
		let mut scope = self.repository.begin(tenant).await?;
		let row = scope.named(name).await?;
		scope.commit().await?;
		row.map(Metadata::from)
			.ok_or_else(|| Error::NotFound("Tenant Secret".into()))
	}

	/// The hosts of an active Tenant Secret, for admission of the definitions
	/// that name it. `None` when it does not exist.
	pub async fn hosts(&self, tenant: &str, name: &str) -> Result<Option<Vec<String>>> {
		let mut scope = self.repository.begin(tenant).await?;
		let row = scope.named(name).await?;
		scope.commit().await?;
		Ok(row.filter(|r| r.state == State::Active).map(|r| r.hosts))
	}

	/// Stores a new Tenant Secret, or replaces the value and the hosts of an
	/// existing one. A replacement changes no definition version.
	pub async fn put(&self, tenant: &str, name: &str, input: Put, actor: &str) -> Result<Metadata> {
		tenant_secrets::validate_name(name)?;
		if let Some(value) = &input.value {
			validate_value(value)?;
		}
		let hosts = input
			.hosts
			.as_deref()
			.map(tenant_secrets::normalize_hosts)
			.transpose()?;
		let mut scope = self.repository.begin(tenant).await?;
		match scope.named(name).await? {
			None => {
				if input.expected_revision != 0 {
					return Err(Error::Conflict("stale Tenant Secret revision".into()));
				}
				let (Some(value), Some(hosts)) = (input.value, hosts) else {
					return Err(Error::Invalid(
						"a new Tenant Secret needs a value and its hosts".into(),
					));
				};
				if scope.count().await? >= self.max_per_tenant {
					return Err(Error::Conflict("Tenant Secret quota reached".into()));
				}
				self.create(tenant, name, value, hosts, scope, actor).await
			}
			Some(mut row) => {
				row.check_revision(input.expected_revision)?;
				let old = row.require_active()?.to_owned();
				if input.value.is_none() && hosts.is_none() {
					return Err(Error::Invalid(
						"nothing to change: send a value, hosts or both".into(),
					));
				}
				// ADR 0096: where the Store seals the hosts with the value, the
				// application host cannot attach new hosts to a value it cannot read.
				if self.store.seals_secret_hosts() && hosts.is_some() && input.value.is_none() {
					return Err(Error::Invalid(
						"changing a Tenant Secret's hosts requires its value on this Deployment"
							.into(),
					));
				}
				let mut added = None;
				if let Some(value) = &input.value {
					// A sealed Store also records the hosts, so a new value sent
					// without hosts keeps the current ones.
					let sealed_hosts = hosts.as_deref().unwrap_or(&row.hosts);
					let version = self
						.store
						.add_secret_version(tenant, &row.secret_resource, sealed_hosts, value)
						.await?;
					let (fingerprint, last4) = self.fingerprint(tenant, value);
					row.pinned_version = Some(version.clone());
					row.fingerprint = fingerprint;
					row.last4 = last4;
					added = Some(version);
				}
				if let Some(hosts) = hosts {
					row.hosts = hosts;
				}
				row.updated_at = Utc::now();
				row.revision += 1;
				let saved = scope.save(&row, "tenant_secret.updated", actor).await;
				let committed = match saved {
					Ok(()) => scope.commit().await,
					Err(error) => Err(error),
				};
				if let Err(error) = committed {
					if let Some(version) = &added {
						let _cleanup = self.store.disable(tenant, version).await;
					}
					return Err(error);
				}
				// The pin is durable; an old version that cannot be disabled now
				// is retried by reconciliation.
				if added.is_some() && self.store.disable(tenant, &old).await.is_err() {
					tracing::warn!("Tenant Secret replaced; old version cleanup is pending");
				}
				Ok(row.into())
			}
		}
	}

	async fn create(
		&self,
		tenant: &str,
		name: &str,
		value: SecretString,
		hosts: Vec<String>,
		mut scope: Box<dyn Scope>,
		actor: &str,
	) -> Result<Metadata> {
		let id = Uuid::now_v7();
		let (fingerprint, last4) = self.fingerprint(tenant, &value);
		let now = Utc::now();
		let mut row = TenantSecret {
			id,
			tenant: tenant.into(),
			name: name.into(),
			hosts,
			secret_resource: self.store.secret_resource(id),
			pinned_version: None,
			fingerprint,
			last4,
			state: State::Pending,
			created_at: now,
			updated_at: now,
			revision: 1,
		};
		scope.insert(&row).await?;
		scope.commit().await?;
		self.store.create_secret(tenant, id).await?;
		let version = self
			.store
			.add_secret_version(tenant, &row.secret_resource, &row.hosts, &value)
			.await?;
		let mut scope = self.repository.begin(tenant).await?;
		let pending = scope.named(name).await?;
		if pending.is_none_or(|p| p.id != id || p.state != State::Pending) {
			// Reconciliation may win while the Secret Store write is in flight.
			self.store.delete(tenant, &row.secret_resource).await?;
			return Err(Error::Conflict("Tenant Secret creation expired".into()));
		}
		row.pinned_version = Some(version);
		row.state = State::Active;
		row.revision += 1;
		scope.save(&row, "tenant_secret.created", actor).await?;
		scope.commit().await?;
		Ok(row.into())
	}

	/// Deletes a Tenant Secret: the calls that name it fail with
	/// `secret_unavailable` from the moment this commits, and its value is
	/// destroyed. Cleanup that fails here is retried by reconciliation.
	pub async fn delete(
		&self,
		tenant: &str,
		name: &str,
		expected_revision: i64,
		actor: &str,
	) -> Result<Metadata> {
		tenant_secrets::validate_name(name)?;
		let mut scope = self.repository.begin(tenant).await?;
		let mut row = scope
			.named(name)
			.await?
			.ok_or_else(|| Error::NotFound("Tenant Secret".into()))?;
		row.check_revision(expected_revision)?;
		if row.state != State::Active {
			return Err(Error::Conflict(
				"Tenant Secret cannot be deleted in its current state".into(),
			));
		}
		// Deleted is the durable, irreversible intent. The pin stays as the
		// cleanup marker until every Secret Store effect has succeeded.
		row.state = State::Deleted;
		row.updated_at = Utc::now();
		row.revision += 1;
		scope.save(&row, "tenant_secret.deleted", actor).await?;
		scope.commit().await?;
		if self.reconcile_candidate(row.clone()).await.is_err() {
			tracing::warn!("Tenant Secret deleted; Secret Store cleanup is pending");
		}
		Ok(row.into())
	}

	/// The active Tenant Secret a call names, after re-checking the endpoint's
	/// host. Missing, deleted, or no longer host-matching is `secret_unavailable`.
	async fn active_row(&self, tenant: &str, name: &str, endpoint: &str) -> Result<TenantSecret> {
		let mut scope = self.repository.begin(tenant).await?;
		let row = scope.named(name).await?;
		scope.commit().await?;
		let Some(row) = row.filter(|r| r.state == State::Active) else {
			return Err(unavailable());
		};
		if !tenant_secrets::permits(&row.hosts, endpoint).unwrap_or(false) {
			return Err(unavailable());
		}
		Ok(row)
	}

	/// Resolves the Tenant Secret one call names, or `secret_unavailable` when
	/// it is missing, deleted, or no longer records the endpoint's host. This
	/// is where dispatch re-checks what registration checked.
	///
	/// With a [`SecretCalls`] configured the value is never read: the Service
	/// re-checks the metadata and returns a [`Resolved::Brokered`] route. A
	/// transport that cannot broker must pass `None` and is refused. Without a
	/// broker, the pinned version is read and returned as [`Resolved::Value`].
	pub async fn resolve(
		&self,
		tenant: &str,
		name: &str,
		endpoint: &str,
		call: Option<SecretCall>,
	) -> Result<Resolved> {
		let row = self.active_row(tenant, name, endpoint).await?;
		self.resolve_row(tenant, &row, call).await
	}

	/// [`Service::resolve`] plus the Tenant Secret identity the resolution
	/// used, so a transport records its record without reading the value.
	pub async fn resolve_with_secret(
		&self,
		tenant: &str,
		name: &str,
		endpoint: &str,
		call: Option<SecretCall>,
	) -> Result<Resolution> {
		let row = self.active_row(tenant, name, endpoint).await?;
		let pinned = row.require_active()?.to_owned();
		let resolved = self.resolve_row(tenant, &row, call).await?;
		let version = match &resolved {
			Resolved::Brokered(_) => numeric_version(&pinned)?,
			// A self-hosted record names the same numeric pin the broker
			// would read; a pin this store cannot express stays as it is.
			Resolved::Value(_) => numeric_version(&pinned).unwrap_or(pinned),
		};
		Ok(Resolution {
			resolved,
			secret: Some(SecretIdentity {
				id: row.id,
				version,
			}),
		})
	}

	/// The resolution shared by [`Service::resolve`],
	/// [`Service::resolve_with_secret`] and [`Service::call_secret`]: the
	/// caller has already checked the Tenant Secret's state and the endpoint's
	/// host.
	async fn resolve_row(
		&self,
		tenant: &str,
		row: &TenantSecret,
		call: Option<SecretCall>,
	) -> Result<Resolved> {
		let pinned = row.require_active()?.to_owned();
		if let Some(calls) = &self.calls {
			let call = call.ok_or_else(|| {
				Error::Invalid("this call path cannot use the Tenant Secret broker".into())
			})?;
			// The token's destination is checked as well, so a broker never sees
			// a route the metadata does not record.
			if !tenant_secrets::permits(&row.hosts, &call.destination.origin).unwrap_or(false) {
				return Err(unavailable());
			}
			let route = calls
				.route(SecretCallGrant {
					tenant: tenant.into(),
					secret: row.id,
					version: numeric_version(&pinned)?,
					destination: call.destination,
					methods: call.methods,
					attach: call.attach,
					subject: call.subject,
					max_response_bytes: call.max_response_bytes,
				})
				.await?;
			return Ok(Resolved::Brokered(route));
		}
		let reader = self
			.reader
			.as_ref()
			.ok_or_else(|| Error::External("Secret Store is not readable".into()))?;
		match reader.read(tenant, &row.secret_resource, &pinned).await {
			Ok(value) => Ok(Resolved::Value(value)),
			// A rejected pin is permanent; a store outage stays retryable.
			Err(Error::Invalid(_)) => Err(unavailable()),
			Err(error) => Err(error),
		}
	}

	/// The direct value for a self-hosted transport. It refuses to read when a
	/// broker is configured, so a Cloud or Dedicated Deployment never falls
	/// back to the value (ADR 0095).
	pub async fn resolve_value(
		&self,
		tenant: &str,
		name: &str,
		endpoint: &str,
	) -> Result<SecretString> {
		match self.resolve(tenant, name, endpoint, None).await? {
			Resolved::Value(value) => Ok(value),
			Resolved::Brokered(_) => Err(Error::Invalid(
				"this call path cannot use the Tenant Secret broker".into(),
			)),
		}
	}

	/// One transport call's credential and the metadata its invocation records.
	///
	/// A self-hosted Deployment reads the pinned value once and returns it. A
	/// Cloud or Dedicated Deployment mints the first route now and returns a
	/// source that mints another for the next request of a long-lived MCP
	/// session; it never reads the value and never falls back to a direct call.
	pub async fn call_secret(
		self: &Arc<Self>,
		tenant: &str,
		name: &str,
		endpoint: &str,
		call: SecretCall,
	) -> Result<CallSecret> {
		let row = self.active_row(tenant, name, endpoint).await?;
		let pinned = row.require_active()?.to_owned();
		let record = SecretRecord {
			id: row.id,
			version: numeric_version(&pinned)?,
			host: tenant_secrets::endpoint_host(endpoint)?,
		};
		let recorded = SecretIdentity {
			id: record.id,
			version: record.version.clone(),
		};
		match self.resolve_row(tenant, &row, Some(call.clone())).await? {
			Resolved::Brokered(first) => Ok(CallSecret::Brokered {
				record,
				source: Arc::new(RefreshSource {
					service: Arc::downgrade(self),
					tenant: tenant.into(),
					name: name.into(),
					endpoint: endpoint.into(),
					call,
					recorded,
					first: Mutex::new(Some(first)),
				}),
				rest: forward_rest(endpoint)?,
			}),
			Resolved::Value(value) => Ok(CallSecret::Value { record, value }),
		}
	}

	/// Destroys every Tenant Secret of a Tenant and its Secret Store contents,
	/// for Tenant Deletion's purge.
	pub async fn purge_tenant(&self, tenant: &str) -> Result<usize> {
		let mut purged = 0;
		for row in self.repository.all(tenant).await? {
			self.destroy_contents(&row).await?;
			let mut scope = self.repository.begin(tenant).await?;
			scope.remove(row.id).await?;
			scope.commit().await?;
			purged += 1;
		}
		Ok(purged)
	}

	async fn destroy_contents(&self, row: &TenantSecret) -> Result<()> {
		for version in self
			.store
			.versions(&row.tenant, &row.secret_resource)
			.await?
		{
			self.store.destroy(&row.tenant, &version).await?;
		}
		self.store.delete(&row.tenant, &row.secret_resource).await
	}

	pub async fn reconcile(&self) -> Result<usize> {
		let result = self.reconcile_page(None).await?;
		if result.failed != 0 {
			return Err(Error::External("Tenant Secret cleanup is pending".into()));
		}
		Ok(result.cleaned)
	}
	/// Runs one bounded page. Callers keep `next` and delay before the next
	/// pass, so one outage cannot starve later Tenants.
	pub async fn reconcile_page(&self, after: Option<Uuid>) -> Result<Reconciliation> {
		let candidates = self
			.repository
			.reconciliation_candidates(after, RECONCILIATION_BATCH_SIZE)
			.await?;
		let next = if candidates.len() == RECONCILIATION_BATCH_SIZE {
			candidates.last().map(|row| row.id)
		} else {
			None
		};
		let mut result = Reconciliation {
			cleaned: 0,
			failed: 0,
			next,
		};
		for candidate in candidates {
			match self.reconcile_candidate(candidate).await {
				Ok(true) => result.cleaned += 1,
				Ok(false) => {}
				Err(_) => result.failed += 1,
			}
		}
		Ok(result)
	}
	async fn reconcile_candidate(&self, candidate: TenantSecret) -> Result<bool> {
		let mut scope = self.repository.begin(&candidate.tenant).await?;
		let Some(mut row) = scope.by_id(candidate.id).await? else {
			return Ok(false);
		};
		let mut cleaned = false;
		match row.state {
			State::Pending if row.created_at <= Utc::now() - PENDING_EXPIRY => {
				// Deleting is idempotent across crashes before and after the write.
				self.store.delete(&row.tenant, &row.secret_resource).await?;
				row.state = State::Deleted;
				row.revision += 1;
				scope
					.save(&row, "tenant_secret.deleted", "tenant-secret-reconciler")
					.await?;
				cleaned = true;
			}
			State::Active => {
				let pinned = row.require_active()?.to_owned();
				for version in self
					.store
					.versions(&row.tenant, &row.secret_resource)
					.await?
				{
					if version != pinned {
						self.store.disable(&row.tenant, &version).await?;
					}
				}
			}
			State::Deleted if row.pinned_version.is_some() => {
				self.destroy_contents(&row).await?;
				row.pinned_version = None;
				scope
					.save(
						&row,
						"tenant_secret.cleanup_completed",
						"tenant-secret-reconciler",
					)
					.await?;
				cleaned = true;
			}
			State::Pending | State::Deleted => {}
		}
		scope.commit().await?;
		Ok(cleaned)
	}
}

/// Re-routes one call through the broker after a token's lifetime. It keeps a
/// weak Service so a finished request does not pin the Secret Store.
struct RefreshSource {
	service: std::sync::Weak<Service>,
	tenant: String,
	name: String,
	endpoint: String,
	call: SecretCall,
	/// The Tenant Secret and version the invocation recorded. A refresh mints
	/// only for these, so every request matches the record.
	recorded: SecretIdentity,
	/// The route minted when the call started, used before the first refresh.
	first: Mutex<Option<SecretRoute>>,
}

#[async_trait]
impl SecretRouteSource for RefreshSource {
	async fn route(&self) -> Result<SecretRoute> {
		if let Some(first) = self.first.lock().unwrap().take() {
			return Ok(first);
		}
		let service = self.service.upgrade().ok_or_else(|| {
			Error::External("Tenant Secret broker routing is no longer available".into())
		})?;
		let row = service
			.active_row(&self.tenant, &self.name, &self.endpoint)
			.await?;
		// A session that outlives a rotation, or a name deleted and created
		// again, stops rather than carry a value its invocation does not
		// record; the next call records the current one (ADR 0095).
		if row.id != self.recorded.id
			|| numeric_version(row.require_active()?)? != self.recorded.version
		{
			return Err(unavailable());
		}
		match service
			.resolve_row(&self.tenant, &row, Some(self.call.clone()))
			.await?
		{
			Resolved::Brokered(route) => Ok(route),
			Resolved::Value(_) => Err(Error::Invalid(
				"this call path cannot use the Tenant Secret broker".into(),
			)),
		}
	}
}

#[cfg(test)]
mod tests;
