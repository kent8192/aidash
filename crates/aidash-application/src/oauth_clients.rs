//! Registered OAuth Clients and per-resource scope ceilings. Client secrets
//! are write-only and live in the Secret Store, never in response metadata.
use crate::{
	Error, Result,
	provider_credentials::{ClientBinding, Store},
	tenant_secrets::Ownership,
};
use aidash_domain::oauth_connections::{OAuthClient, admissible_scopes, canonical_resource};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use uuid::Uuid;

/// Validate the transport boundary without changing the issuer's exact identity.
pub fn normalize_issuer(issuer: &str) -> Result<String> {
	canonical_resource(issuer).map_err(|error| Error::Invalid(error.to_string()))?;
	Ok(issuer.to_owned())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ConnectionScopeCeiling")]
pub struct ScopeCeiling {
	pub tenant: String,
	pub resource: String,
	pub scopes: Vec<String>,
	pub updated_at: DateTime<Utc>,
	pub revision: i64,
}
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
#[schemars(rename = "OAuthClientMetadata")]
pub struct Metadata {
	pub id: Uuid,
	pub tenant: String,
	pub owner: Ownership,
	pub issuer: String,
	pub client_id: String,
	pub has_secret: bool,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
	pub revision: i64,
}
impl From<OAuthClient> for Metadata {
	fn from(row: OAuthClient) -> Self {
		Self {
			id: row.id,
			tenant: row.tenant,
			owner: if row.owner.is_some() {
				Ownership::Me
			} else {
				Ownership::Tenant
			},
			issuer: row.issuer,
			client_id: row.client_id,
			has_secret: row.pinned_version.is_some(),
			created_at: row.created_at,
			updated_at: row.updated_at,
			revision: row.revision,
		}
	}
}
pub struct Put {
	pub owner: Ownership,
	pub issuer: String,
	pub client_id: String,
	pub client_secret: Option<SecretString>,
	pub expected_revision: i64,
}
/// A serialized Tenant scope protects ownership, uniqueness and revision checks.
#[async_trait]
pub trait Scope: Send {
	async fn clients(&mut self) -> Result<Vec<OAuthClient>>;
	async fn save_client(&mut self, row: &OAuthClient, new: bool) -> Result<()>;
	async fn remove_client(&mut self, id: Uuid) -> Result<()>;
	async fn ceiling(&mut self, resource: &str) -> Result<Option<ScopeCeiling>>;
	async fn save_ceiling(&mut self, row: &ScopeCeiling, new: bool) -> Result<()>;
	async fn remove_ceiling(&mut self, resource: &str) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait Repository: Send + Sync {
	async fn begin(&self, tenant: &str) -> Result<Box<dyn Scope>>;
}
pub struct Service {
	pub repository: Arc<dyn Repository>,
	pub store: Option<Arc<dyn Store>>,
}
fn visible(row: &OAuthClient, subject: &str) -> bool {
	row.owner.is_none() || row.owner.as_deref() == Some(subject)
}
impl Service {
	pub async fn list(&self, tenant: &str, subject: &str) -> Result<Vec<Metadata>> {
		let mut tx = self.repository.begin(tenant).await?;
		let rows = tx.clients().await?;
		tx.commit().await?;
		Ok(rows
			.into_iter()
			.filter(|row| visible(row, subject))
			.map(Into::into)
			.collect())
	}
	pub async fn get(&self, tenant: &str, subject: &str, id: Uuid) -> Result<OAuthClient> {
		let mut tx = self.repository.begin(tenant).await?;
		let row = tx
			.clients()
			.await?
			.into_iter()
			.find(|row| row.id == id && visible(row, subject));
		tx.commit().await?;
		row.ok_or_else(|| Error::NotFound("OAuth Client".into()))
	}
	pub async fn put(
		&self,
		tenant: &str,
		subject: &str,
		id: Option<Uuid>,
		input: Put,
	) -> Result<Metadata> {
		let issuer = normalize_issuer(&input.issuer)?;
		if input.client_id.is_empty()
			|| input.client_id.len() > 4096
			|| input.client_id.chars().any(char::is_control)
		{
			return Err(Error::Invalid("invalid OAuth client_id".into()));
		}
		if let Some(secret) = &input.client_secret
			&& (secret.expose_secret().is_empty()
				|| secret.expose_secret().len() > 4096
				|| secret.expose_secret().chars().any(char::is_control))
		{
			return Err(Error::Invalid("invalid OAuth client_secret".into()));
		}
		let owner = input.owner.subject(subject).map(str::to_owned);
		let mut tx = self.repository.begin(tenant).await?;
		let rows = tx.clients().await?;
		let current = match id {
			Some(id) => Some(
				rows.iter()
					.find(|row| row.id == id && row.owner == owner)
					.ok_or_else(|| Error::NotFound("OAuth Client".into()))?
					.clone(),
			),
			None => rows
				.iter()
				.find(|row| row.owner == owner && row.issuer == issuer)
				.cloned(),
		};
		// A registration's issuer also binds its write-only secret and policy
		// scope. Retargeting requires a new registration, never an update.
		if current.as_ref().is_some_and(|row| row.issuer != issuer) {
			return Err(Error::Conflict("oauth_client_issuer_immutable".into()));
		}
		if input.client_secret.is_some()
			&& self
				.store
				.as_ref()
				.is_none_or(|store| !store.supports_oauth_client_secrets())
		{
			return Err(Error::OAuthClientSecretsUnavailable);
		}
		if current.as_ref().map_or(0, |row| row.revision) != input.expected_revision {
			return Err(Error::Conflict("stale OAuth Client revision".into()));
		}
		if rows.iter().any(|row| {
			row.owner == owner
				&& row.issuer == issuer
				&& current.as_ref().is_none_or(|current| current.id != row.id)
		}) {
			return Err(Error::Conflict(
				"an OAuth Client for this issuer exists".into(),
			));
		}
		if current
			.as_ref()
			.is_some_and(|row| row.secret_resource.is_some() && row.pinned_version.is_none())
			&& input.client_secret.is_none()
		{
			return Err(Error::Conflict(
				"OAuth Client secret write is pending; supply its secret again".into(),
			));
		}
		// The stored secret version is sealed with its client_id; keeping it under a new
		// ID would save a registration whose token exchanges the broker always refuses.
		if current
			.as_ref()
			.is_some_and(|row| row.secret_resource.is_some() && row.client_id != input.client_id)
			&& input.client_secret.is_none()
		{
			return Err(Error::Conflict(
				"oauth_client_secret_required: changing the client ID requires its secret".into(),
			));
		}
		let new = current.is_none();
		let now = Utc::now();
		let mut row = current.unwrap_or_else(|| OAuthClient {
			id: Uuid::now_v7(),
			tenant: tenant.into(),
			owner,
			issuer: issuer.clone(),
			client_id: input.client_id.clone(),
			secret_resource: None,
			pinned_version: None,
			created_at: now,
			updated_at: now,
			revision: 0,
		});
		// Persist the resource inventory before the external Store write. An
		// interrupted write remains discoverable by retirement and purge.
		if input.client_secret.is_some() && row.secret_resource.is_none() {
			row.secret_resource = Some(
				self.store
					.as_ref()
					.ok_or(Error::OAuthClientSecretsUnavailable)?
					.oauth_client_resource(row.id),
			);
		}
		if new || (input.client_secret.is_some() && row.pinned_version.is_none()) {
			tx.save_client(&row, new).await?;
			tx.commit().await?;
			tx = self.repository.begin(tenant).await?;
			row = tx
				.clients()
				.await?
				.into_iter()
				.find(|candidate| candidate.id == row.id)
				.ok_or_else(|| Error::Conflict("OAuth Client retired".into()))?;
			if row.revision != input.expected_revision {
				return Err(Error::Conflict("OAuth Client changed".into()));
			}
		}

		let added = if let Some(secret) = input.client_secret {
			let store = self
				.store
				.as_ref()
				.ok_or(Error::OAuthClientSecretsUnavailable)?;
			store.create_oauth_client(tenant, row.id).await?;
			let resource = store.oauth_client_resource(row.id);
			let pin = store
				.add_client_version(
					tenant,
					&resource,
					ClientBinding {
						issuer: issuer.clone(),
						client_id: input.client_id.clone(),
						redirect_uri: None,
					},
					&secret,
				)
				.await?;
			row.secret_resource = Some(resource);
			row.pinned_version = Some(pin.clone());
			Some(pin)
		} else {
			None
		};
		row.client_id = input.client_id;
		row.updated_at = now;
		row.revision += 1;
		let result = async {
			tx.save_client(&row, false).await?;
			tx.commit().await
		}
		.await;
		if let Err(error) = result {
			if let (Some(pin), Some(store)) = (added, &self.store) {
				let _ = store.destroy(tenant, &pin).await;
			}
			return Err(error);
		}
		if self.cleanup_unpinned(&row).await.is_err() {
			tracing::warn!("OAuth Client updated; old secret version cleanup is pending");
		}

		Ok(row.into())
	}
	/// Ownership was selected by the authenticated management service. A UUID
	/// alone cannot address another person's registration.
	pub async fn delete(&self, tenant: &str, owner: Option<&str>, id: Uuid) -> Result<()> {
		let mut tx = self.repository.begin(tenant).await?;
		let row = tx
			.clients()
			.await?
			.into_iter()
			.find(|row| row.id == id && row.owner.as_deref() == owner)
			.ok_or_else(|| Error::NotFound("OAuth Client".into()))?;
		self.destroy(&row).await?;
		tx.remove_client(id).await?;
		tx.commit().await
	}
	async fn cleanup_unpinned(&self, saved: &OAuthClient) -> Result<()> {
		// Re-read the current pin under the same mutation lock: another
		// replacement may already have committed since this write finished.
		let mut tx = self.repository.begin(&saved.tenant).await?;
		if let Some(row) = tx
			.clients()
			.await?
			.into_iter()
			.find(|row| row.id == saved.id)
			&& let Some(resource) = &row.secret_resource
		{
			let store = self
				.store
				.as_ref()
				.ok_or(Error::OAuthClientSecretsUnavailable)?;
			for version in store.versions(&row.tenant, resource).await? {
				if Some(&version) != row.pinned_version.as_ref() {
					store.destroy(&row.tenant, &version).await?;
				}
			}
		}
		tx.commit().await
	}

	async fn destroy(&self, row: &OAuthClient) -> Result<()> {
		if let Some(resource) = &row.secret_resource {
			let store = self
				.store
				.as_ref()
				.ok_or(Error::OAuthClientSecretsUnavailable)?;
			for version in store.versions(&row.tenant, resource).await? {
				store.destroy(&row.tenant, &version).await?;
			}
			store.delete(&row.tenant, resource).await?;
		}
		Ok(())
	}
	pub async fn retire_subject(&self, tenant: &str, subject: &str) -> Result<()> {
		self.retire(tenant, Some(subject)).await
	}
	pub async fn retire_tenant(&self, tenant: &str) -> Result<()> {
		self.retire(tenant, None).await
	}
	async fn retire(&self, tenant: &str, subject: Option<&str>) -> Result<()> {
		let mut tx = self.repository.begin(tenant).await?;
		for row in tx
			.clients()
			.await?
			.into_iter()
			.filter(|row| subject.is_none() || row.owner.as_deref() == subject)
		{
			self.destroy(&row).await?;
			tx.remove_client(row.id).await?;
		}
		tx.commit().await
	}
	pub async fn ceiling(&self, tenant: &str, resource: &str) -> Result<Option<ScopeCeiling>> {
		let resource =
			canonical_resource(resource).map_err(|error| Error::Invalid(error.to_string()))?;
		let mut tx = self.repository.begin(tenant).await?;
		let row = tx.ceiling(&resource).await?;
		tx.commit().await?;
		Ok(row)
	}
	pub async fn put_ceiling(
		&self,
		tenant: &str,
		resource: &str,
		scopes: Vec<String>,
		expected: i64,
	) -> Result<ScopeCeiling> {
		let resource =
			canonical_resource(resource).map_err(|error| Error::Invalid(error.to_string()))?;
		let scopes = admissible_scopes(&scopes, None, None, None)
			.map_err(|error| Error::Invalid(error.to_string()))?;
		let mut tx = self.repository.begin(tenant).await?;
		let current = tx.ceiling(&resource).await?;
		if current.as_ref().map_or(0, |row| row.revision) != expected {
			return Err(Error::Conflict("stale scope ceiling revision".into()));
		}
		let row = ScopeCeiling {
			tenant: tenant.into(),
			resource,
			scopes,
			updated_at: Utc::now(),
			revision: expected + 1,
		};
		tx.save_ceiling(&row, current.is_none()).await?;
		tx.commit().await?;
		Ok(row)
	}
	pub async fn delete_ceiling(&self, tenant: &str, resource: &str) -> Result<()> {
		let resource =
			canonical_resource(resource).map_err(|error| Error::Invalid(error.to_string()))?;
		let mut tx = self.repository.begin(tenant).await?;
		tx.remove_ceiling(&resource).await?;
		tx.commit().await
	}
}

#[cfg(test)]
mod issuer_tests {
	#[rstest::rstest]
	#[case("https://auth.example/")]
	#[case("https://auth.example/realm/")]
	fn registered_client_preserves_advertised_issuer(#[case] issuer: &str) {
		let stored = super::normalize_issuer(issuer).unwrap();
		assert_eq!(stored, issuer);
	}
}
