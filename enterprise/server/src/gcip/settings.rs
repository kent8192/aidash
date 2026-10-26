// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! `[dashboard.gcip]` and the managed source that Cloud Environments write.
use aidash_server::{Error, Result};
use indexmap::IndexMap;
use reinhardt::conf::settings::sources::{ConfigSource, ScopedSource, SourceError};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use std::path::PathBuf;

fn absolute_seconds() -> i64 {
	43200
}

fn idle_seconds() -> i64 {
	1800
}

/// Origin that serves the Firebase sign-in helper (`/__/auth/*`) to the browser.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthHelper {
	/// Google's `<project>.firebaseapp.com`; needs no proxy but is cross-origin.
	#[default]
	Firebase,
	/// The public origin, which must transparently proxy `/__/auth/*` to Firebase.
	PublicOrigin,
}

/// `[dashboard.gcip]`, which only the Enterprise build reads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GcipConfig {
	pub project_id: String,
	pub web_api_key: String,
	pub public_origin: String,
	/// The GCIP Tenant with no Tenant Binding whose identities follow the
	/// Tenancy Rules. A Deployment names at most one.
	#[serde(default)]
	pub shared_pool: Option<String>,
	/// GCIP Tenant → the only Tenant its identities may be mapped into.
	#[serde(default)]
	pub tenant_bindings: std::collections::BTreeMap<String, String>,
	#[serde(default)]
	pub providers: std::collections::BTreeMap<String, Vec<String>>,
	#[serde(default)]
	pub password_sign_up: std::collections::BTreeSet<String>,
	/// Pools whose MFA Requirement is `mandatory`. Only a sign-in page hint that
	/// leads a user with no second factor through enrollment; GCIP enforces the
	/// Requirement and second-factor claims are never authority.
	#[serde(default)]
	pub mfa_mandatory: std::collections::BTreeSet<String>,
	/// Sign-in Domain → GCIP Tenant ID. Routing only; never authority.
	#[serde(default)]
	pub sign_in_domains: std::collections::BTreeMap<String, String>,
	#[serde(default)]
	pub auth_helper: AuthHelper,
	#[serde(default = "absolute_seconds")]
	pub session_absolute_seconds: i64,
	#[serde(default = "idle_seconds")]
	pub session_idle_seconds: i64,
}
impl fmt::Debug for GcipConfig {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("GcipConfig")
			.field("project_id", &self.project_id)
			.field("public_origin", &self.public_origin)
			.field("shared_pool", &self.shared_pool)
			.field("tenant_bindings", &self.tenant_bindings)
			.field("auth_helper", &self.auth_helper)
			.finish_non_exhaustive()
	}
}
/// Whether a GCIP Tenant admits anyone, and on which terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolStatus {
	/// The Shared Pool: the Tenancy Rules apply.
	Shared,
	/// Bound to the only Tenant its identities may be mapped into.
	Bound(String),
}

impl PoolStatus {
	/// The Tenant a Tenant Binding names, as `gcip_admissions` records it.
	pub fn tenant(&self) -> Option<&str> {
		match self {
			Self::Shared => None,
			Self::Bound(tenant) => Some(tenant),
		}
	}
}

impl GcipConfig {
	/// What `pool` is under these settings; `None` admits nobody.
	pub fn status(&self, pool: &str) -> Option<PoolStatus> {
		if self.shared_pool.as_deref() == Some(pool) {
			return Some(PoolStatus::Shared);
		}
		self.tenant_bindings
			.get(pool)
			.map(|tenant| PoolStatus::Bound(tenant.clone()))
	}

	pub fn issuer(&self) -> String {
		format!("https://securetoken.google.com/{}", self.project_id)
	}
	/// Firebase `authDomain`: a host with an optional port, never a URL.
	pub fn auth_domain(&self) -> String {
		match self.auth_helper {
			AuthHelper::Firebase => format!("{}.firebaseapp.com", self.project_id),
			AuthHelper::PublicOrigin => {
				let origin = self.public_origin.trim_end_matches('/');
				origin
					.split_once("://")
					.map_or(origin, |(_, authority)| authority)
					.to_owned()
			}
		}
	}
	pub fn normalized(&self) -> Result<Self> {
		if self.project_id.is_empty()
			|| self.project_id.len() > 63
			|| !self
				.project_id
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'-')
			|| self.web_api_key.trim().is_empty()
		{
			return Err(Error::Invalid(
				"dashboard.gcip requires a project ID and web API key".into(),
			));
		}
		let origin = Url::parse(&self.public_origin)
			.map_err(|_| Error::Invalid("invalid dashboard.gcip.public_origin".into()))?;
		if origin.scheme() != "https"
			|| origin.host_str().is_none()
			|| !origin.username().is_empty()
			|| origin.password().is_some()
			|| origin.query().is_some()
			|| origin.fragment().is_some()
			|| origin.path() != "/"
		{
			return Err(Error::Invalid(
				"invalid dashboard.gcip.public_origin".into(),
			));
		}
		let gcip_tenant_id = |pool: &str| {
			!pool.is_empty()
				&& pool.len() <= 128
				&& pool.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
		};
		if let Some(shared) = &self.shared_pool {
			if !gcip_tenant_id(shared) {
				return Err(Error::Invalid("invalid GCIP Tenant ID".into()));
			}
			if self.tenant_bindings.contains_key(shared) {
				return Err(Error::Invalid(
					"the GCIP Shared Pool cannot also have a Tenant Binding".into(),
				));
			}
		}
		let mut tenants = std::collections::BTreeSet::new();
		for (pool, tenant) in &self.tenant_bindings {
			if !gcip_tenant_id(pool) {
				return Err(Error::Invalid("invalid GCIP Tenant ID".into()));
			}
			aidash_domain::policy::identifier(tenant)?;
			if !tenants.insert(tenant) {
				return Err(Error::Invalid("Tenant Bindings must be one-to-one".into()));
			}
		}
		if self
			.providers
			.keys()
			.chain(self.password_sign_up.iter())
			.chain(self.mfa_mandatory.iter())
			.any(|pool| self.status(pool).is_none())
		{
			return Err(Error::Invalid(
				"GCIP provider configuration names a GCIP Tenant that is neither shared nor bound"
					.into(),
			));
		}
		if self.providers.values().flatten().any(|id| {
			id != "password"
				&& id != "google.com"
				&& !id.starts_with("saml.")
				&& !id.starts_with("oidc.")
		}) {
			return Err(Error::Invalid("unsupported GCIP sign-in provider".into()));
		}
		// Runtime lookups use the configured keys, so they must already be in the
		// canonical form that addresses are normalized to (lowercase IDNA ASCII).
		for (domain, pool) in &self.sign_in_domains {
			if sign_in_domain(domain).as_ref() != Some(domain) {
				return Err(Error::Invalid(
					"GCIP Sign-in Domains must be lowercase ASCII DNS names".into(),
				));
			}
			if !self.tenant_bindings.contains_key(pool) {
				return Err(Error::Invalid(
					"GCIP Sign-in Domain requires a Tenant Binding".into(),
				));
			}
		}
		if !(60..=604800).contains(&self.session_idle_seconds)
			|| !(60..=604800).contains(&self.session_absolute_seconds)
			|| self.session_idle_seconds > self.session_absolute_seconds
		{
			return Err(Error::Invalid("invalid GCIP session lifetime".into()));
		}
		Ok(Self {
			public_origin: origin.origin().ascii_serialization(),
			..self.clone()
		})
	}
}

/// Canonical form of a Sign-in Domain: IDNA ASCII, lowercase, at least two DNS
/// labels and no trailing dot. Returns `None` for anything else.
pub fn sign_in_domain(raw: &str) -> Option<String> {
	// The URL host parser percent-decodes its input, so only DNS characters
	// (and non-ASCII for IDN) may reach it; `%61cme.com` must not become `acme.com`.
	if raw.is_empty()
		|| raw.len() > 253
		|| raw.ends_with('.')
		|| raw
			.bytes()
			.any(|b| b.is_ascii() && !(b.is_ascii_alphanumeric() || b == b'-' || b == b'.'))
	{
		return None;
	}
	let url::Host::Domain(domain) = url::Host::parse(raw).ok()? else {
		return None;
	};
	let labels = domain.split('.').collect::<Vec<_>>();
	(domain.len() <= 253
		&& labels.len() >= 2
		&& labels.iter().all(|label| {
			(1..=63).contains(&label.len())
				&& !label.starts_with('-')
				&& !label.ends_with('-')
				&& label
					.bytes()
					.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
		}))
	.then_some(domain)
}

/// A Sign-in Domain only routes sign-in to a GCIP Tenant; it grants no authority.
/// Malformed addresses and unmapped domains are indistinguishable (`None`).
/// The local part must be an RFC 5322 dot-atom of ASCII `atext`; quoted and
/// internationalized local parts are not supported.
pub fn routed_gcip_tenant<'a>(
	sign_in_domains: &'a std::collections::BTreeMap<String, String>,
	email: &str,
) -> Option<&'a str> {
	let email = email.trim();
	if email.len() > 320 {
		return None;
	}
	let (local, domain) = email.split_once('@')?;
	let atext = |b: u8| b.is_ascii_alphanumeric() || b"!#$%&'*+/=?^_`{|}~-".contains(&b);
	if local.len() > 64
		|| domain.contains('@')
		|| !local
			.split('.')
			.all(|atom| !atom.is_empty() && atom.bytes().all(atext))
	{
		return None;
	}
	sign_in_domains
		.get(&sign_in_domain(domain)?)
		.map(String::as_str)
}

/// `AIDASH_GCIP_SETTINGS`: a JSON file holding only `{"dashboard":{"gcip":...}}`.
pub struct ManagedGcipSource {
	pub path: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
	dashboard: Dashboard,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Dashboard {
	gcip: GcipConfig,
}

impl ManagedGcipSource {
	pub fn from_env() -> Self {
		Self {
			path: std::env::var_os("AIDASH_GCIP_SETTINGS").map(PathBuf::from),
		}
	}
}

impl ConfigSource for ManagedGcipSource {
	fn load(&self) -> std::result::Result<IndexMap<String, Value>, SourceError> {
		let Some(path) = &self.path else {
			return Ok(IndexMap::new());
		};
		let envelope: Envelope = serde_json::from_slice(&std::fs::read(path)?)?;
		Ok(IndexMap::from([(
			"dashboard".into(),
			json!({"gcip": envelope.dashboard.gcip}),
		)]))
	}

	fn load_scoped(&self) -> std::result::Result<ScopedSource, SourceError> {
		Ok(ScopedSource {
			values: self.load()?,
			interpolation_file: None,
		})
	}

	fn priority(&self) -> u8 {
		56 // Managed settings override files without replacing higher-priority environment settings.
	}

	fn description(&self) -> String {
		"Managed Cloud GCIP settings".into()
	}
}

#[cfg(test)]
mod tests;
