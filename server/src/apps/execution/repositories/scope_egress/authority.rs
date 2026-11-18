//! Per-process credentials bind a Tenant, Scratch Space and Run (ADR 0136).
//! Preparation uses a distinct MAC domain. Run revocation and current Space
//! authority are checked on every credential authentication.
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

const RUN_DOMAIN: &[u8] = b"aidash-run-egress-v1\0";
const PREPARATION_DOMAIN: &[u8] = b"aidash-preparation-egress-v1\0";
const PREFIX: &str = "aidash-run-v1";

fn scope_mac(key: &[u8], domain: &[u8], tenant: &str, space: Uuid, run: Uuid) -> Hmac<Sha256> {
	let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC key");
	mac.update(domain);
	mac.update(&(tenant.len() as u64).to_be_bytes());
	mac.update(tenant.as_bytes());
	mac.update(space.as_bytes());
	mac.update(run.as_bytes());
	mac
}
fn issue(key: &[u8], domain: &[u8], tenant: &str, space: Uuid, run: Uuid) -> String {
	let signature = URL_SAFE_NO_PAD.encode(
		scope_mac(key, domain, tenant, space, run)
			.finalize()
			.into_bytes(),
	);
	let tenant = URL_SAFE_NO_PAD.encode(tenant);
	format!("{PREFIX}.{space}.{tenant}.{run}.{signature}")
}
/// Verify the entire identity before using any part of it.
fn verify(key: &[u8], credential: &str) -> Option<(String, Uuid, Uuid, bool)> {
	let mut parts = credential.split('.');
	let (prefix, space, tenant, run, signature) = (
		parts.next()?,
		parts.next()?,
		parts.next()?,
		parts.next()?,
		parts.next()?,
	);
	if prefix != PREFIX || parts.next().is_some() {
		return None;
	}
	let space = Uuid::parse_str(space).ok()?;
	let tenant = String::from_utf8(URL_SAFE_NO_PAD.decode(tenant).ok()?).ok()?;
	let run = Uuid::parse_str(run).ok()?;
	let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
	let preparation = if scope_mac(key, RUN_DOMAIN, &tenant, space, run)
		.verify_slice(&signature)
		.is_ok()
	{
		false
	} else if scope_mac(key, PREPARATION_DOMAIN, &tenant, space, run)
		.verify_slice(&signature)
		.is_ok()
	{
		true
	} else {
		return None;
	};
	Some((tenant, space, run, preparation))
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

	/// Credential for an individual Run, passed only to its processes.
	pub fn credential(&self, tenant: &str, space: Uuid, run: Uuid) -> crate::Result<String> {
		Ok(issue(
			self.key()?.expose_secret().as_bytes(),
			RUN_DOMAIN,
			tenant,
			space,
			run,
		))
	}
	/// Preparation identities cannot be substituted for Run credentials.
	pub fn preparation_credential(
		&self,
		tenant: &str,
		space: Uuid,
		operation: Uuid,
	) -> crate::Result<String> {
		Ok(issue(
			self.key()?.expose_secret().as_bytes(),
			PREPARATION_DOMAIN,
			tenant,
			space,
			operation,
		))
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
			run_id: None,
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
		let Some((tenant, space, run, preparation)) =
			verify(key.expose_secret().as_bytes(), credential)
		else {
			return Err(aidash_application::Error::Unauthorized);
		};
		if !preparation {
			use crate::apps::execution::capabilities::models::egress::RunRevocation;
			use reinhardt::db::orm::Model;
			let mut tx = self
				.store
				.pool
				.begin(&TenancyScope::tenant(&tenant)?)
				.await?;
			let revoked = RunRevocation::objects()
				.filter(RunRevocation::field_tenant().eq(tenant.clone()))
				.filter(RunRevocation::field_run_id().eq(run))
				.all_with_executor(&mut *tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)
				.map_err(crate::Error::from)?;
			tx.commit().await?;
			if !revoked.is_empty() {
				return Err(aidash_application::Error::Forbidden);
			}
		}
		let mut settings = self
			.settings(&tenant, space)
			.await?
			.ok_or(aidash_application::Error::Unauthorized)?;
		settings.run_id = if preparation { None } else { Some(run) };
		Ok(settings)
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
	#[test]
	fn credential_binds_tenant_space_run_and_domain() {
		let (space, run) = (Uuid::new_v4(), Uuid::new_v4());
		let credential = issue(b"key", RUN_DOMAIN, "acme", space, run);
		assert_eq!(
			verify(b"key", &credential),
			Some(("acme".into(), space, run, false))
		);
		assert!(verify(b"other", &credential).is_none());
		let altered = credential.replace(&run.to_string(), &Uuid::new_v4().to_string());
		assert!(verify(b"key", &altered).is_none());
		let preparation = issue(b"key", PREPARATION_DOMAIN, "acme", space, run);
		assert_ne!(credential, preparation);
		assert_eq!(
			verify(b"key", &preparation),
			Some(("acme".into(), space, run, true))
		);
		assert!(verify(b"key", "aidash-scope-v1.old.space.signature").is_none());
	}
}
