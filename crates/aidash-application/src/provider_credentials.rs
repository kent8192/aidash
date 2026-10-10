//! Provider Credential lifecycle; all Key Material stays in write-only ports.
use crate::{Error, Result};
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
/// Deliberately no access/read-payload operation, even for the fake adapter.
#[async_trait]
pub trait Store: Send + Sync {
	fn resource(&self, id: Uuid) -> String;
	async fn create(&self, tenant: &str, id: Uuid) -> Result<()>;
	async fn add_version(&self, tenant: &str, resource: &str, key: &SecretString)
	-> Result<String>;
	async fn versions(&self, resource: &str) -> Result<Vec<String>>;
	async fn disable(&self, version: &str) -> Result<()>;
	async fn destroy(&self, version: &str) -> Result<()>;
	async fn delete(&self, resource: &str) -> Result<()>;
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
pub struct Service {
	pub repository: Arc<dyn Repository>,
	pub store: Arc<dyn Store>,
	pub validator: Arc<dyn KeyValidator>,
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
	pub async fn create(
		&self,
		tenant: &str,
		id: Uuid,
		provider: Provider,
		key: SecretString,
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
			self.store.delete(&row.secret_resource).await?;
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
		})
	}
	pub async fn rotate(
		&self,
		tenant: &str,
		id: Uuid,
		expected: i64,
		key: SecretString,
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
		row.pinned_version = Some(version.clone());
		row.fingerprint = fingerprint;
		row.last4 = last4;
		row.rotated_at = Some(Utc::now());
		row.revision += 1;
		scope
			.save(&row, "provider_credential.rotated", actor)
			.await?;
		if let Err(error) = scope.commit().await {
			let _cleanup = self.store.disable(&version).await;
			return Err(error);
		}
		// The pin and revision are already committed. PostgreSQL's active row
		// remains the durable inventory for retrying every unpinned version, so
		// report cleanup separately instead of turning a successful rotation into
		// an error that the caller cannot safely retry at its original revision.
		let mut warnings = validation.warnings;
		if self.store.disable(&old).await.is_err() {
			warnings.push(
				"Provider Credential rotation committed; previous version cleanup is pending"
					.into(),
			);
		}
		Ok(Validated {
			provider_credential: row.into(),
			warnings,
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
	pub async fn bind(
		&self,
		tenant: &str,
		provider: Provider,
		id: impl Into<Option<Uuid>>,
		expected: i64,
		actor: &str,
	) -> Result<Binding> {
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
				self.store.delete(&row.secret_resource).await?;
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
				for version in self.store.versions(&row.secret_resource).await? {
					if version != pinned {
						self.store.disable(&version).await?;
					}
				}
			}
			State::Revoked => {
				for version in self.store.versions(&row.secret_resource).await? {
					self.store.disable(&version).await?;
				}
			}
			State::Deleted if row.pinned_version.is_some() => {
				for version in self.store.versions(&row.secret_resource).await? {
					self.store.destroy(&version).await?;
				}
				self.store.delete(&row.secret_resource).await?;
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
#[cfg(test)]
mod tests;
