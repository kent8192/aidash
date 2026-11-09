//! The live Scratch Space behind a scope credential.
//!
//! A credential is a MAC over the Tenant and the Scratch Space, so it needs no
//! table and has the same value on every Pod `ensure`, which keeps the Pod's spec
//! stable. It stays valid while the Scratch Space exists and neither its Tenant
//! nor its Node is deleted (ADR 0060, ADR 0083); host and suspension changes
//! reach an open connection through the gateway's periodic recheck.
use aidash_application::egress::{Admission, ScopeAuthority, ScopeSettings};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::egress::{AllowedHosts, AnomalyRule, DailyBudget, ScopeId};
use aidash_domain::scratch::{HostAllowlist, ScratchState};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use uuid::Uuid;

use crate::apps::execution::repositories::egress::runner_key;
use crate::apps::identity::repositories::{node_deletions, tenant_deletions};
use crate::apps::workspaces::repositories::scratch;
use crate::config::ScratchOperatorPolicy;
use crate::store::Store;

const DOMAIN: &[u8] = b"aidash-scope-egress-v1\0";
const PREFIX: &str = "aidash-scope-v1";

fn scope_mac(key: &[u8], tenant: &str, space: Uuid) -> Hmac<Sha256> {
	let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC key");
	mac.update(DOMAIN);
	mac.update(&(tenant.len() as u64).to_be_bytes());
	mac.update(tenant.as_bytes());
	mac.update(space.as_bytes());
	mac
}

/// Holds only URL-safe characters, because a Pod carries it in its proxy URL.
fn issue(key: &[u8], tenant: &str, space: Uuid) -> String {
	let signature = URL_SAFE_NO_PAD.encode(scope_mac(key, tenant, space).finalize().into_bytes());
	let tenant = URL_SAFE_NO_PAD.encode(tenant);
	format!("{PREFIX}.{space}.{tenant}.{signature}")
}

/// The Tenant and Scratch Space a credential was issued for. The signature is
/// checked before either value is used, and in constant time.
fn verify(key: &[u8], credential: &str) -> Option<(String, Uuid)> {
	let mut parts = credential.split('.');
	let (prefix, space, tenant, signature) =
		(parts.next()?, parts.next()?, parts.next()?, parts.next()?);
	if prefix != PREFIX || parts.next().is_some() {
		return None;
	}
	let space = Uuid::parse_str(space).ok()?;
	let tenant = String::from_utf8(URL_SAFE_NO_PAD.decode(tenant).ok()?).ok()?;
	let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
	scope_mac(key, &tenant, space)
		.verify_slice(&signature)
		.ok()?;
	Some((tenant, space))
}

/// Applies a Scratch Space's current settings under the operator's and Tenant's
/// current limits.
pub struct ScratchAuthority {
	store: Store,
	policy: ScratchOperatorPolicy,
	#[cfg(test)]
	key: Option<SecretString>,
}
impl ScratchAuthority {
	pub fn new(store: Store, policy: ScratchOperatorPolicy) -> Self {
		Self {
			store,
			policy,
			#[cfg(test)]
			key: None,
		}
	}

	#[cfg(test)]
	pub(super) fn with_key(store: Store, policy: ScratchOperatorPolicy, key: &str) -> Self {
		Self {
			key: Some(SecretString::from(key.to_owned())),
			..Self::new(store, policy)
		}
	}

	fn key(&self) -> crate::Result<SecretString> {
		#[cfg(test)]
		if let Some(key) = &self.key {
			return Ok(key.clone());
		}
		runner_key(&self.store)
	}

	/// The credential to pass in `/v2/pods/ensure`.
	pub fn credential(&self, tenant: &str, space: Uuid) -> crate::Result<String> {
		Ok(issue(self.key()?.expose_secret().as_bytes(), tenant, space))
	}

	async fn settings(
		&self,
		tenant: &str,
		space: Uuid,
	) -> aidash_application::Result<Option<ScopeSettings>> {
		let found: crate::Result<_> = async {
			let mut tx = self
				.store
				.pool
				.begin(&TenancyScope::tenant(tenant)?)
				.await?;
			let found = scratch::read(&mut *tx, tenant, space).await?;
			// Deleting a Tenant or a Node closes admission at once, although the
			// Scratch Space stays Active until its data is purged.
			let deleted = match &found {
				Some(found) => {
					tenant_deletions::is_deleted(&mut *tx, tenant).await?
						|| !node_deletions::current_of(
							&mut *tx,
							tenant,
							vec![found.node.to_string()],
						)
						.await?
						.is_empty()
				}
				None => false,
			};
			tx.commit().await?;
			Ok((found, deleted))
		}
		.await;
		let (found, deleted) = found.map_err(aidash_application::Error::from)?;
		let Some(found) = found else {
			return Ok(None);
		};
		if found.state != ScratchState::Active || deleted {
			return Err(aidash_application::Error::Forbidden);
		}
		let (ceilings, tenant_hosts) = scratch::effective_policy(&self.policy, tenant);
		let settings = found.settings.effective_for(&ceilings, &tenant_hosts);
		let allowed_hosts = match settings.allowed_hosts {
			HostAllowlist::All => AllowedHosts {
				hosts: Vec::new(),
				allow_all: true,
			},
			HostAllowlist::Hosts(hosts) => AllowedHosts {
				hosts: hosts.into_iter().collect(),
				allow_all: false,
			},
		};
		Ok(Some(ScopeSettings {
			id: ScopeId {
				tenant: tenant.to_owned(),
				node: found.node.to_string(),
				scratch_space: space,
			},
			allowed_hosts,
			budget: DailyBudget {
				sent_bytes: settings.daily_sent_bytes,
				received_bytes: settings.daily_received_bytes,
			},
			anomaly: AnomalyRule::default(),
			operator_allows_all: ceilings.all_hosts_allowed,
			tunnel_limits: Default::default(),
		}))
	}
}

#[async_trait]
impl ScopeAuthority for ScratchAuthority {
	async fn authenticate(&self, credential: &str) -> aidash_application::Result<ScopeSettings> {
		let key = self.key().map_err(aidash_application::Error::from)?;
		let Some((tenant, space)) = verify(key.expose_secret().as_bytes(), credential) else {
			return Err(aidash_application::Error::Unauthorized);
		};
		self.settings(&tenant, space)
			.await?
			.ok_or(aidash_application::Error::Unauthorized)
	}

	async fn current(&self, scope: &ScopeId) -> aidash_application::Result<ScopeSettings> {
		self.settings(&scope.tenant, scope.scratch_space)
			.await?
			.filter(|settings| &settings.id == scope)
			.ok_or(aidash_application::Error::Forbidden)
	}

	/// Refused until #738: admission that attaches a Tenant Secret needs its own
	/// care, and External services keep working through the operation route.
	async fn admit_external(
		&self,
		_: &str,
		_: &ScopeId,
		_: &str,
		_: &str,
		_: &str,
		_: &str,
	) -> aidash_application::Result<Admission> {
		Err(aidash_application::Error::Forbidden)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const KEY: &[u8] = b"test-key";

	#[test]
	fn a_credential_names_its_tenant_and_space_and_stays_the_same() {
		let space = Uuid::new_v4();
		let credential = issue(KEY, "acme", space);
		assert_eq!(credential, issue(KEY, "acme", space));
		assert_eq!(verify(KEY, &credential), Some(("acme".to_owned(), space)));
		assert!(
			credential
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b)),
			"{credential}"
		);
	}

	#[test]
	fn a_changed_or_foreign_credential_is_refused() {
		let space = Uuid::new_v4();
		let credential = issue(KEY, "acme", space);
		assert_eq!(verify(b"other-key", &credential), None);
		let parts: Vec<_> = credential.split('.').collect();
		let beta = URL_SAFE_NO_PAD.encode("beta");
		let other_tenant = format!("{}.{}.{beta}.{}", parts[0], parts[1], parts[3]);
		assert_eq!(verify(KEY, &other_tenant), None);
		let other_space = format!("{}.{}.{}.{}", parts[0], Uuid::new_v4(), parts[2], parts[3]);
		assert_eq!(verify(KEY, &other_space), None);
		for malformed in ["", "invalid", PREFIX, &format!("{credential}.extra")] {
			assert_eq!(verify(KEY, malformed), None, "{malformed}");
		}
	}
}
