//! Provider Credential lifecycle; all Key Material stays in write-only ports.
use crate::{Error, Result, provider_authorization::Target};
use aidash_domain::provider_credentials::{Binding, Provider, ProviderCredential, State};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Validation {
	pub warnings: Vec<String>,
}
#[async_trait]
pub trait KeyValidator: Send + Sync {
	async fn validate(&self, provider: Provider, key: &SecretString) -> Result<Validation>;
}
/// The write port of the Secret Store: Key Material, Plan Tokens and Tenant
/// Secret values. Deliberately no access/read-payload operation, even for the
/// fake adapter.
#[async_trait]
pub trait Store: Send + Sync {
	fn resource(&self, id: Uuid) -> String;
	async fn create(&self, tenant: &str, id: Uuid) -> Result<()>;
	async fn add_version(&self, tenant: &str, resource: &str, key: &SecretString)
	-> Result<String>;
	/// Every operation names the Tenant whose Key Material it touches, so a
	/// store inside the database works in that Tenant's scope.
	async fn versions(&self, tenant: &str, resource: &str) -> Result<Vec<String>>;
	async fn disable(&self, tenant: &str, version: &str) -> Result<()>;
	async fn destroy(&self, tenant: &str, version: &str) -> Result<()>;
	async fn delete(&self, tenant: &str, resource: &str) -> Result<()>;
	/// Whether this Store implements OAuth Client secret storage.
	fn supports_oauth_client_secrets(&self) -> bool {
		false
	}
	/// The resource holding one registered OAuth Client secret.
	fn oauth_client_resource(&self, id: Uuid) -> String {
		format!("aidash-store/oauth-clients/{id}")
	}
	/// OAuth Client secrets are unavailable in stores that have not implemented
	/// their namespace, including the enterprise Secret Manager adapter.
	/// Implementations must preserve an existing resource and version counter
	/// when a replacement or interrupted write retries this operation.
	async fn create_oauth_client(&self, _tenant: &str, _id: Uuid) -> Result<()> {
		Err(Error::OAuthClientSecretsUnavailable)
	}
	/// The resource holding one Plan Connection's Plan Tokens (ADR 0008).
	fn plan_resource(&self, id: Uuid) -> String {
		format!("aidash-store/plan-connections/{id}")
	}
	/// Creates one Plan Connection's resource. A Store that cannot hold Plan
	/// Tokens, such as Cloud's Secret Manager until #248, refuses.
	async fn create_plan(&self, _tenant: &str, _id: Uuid) -> Result<()> {
		Err(Error::Invalid(
			"this Secret Store does not hold Plan Tokens".into(),
		))
	}
	/// The resource holding one Tenant Secret's values, under its own
	/// namespace (ADR 0051).
	fn secret_resource(&self, id: Uuid) -> String {
		format!("aidash-store/tenant-secrets/{id}")
	}
	/// Whether one Tenant Secret version seals the value together with the
	/// hosts it may be sent to (ADR 0096). A Store that does needs the value
	/// again to change them; a Store that does not keeps hosts in PostgreSQL.
	fn seals_secret_hosts(&self) -> bool {
		false
	}
	/// Creates one Tenant Secret's resource. A Store that cannot hold Tenant
	/// Secrets refuses.
	async fn create_secret(&self, _tenant: &str, _id: Uuid) -> Result<()> {
		Err(Error::Invalid(
			"this Secret Store does not hold Tenant Secrets".into(),
		))
	}
	/// Writes one Tenant Secret version. A Store that seals hosts with the
	/// value uses them; a Store that does not ignores them and calls
	/// [`Store::add_version`].
	async fn add_secret_version(
		&self,
		tenant: &str,
		resource: &str,
		_hosts: &[String],
		value: &SecretString,
	) -> Result<String> {
		self.add_version(tenant, resource, value).await
	}
}
/// Every mutating scope serializes one Tenant, including quota and binding races.
#[async_trait]
pub trait Scope: Send {
	async fn get(&mut self, id: Uuid) -> Result<ProviderCredential>;
	async fn list(&mut self, offset: usize, limit: usize) -> Result<Vec<ProviderCredential>>;
	async fn count(&mut self) -> Result<usize>;
	async fn insert(&mut self, value: &ProviderCredential) -> Result<()>;
	async fn save(&mut self, value: &ProviderCredential, event: &str, actor: &str) -> Result<()>;
	async fn bindings(&mut self) -> Result<Vec<Binding>>;
	async fn bind(&mut self, value: &Binding, actor: &str) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait Repository: Send + Sync {
	async fn begin(&self, tenant: &str) -> Result<Box<dyn Scope>>;
	/// A bounded UUID keyset page of live metadata and deleted tombstones whose
	/// private version pin still marks unfinished cleanup. PostgreSQL is the
	/// cleanup inventory; Secret Manager secrets are never listed.
	async fn reconciliation_candidates(
		&self,
		after: Option<Uuid>,
		limit: usize,
	) -> Result<Vec<ProviderCredential>>;
}
/// Provider Credentials purged per repository transaction.
const PURGE_PAGE_SIZE: usize = 50;
pub struct Service {
	pub repository: Arc<dyn Repository>,
	pub store: Arc<dyn Store>,
	pub validator: Arc<dyn KeyValidator>,
	/// Provider Authorization code exchange; the only production source of Key Material.
	pub exchange: Arc<dyn crate::provider_authorization::Exchange>,
	pub fingerprint_key: SecretString,
	pub max_per_tenant: usize,
}
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
#[schemars(rename = "ProviderCredentialMetadata")]
pub struct Metadata {
	pub id: Uuid,
	pub tenant: String,
	pub provider: Provider,
	pub base_url: String,
	pub fingerprint: String,
	pub last4: String,
	/// The provider's owner-only settings page for the issued key. Revocation
	/// destroys only Aidash's copy, so users delete the key there.
	pub key_settings_url: Option<String>,
	pub state: State,
	pub created_at: chrono::DateTime<Utc>,
	pub rotated_at: Option<chrono::DateTime<Utc>>,
	pub revoked_at: Option<chrono::DateTime<Utc>>,
	pub revision: i64,
}
impl From<ProviderCredential> for Metadata {
	fn from(v: ProviderCredential) -> Self {
		Self {
			id: v.id,
			tenant: v.tenant,
			provider: v.provider,
			base_url: v.base_url,
			fingerprint: v.fingerprint,
			last4: v.last4,
			key_settings_url: v
				.key_digest
				.as_deref()
				.and_then(|digest| v.provider.key_settings_url(digest)),
			state: v.state,
			created_at: v.created_at,
			rotated_at: v.rotated_at,
			revoked_at: v.revoked_at,
			revision: v.revision,
		}
	}
}
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
#[schemars(rename = "ValidatedProviderCredential")]
pub struct Validated {
	pub provider_credential: Metadata,
	pub warnings: Vec<String>,
	/// After a rotation, the owner-only settings page of the key it replaced.
	/// Rotation disables only Aidash's copy; a provider-issued key stays live
	/// until the user deletes it there, and this is the last place its link
	/// appears once the new digest is recorded.
	pub previous_key_settings_url: Option<String>,
}
/// The rotation warning about Aidash's own Store cleanup; the reconciler
/// retries it, and it says nothing about the provider's key.
const ROTATION_CLEANUP_PENDING: &str =
	"Provider Credential rotation committed; previous version cleanup is pending";
impl Validated {
	/// Warnings the provider's validation reported about the key itself, such
	/// as a missing spending limit.
	pub fn key_warnings(&self) -> impl Iterator<Item = &str> {
		self.warnings
			.iter()
			.map(String::as_str)
			.filter(|warning| *warning != ROTATION_CLEANUP_PENDING)
	}
}
/// A Provider Authorization that ended without storing a key. Once the
/// exchange succeeds the provider has already issued a live key, so a later
/// failure carries that key's owner-only settings page: the user's only way to
/// delete it.
#[derive(Debug)]
pub struct AuthorizationFailure<E = Error> {
	pub error: E,
	pub key_settings_url: Option<String>,
}
impl<E> From<E> for AuthorizationFailure<E> {
	fn from(error: E) -> Self {
		Self {
			error,
			key_settings_url: None,
		}
	}
}
/// Each supervised pass advances even when one candidate's external cleanup fails.
#[derive(Debug)]
pub struct Reconciliation {
	pub cleaned: usize,
	pub failed: usize,
	pub next: Option<Uuid>,
}
pub const RECONCILIATION_BATCH_SIZE: usize = 25;

impl Service {
	fn validate_key(key: &SecretString) -> Result<()> {
		if !(8..=4096).contains(&key.expose_secret().len())
			|| !key.expose_secret().bytes().all(|b| b.is_ascii_graphic())
		{
			return Err(Error::Invalid(
				"invalid Provider Credential Key Material".into(),
			));
		}
		Ok(())
	}
	fn fingerprint(&self, tenant: &str, key: &SecretString) -> (String, String) {
		// Derive independent Tenant keys; identical Key Material has different fingerprints.
		let mut tenant_key =
			Hmac::<Sha256>::new_from_slice(self.fingerprint_key.expose_secret().as_bytes())
				.expect("HMAC accepts arbitrary keys");
		tenant_key.update(b"aidash-provider-credential-fingerprint-v1\0");
		tenant_key.update(tenant.as_bytes());
		let tenant_key = tenant_key.finalize().into_bytes();
		let mut digest =
			Hmac::<Sha256>::new_from_slice(&tenant_key).expect("HMAC accepts derived key");
		digest.update(key.expose_secret().as_bytes());
		let bytes = digest.finalize().into_bytes();
		let fingerprint = bytes[..8].iter().map(|b| format!("{b:02x}")).collect();
		let last4 = key
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
	/// Completes one Provider Authorization: exchanges the code for Key Material
	/// and writes it as a new Provider Credential or, on reconnect, as a new
	/// version. A rejected code or verifier stores nothing; a failure after the
	/// exchange reports the issued key's settings page, and a reconnect reports
	/// the settings page of the key it replaced.
	pub async fn authorize(
		&self,
		tenant: &str,
		provider: Provider,
		target: Target,
		code: &str,
		verifier: &SecretString,
		actor: &str,
	) -> std::result::Result<Validated, AuthorizationFailure> {
		Provider::credential(provider.id()).map_err(Error::from)?;
		self.check_authorization(tenant, provider, target).await?;
		let key = self.exchange.exchange(provider, code, verifier).await?;
		let digest = crate::provider_authorization::key_digest(&key);
		let key_settings_url = provider.key_settings_url(&digest);
		let written = match target {
			Target::Connect { id } => {
				self.write_new(tenant, id, provider, key, Some(digest), actor)
					.await
			}
			Target::Reconnect {
				id,
				expected_revision,
			} => {
				self.write_rotation(tenant, id, expected_revision, key, Some(digest), actor)
					.await
			}
		};
		written.map_err(|error| AuthorizationFailure {
			error,
			key_settings_url,
		})
	}
	/// Checks what the write will check before the provider issues a key, so a
	/// predictable refusal does not leave an unused key behind.
	async fn check_authorization(
		&self,
		tenant: &str,
		provider: Provider,
		target: Target,
	) -> Result<()> {
		let mut scope = self.repository.begin(tenant).await?;
		match target {
			Target::Connect { .. } => {
				if scope.count().await? >= self.max_per_tenant {
					return Err(Error::Conflict("Provider Credential quota reached".into()));
				}
			}
			Target::Reconnect {
				id,
				expected_revision,
			} => {
				let row = scope.get(id).await?;
				row.check_revision(expected_revision)?;
				row.require_active()?;
				if row.provider != provider {
					return Err(Error::Invalid(
						"Provider Authorization does not match the Provider Credential".into(),
					));
				}
			}
		}
		scope.commit().await
	}
	async fn write_new(
		&self,
		tenant: &str,
		id: Uuid,
		provider: Provider,
		key: SecretString,
		key_digest: Option<String>,
		actor: &str,
	) -> Result<Validated> {
		Self::validate_key(&key)?;
		if id.get_version_num() != 7 {
			return Err(Error::Invalid(
				"Provider Credential ID must be UUIDv7".into(),
			));
		}
		let (fingerprint, last4) = self.fingerprint(tenant, &key);
		let mut row = ProviderCredential {
			id,
			tenant: tenant.into(),
			provider,
			base_url: provider.base_url().into(),
			secret_resource: self.store.resource(id),
			pinned_version: None,
			fingerprint,
			last4,
			key_digest,
			state: State::Pending,
			created_at: Utc::now(),
			rotated_at: None,
			revoked_at: None,
			revision: 1,
		};
		let mut scope = self.repository.begin(tenant).await?;
		if scope.count().await? >= self.max_per_tenant {
			return Err(Error::Conflict("Provider Credential quota reached".into()));
		}
		scope.insert(&row).await?;
		scope.commit().await?;
		let validation = match self.validator.validate(provider, &key).await {
			Ok(v) => v,
			Err(e) => {
				let mut scope = self.repository.begin(tenant).await?;
				row.state = State::Deleted;
				row.revision += 1;
				scope
					.save(&row, "provider_credential.deleted", actor)
					.await?;
				scope.commit().await?;
				return Err(e);
			}
		};
		self.store.create(tenant, id).await?;
		let version = self
			.store
			.add_version(tenant, &row.secret_resource, &key)
			.await?;
		let mut scope = self.repository.begin(tenant).await?;
		let pending = scope.get(id).await?;
		if pending.state != State::Pending {
			// Reconciliation may win while an external create is in flight.
			self.store.delete(&row.tenant, &row.secret_resource).await?;
			return Err(Error::Conflict(
				"Provider Credential creation expired".into(),
			));
		}
		row.pinned_version = Some(version);
		row.state = State::Active;
		row.revision += 1;
		scope
			.save(&row, "provider_credential.created", actor)
			.await?;
		scope.commit().await?;
		Ok(Validated {
			provider_credential: row.into(),
			warnings: validation.warnings,
			previous_key_settings_url: None,
		})
	}
	async fn write_rotation(
		&self,
		tenant: &str,
		id: Uuid,
		expected: i64,
		key: SecretString,
		key_digest: Option<String>,
		actor: &str,
	) -> Result<Validated> {
		Self::validate_key(&key)?;
		let mut scope = self.repository.begin(tenant).await?;
		let mut row = scope.get(id).await?;
		row.check_revision(expected)?;
		let old = row.require_active()?.to_owned();
		let validation = self.validator.validate(row.provider, &key).await?;
		let version = self
			.store
			.add_version(tenant, &row.secret_resource, &key)
			.await?;
		let (fingerprint, last4) = self.fingerprint(tenant, &key);
		let previous_key_settings_url = row
			.key_digest
			.as_deref()
			.filter(|previous| key_digest.as_deref() != Some(*previous))
			.and_then(|previous| row.provider.key_settings_url(previous));
		row.pinned_version = Some(version.clone());
		row.fingerprint = fingerprint;
		row.last4 = last4;
		row.key_digest = key_digest;
		row.rotated_at = Some(Utc::now());
		row.revision += 1;
		scope
			.save(&row, "provider_credential.rotated", actor)
			.await?;
		if let Err(error) = scope.commit().await {
			let _cleanup = self.store.disable(&row.tenant, &version).await;
			return Err(error);
		}
		// The pin and revision are already committed. PostgreSQL's active row
		// remains the durable inventory for retrying every unpinned version, so
		// report cleanup separately instead of turning a successful rotation into
		// an error that the caller cannot safely retry at its original revision.
		let mut warnings = validation.warnings;
		if self.store.disable(&row.tenant, &old).await.is_err() {
			warnings.push(ROTATION_CLEANUP_PENDING.into());
		}
		Ok(Validated {
			provider_credential: row.into(),
			warnings,
			previous_key_settings_url,
		})
	}
	pub async fn revoke(
		&self,
		tenant: &str,
		id: Uuid,
		expected: i64,
		actor: &str,
	) -> Result<Metadata> {
		let mut scope = self.repository.begin(tenant).await?;
		let mut row = scope.get(id).await?;
		row.check_revision(expected)?;
		row.require_active()?;
		// Close effective access durably before external effects. A failed commit
		// leaves both the active pin and its versions usable; a crash afterwards
		// leaves revoked metadata as the inventory for retrying every disable.
		row.state = State::Revoked;
		row.revoked_at = Some(Utc::now());
		row.revision += 1;
		scope
			.save(&row, "provider_credential.revoked", actor)
			.await?;
		scope.commit().await?;
		if self.reconcile_candidate(row.clone()).await.is_err() {
			tracing::warn!("Provider Credential revocation committed; version cleanup is pending");
		}
		Ok(row.into())
	}
	pub async fn delete(
		&self,
		tenant: &str,
		id: Uuid,
		expected: i64,
		actor: &str,
	) -> Result<Metadata> {
		let mut scope = self.repository.begin(tenant).await?;
		let mut row = scope.get(id).await?;
		row.check_revision(expected)?;
		if row.state == State::Deleted || row.state == State::Pending {
			return Err(Error::Conflict(
				"Provider Credential cannot be deleted in its current state".into(),
			));
		}
		if scope
			.bindings()
			.await?
			.iter()
			.any(|b| b.provider_credential_id == Some(id))
		{
			return Err(Error::Conflict("Provider Credential is bound".into()));
		}
		// Deleted is the durable, irreversible intent. Retain the private pin as
		// a cleanup marker until every external effect has succeeded, so a crash
		// or partial destruction never leaves apparently usable metadata behind.
		row.state = State::Deleted;
		row.revision += 1;
		scope
			.save(&row, "provider_credential.deleted", actor)
			.await?;
		scope.commit().await?;
		if self.reconcile_candidate(row.clone()).await.is_err() {
			tracing::warn!("Provider Credential deletion committed; secret cleanup is pending");
		}
		Ok(row.into())
	}
	/// Ends every Provider Credential of a Tenant being purged, and with it
	/// the Key Material stored outside the database (ADR 0061). Unlike
	/// [`Self::delete`] it ignores Bindings, which are purged with the Tenant,
	/// and it also ends Pending and Revoked credentials. It fails when any
	/// external secret could not be destroyed, so the caller retries; each
	/// credential's durable `Deleted` mark and version pin let the retry resume.
	pub async fn purge_tenant(&self, tenant: &str, actor: &str) -> Result<()> {
		let mut offset = 0;
		loop {
			let mut scope = self.repository.begin(tenant).await?;
			let page = scope.list(offset, PURGE_PAGE_SIZE).await?;
			scope.commit().await?;
			let exhausted = page.len() < PURGE_PAGE_SIZE;
			offset += page.len();
			for row in page {
				self.purge_credential(row, actor).await?;
			}
			if exhausted {
				return Ok(());
			}
		}
	}
	async fn purge_credential(&self, mut row: ProviderCredential, actor: &str) -> Result<()> {
		if row.state == State::Pending {
			// No version is pinned yet; deleting the resource is idempotent
			// across a crash before or after the secret exists.
			self.store.delete(&row.tenant, &row.secret_resource).await?;
		}
		if row.state != State::Deleted {
			// Deleted is the durable intent; the pin stays as the cleanup marker.
			let mut scope = self.repository.begin(&row.tenant).await?;
			row = scope.get(row.id).await?;
			row.state = State::Deleted;
			row.revision += 1;
			scope
				.save(&row, "provider_credential.deleted", actor)
				.await?;
			scope.commit().await?;
		}
		if row.pinned_version.is_some() {
			self.reconcile_candidate(row).await?;
		}
		Ok(())
	}
	pub async fn bind(
		&self,
		tenant: &str,
		provider: Provider,
		id: impl Into<Option<Uuid>>,
		expected: i64,
		actor: &str,
	) -> Result<Binding> {
		provider.require_credential()?;
		let mut scope = self.repository.begin(tenant).await?;
		let id = id.into();
		if let Some(id) = id {
			let row = scope.get(id).await?;
			row.require_active()?;
			if row.tenant != tenant || row.provider != provider {
				return Err(Error::Invalid(
					"Provider Credential binding target does not match Tenant and provider".into(),
				));
			}
		}
		let current = scope
			.bindings()
			.await?
			.into_iter()
			.find(|b| b.provider == provider);
		if current.map_or(0, |b| b.revision) != expected {
			return Err(Error::Conflict(
				"stale Provider Credential Binding revision".into(),
			));
		}
		let binding = Binding {
			tenant: tenant.into(),
			provider,
			provider_credential_id: id,
			revision: expected + 1,
		};
		scope.bind(&binding, actor).await?;
		scope.commit().await?;
		Ok(binding)
	}
	pub async fn reconcile(&self) -> Result<usize> {
		let result = self.reconcile_page(None).await?;
		if result.failed != 0 {
			return Err(Error::External(
				"Provider Credential cleanup is pending".into(),
			));
		}
		Ok(result.cleaned)
	}
	/// Run one bounded page. Callers retain `next` and delay before the next
	/// pass, including failures, so one outage cannot starve later Tenants.
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
	async fn reconcile_candidate(&self, candidate: ProviderCredential) -> Result<bool> {
		let mut scope = self.repository.begin(&candidate.tenant).await?;
		let mut row = scope.get(candidate.id).await?;
		let mut cleaned = false;
		match row.state {
			State::Pending if row.created_at <= Utc::now() - Duration::minutes(5) => {
				// Delete is idempotent across crashes before/after Secret creation.
				self.store.delete(&row.tenant, &row.secret_resource).await?;
				row.state = State::Deleted;
				row.revision += 1;
				scope
					.save(
						&row,
						"provider_credential.deleted",
						"provider-credential-reconciler",
					)
					.await?;
				cleaned = true;
			}
			State::Active => {
				let pinned = row.require_active()?;
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
			State::Revoked => {
				for version in self
					.store
					.versions(&row.tenant, &row.secret_resource)
					.await?
				{
					self.store.disable(&row.tenant, &version).await?;
				}
			}
			State::Deleted if row.pinned_version.is_some() => {
				for version in self
					.store
					.versions(&row.tenant, &row.secret_resource)
					.await?
				{
					self.store.destroy(&row.tenant, &version).await?;
				}
				self.store.delete(&row.tenant, &row.secret_resource).await?;
				row.pinned_version = None;
				scope
					.save(
						&row,
						"provider_credential.cleanup_completed",
						"provider-credential-reconciler",
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
/// Test-only write port (ADR 0006). Tests and CI register Provider Credentials
/// with fixture Key Material through it. It is compiled only into this crate's
/// tests or with the `test-support` feature, which no production dependency
/// enables (`scripts/check-workspace-boundaries.py` enforces this).
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
	use super::*;
	#[async_trait]
	pub trait KeyMaterialWritePort {
		async fn create(
			&self,
			tenant: &str,
			id: Uuid,
			provider: Provider,
			key: SecretString,
			actor: &str,
		) -> Result<Validated>;
		async fn rotate(
			&self,
			tenant: &str,
			id: Uuid,
			expected: i64,
			key: SecretString,
			actor: &str,
		) -> Result<Validated>;
	}
	#[async_trait]
	impl KeyMaterialWritePort for Service {
		async fn create(
			&self,
			tenant: &str,
			id: Uuid,
			provider: Provider,
			key: SecretString,
			actor: &str,
		) -> Result<Validated> {
			self.write_new(tenant, id, provider, key, None, actor).await
		}
		async fn rotate(
			&self,
			tenant: &str,
			id: Uuid,
			expected: i64,
			key: SecretString,
			actor: &str,
		) -> Result<Validated> {
			self.write_rotation(tenant, id, expected, key, None, actor)
				.await
		}
	}
	/// Fails every exchange; for tests that never run Provider Authorization.
	pub struct NoExchange;
	#[async_trait]
	impl crate::provider_authorization::Exchange for NoExchange {
		async fn exchange(&self, _: Provider, _: &str, _: &SecretString) -> Result<SecretString> {
			Err(Error::Invalid(
				"Provider Authorization is not available in this test".into(),
			))
		}
	}
}
#[cfg(test)]
mod tests;
