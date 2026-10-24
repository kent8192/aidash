use crate::{Error, Result};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Settings {
	pub namespace: String,
	pub bootstrap: bool,
	pub max_age: Duration,
	pub max_bytes: i64,
	pub replicas: usize,
	pub slots: usize,
	pub recovery: Duration,
	pub fallback: Duration,
	pub test_pause_file: Option<std::path::PathBuf>,
	pub test_after_ack_pause_file: Option<std::path::PathBuf>,
}
impl Default for Settings {
	fn default() -> Self {
		Self {
			namespace: "default".into(),
			bootstrap: false,
			max_age: Duration::from_secs(86400),
			max_bytes: 1024 * 1024 * 1024,
			replicas: 1,
			slots: 4,
			recovery: Duration::from_secs(5),
			fallback: Duration::from_secs(1),
			test_pause_file: None,
			test_after_ack_pause_file: None,
		}
	}
}
impl Settings {
	pub fn from_env() -> Result<Self> {
		fn integer(key: &str, default: u64, max: u64) -> Result<u64> {
			let value = match std::env::var(key) {
				Ok(s) => s
					.parse::<u64>()
					.map_err(|_| Error::Invalid(format!("invalid {key}")))?,
				Err(_) => default,
			};
			if value == 0 || value > max {
				return Err(Error::Invalid(format!("invalid {key}")));
			}
			Ok(value)
		}
		let mut s = Self::default();
		s.namespace = std::env::var("AIDASH_ACTIVATION_NAMESPACE").unwrap_or(s.namespace);
		if s.namespace.is_empty() || s.namespace.len() > 128 {
			return Err(Error::Invalid("invalid activation namespace".into()));
		}
		s.bootstrap = match std::env::var("AIDASH_ACTIVATION_BOOTSTRAP").as_deref() {
			Ok("true") => true,
			Ok("false") | Err(_) => false,
			_ => return Err(Error::Invalid("invalid AIDASH_ACTIVATION_BOOTSTRAP".into())),
		};
		s.slots = integer("AIDASH_WORKER_SLOTS", 4, 4)? as usize;
		s.max_age = Duration::from_secs(integer(
			"AIDASH_ACTIVATION_MAX_AGE_SECONDS",
			86400,
			31536000,
		)?);
		s.max_bytes = integer("AIDASH_ACTIVATION_MAX_BYTES", 1073741824, i64::MAX as u64)? as i64;
		s.replicas = integer("AIDASH_ACTIVATION_REPLICAS", 1, 5)? as usize;
		// A deliberate acceptance-test override; normal deployments are bounded by 5s/1s.
		s.test_pause_file = std::env::var_os("AIDASH_ACTIVATION_TEST_PAUSE_FILE").map(Into::into);
		s.test_after_ack_pause_file =
			std::env::var_os("AIDASH_ACTIVATION_TEST_AFTER_ACK_PAUSE_FILE").map(Into::into);
		let test_delay = std::env::var("AIDASH_ACTIVATION_TEST_RECOVERY_MS").ok();
		let has_test_controls = test_delay.is_some()
			|| s.test_pause_file.is_some()
			|| s.test_after_ack_pause_file.is_some();
		if has_test_controls && std::env::var("AIDASH_ENV").as_deref() != Ok("test") {
			return Err(Error::Invalid(
				"activation overrides require AIDASH_ENV=test".into(),
			));
		}
		if test_delay.is_some() {
			let delay =
				Duration::from_millis(integer("AIDASH_ACTIVATION_TEST_RECOVERY_MS", 60000, 60000)?);
			s.recovery = delay;
			s.fallback = delay;
		}
		Ok(s)
	}
}
