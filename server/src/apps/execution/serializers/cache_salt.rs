//! Node Cache Salt Keys for salted Projection Versions (ADR 0016).
use aidash_integrations::inference::{CacheSaltKey, CacheSaltKeys};
use hmac::{Hmac, Mac};
use reinhardt::conf::settings::{
	fragment::SettingsValidation,
	profile::Profile,
	validation::{ValidationError, ValidationResult},
};
use reinhardt::settings;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fmt;

/// `[cache_salt]` with `keys = [{ version = 1, secret = "${AIDASH_CACHE_SALT_KEY_V1}" }]`
/// and `current = 1`. Rotation changes `current`. Without keys this node derives
/// one from `core.secret_key` (ADR 0115).
#[settings(fragment = true, section = "cache_salt", validate = false)]
#[derive(Clone, Serialize, Deserialize)]
pub struct CacheSaltSettings {
	#[setting(default = "Vec::new()", leaf)]
	pub keys: Vec<CacheSaltKey>,
	#[setting(default = "None")]
	pub current: Option<u32>,
}

impl fmt::Debug for CacheSaltSettings {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// `CacheSaltKey` prints only its version.
		f.debug_struct("CacheSaltSettings")
			.field("keys", &self.keys)
			.field("current", &self.current)
			.finish()
	}
}

impl CacheSaltSettings {
	/// The validated keys, or `None` when this node configures none.
	pub fn keys(&self) -> crate::Result<Option<CacheSaltKeys>> {
		match self.current {
			None if self.keys.is_empty() => Ok(None),
			None => Err(crate::Error::Invalid(
				"cache_salt.current must name a configured key".into(),
			)),
			Some(current) => Ok(Some(CacheSaltKeys::new(&self.keys, current)?)),
		}
	}
}

impl CacheSaltSettings {
	/// The keys this node salts with: the configured ones, else one derived from
	/// the node's secret key. Native is the default for new Agents, so a node
	/// without a key would reject their Runs (ADR 0115). The derivation is one-way,
	/// and rotating the secret key only causes cache misses.
	pub fn node_keys(&self, secret_key: &str) -> crate::Result<Option<CacheSaltKeys>> {
		if let Some(keys) = self.keys()? {
			return Ok(Some(keys));
		}
		if secret_key.trim().is_empty() {
			return Ok(None);
		}
		let mut mac = Hmac::<Sha256>::new_from_slice(secret_key.as_bytes())
			.map_err(|_| crate::Error::Invalid("invalid core.secret_key".into()))?;
		mac.update(b"aidash-cache-salt/v1");
		let secret = mac
			.finalize()
			.into_bytes()
			.iter()
			.map(|byte| format!("{byte:02x}"))
			.collect();
		Ok(Some(CacheSaltKeys::new(
			&[CacheSaltKey { version: 1, secret }],
			1,
		)?))
	}
}

impl SettingsValidation for CacheSaltSettings {
	fn validate(&self, _profile: &Profile) -> ValidationResult {
		self.keys()
			.map(drop)
			.map_err(|error| ValidationError::Constraint(error.to_string()))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const SECRET: &str = "fixture-cache-salt-settings-secret";

	fn key(version: u32, secret: &str) -> CacheSaltKey {
		CacheSaltKey {
			version,
			secret: secret.into(),
		}
	}

	#[rstest::rstest]
	fn omitted_keys_leave_salting_unavailable() {
		let settings = CacheSaltSettings {
			keys: vec![],
			current: None,
		};
		assert!(settings.keys().unwrap().is_none());
	}

	#[rstest::rstest]
	fn a_node_without_keys_derives_one_from_its_secret_key() {
		let settings = CacheSaltSettings {
			keys: vec![],
			current: None,
		};
		let scope = aidash_domain::projection::CacheScope {
			tenant: "tenant-a".into(),
			key_version: 1,
		};
		let line = |secret: &str| {
			settings
				.node_keys(secret)
				.unwrap()
				.unwrap()
				.line(&scope)
				.unwrap()
		};
		assert_eq!(line(SECRET), line(SECRET), "replicas agree");
		assert_ne!(line(SECRET), line("another-node-secret"));
		assert!(!line(SECRET).contains(SECRET));
		assert!(settings.node_keys(" ").unwrap().is_none());
	}

	#[rstest::rstest]
	fn configured_keys_win_over_the_derived_one() {
		let configured = CacheSaltSettings {
			keys: vec![key(7, SECRET)],
			current: Some(7),
		};
		let keys = configured.node_keys("node-secret").unwrap().unwrap();
		assert_eq!(keys.current(), 7);
	}

	#[rstest::rstest]
	fn configured_keys_expose_only_the_current_version() {
		let settings = CacheSaltSettings {
			keys: vec![key(1, SECRET), key(2, "fixture-rotated-secret")],
			current: Some(2),
		};
		assert_eq!(settings.keys().unwrap().unwrap().current(), 2);
		let rendered = format!("{settings:?} {:?}", settings.keys().unwrap());
		assert!(!rendered.contains(SECRET));
		assert!(!rendered.contains("fixture-rotated-secret"));
	}

	#[rstest::rstest]
	#[case::keys_without_current(vec![key(1, SECRET)], None)]
	#[case::current_without_key(vec![key(1, SECRET)], Some(2))]
	#[case::current_without_any_key(vec![], Some(1))]
	#[case::empty_secret(vec![key(1, "")], Some(1))]
	fn invalid_settings_fail_validation(
		#[case] keys: Vec<CacheSaltKey>,
		#[case] current: Option<u32>,
	) {
		let settings = CacheSaltSettings { keys, current };
		let error = settings.validate(&Profile::parse("local")).unwrap_err();
		assert!(!error.to_string().contains(SECRET));
	}
}
