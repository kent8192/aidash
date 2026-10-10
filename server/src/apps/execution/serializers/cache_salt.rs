//! Node Cache Salt Keys for salted Projection Versions (ADR 0016).
use aidash_integrations::inference::{CacheSaltKey, CacheSaltKeys};
use reinhardt::conf::settings::{
	fragment::SettingsValidation,
	profile::Profile,
	validation::{ValidationError, ValidationResult},
};
use reinhardt::settings;
use serde::{Deserialize, Serialize};
use std::fmt;

/// `[cache_salt]` with `keys = [{ version = 1, secret = "${AIDASH_CACHE_SALT_KEY_V1}" }]`
/// and `current = 1`. Rotation changes `current`. Without keys this node
/// creates only Legacy Runs.
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
