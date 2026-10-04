//! Harness-owned files and execution capabilities. No Registry tool identity grants authority.
#[path = "approvals.rs"]
pub(crate) mod approvals;
#[path = "cleanup.rs"]
pub(crate) mod cleanup;
#[path = "configuration.rs"]
pub(crate) mod configuration;
#[path = "contracts.rs"]
pub mod contracts;
#[path = "endpoints.rs"]
pub(crate) mod endpoints;
#[path = "errors.rs"]
pub(crate) mod errors;
#[path = "limits.rs"]
pub mod limits;
#[path = "management.rs"]
pub(crate) mod management;
#[path = "network.rs"]
pub(crate) mod network;
#[path = "objects.rs"]
pub mod objects;
#[path = "operations.rs"]
pub mod operations;
#[path = "packages.rs"]
pub(crate) mod packages;
#[path = "patch.rs"]
pub(crate) mod patch;
#[path = "python.rs"]
pub(crate) mod python;
#[path = "reclamation.rs"]
pub(crate) mod reclamation;
#[path = "records.rs"]
pub(crate) mod records;
#[path = "references.rs"]
pub(crate) mod references;
#[path = "service.rs"]
pub(crate) mod service;
#[path = "sessions.rs"]
pub(crate) mod sessions;
#[path = "sharing.rs"]
pub(crate) mod sharing;
#[path = "skills.rs"]
pub(crate) mod skills;
#[path = "thread_lifecycle.rs"]
pub(crate) mod thread_lifecycle;
#[path = "tools.rs"]
pub(crate) mod tools;
#[path = "transfer.rs"]
pub mod transfer;

use crate::{Error, Result};
use std::{path::PathBuf, sync::Arc};

impl Default for Profile {
	fn default() -> Self {
		Self {
			limits: limits::ContentLimits::default(),
			admission: false,
			outbound_origins: vec![],
			package_origins: vec![],
			storage: PathBuf::from("/var/lib/aidash/capabilities"),
			working_bytes: 1 << 30,
			retained_bytes: 10 << 30,
			temporary_bytes: 256 << 20,
			cpu: 2,
			memory_bytes: 2 << 30,
			processes: 128,
			operation_seconds: 120,
			maximum_seconds: 600,
			install_seconds: 300,
			idle_seconds: 1800,
			recovery_seconds: 7 * 86400,
			grant_seconds: 3600,
			approval_seconds: 86400,
			output_bytes: 8 << 20,
			staging_seconds: 86400,
			runner: None,
		}
	}
}
#[derive(Debug, Clone)]
pub struct Runtime(pub Arc<Profile>);
impl Runtime {
	pub fn new(profile: Profile) -> Result<Self> {
		if !profile.limits.valid()
			|| !profile.storage.is_absolute()
			|| profile.storage.parent().is_none()
			|| profile.working_bytes == 0
			|| profile.working_bytes > 1 << 30
			|| profile.retained_bytes < profile.working_bytes
			|| profile.retained_bytes > i64::MAX as u64
			|| profile.temporary_bytes == 0
			|| profile.temporary_bytes > i64::MAX as u64
			|| !(1..=64).contains(&profile.cpu)
			|| profile.memory_bytes < 64 << 20
			|| profile.memory_bytes > i64::MAX as u64
			|| !(16..=4096).contains(&profile.processes)
			|| profile.operation_seconds == 0
			|| profile.install_seconds == 0
			|| !(1..=600).contains(&profile.maximum_seconds)
			|| !(1..=1800).contains(&profile.idle_seconds)
			|| profile.recovery_seconds == 0
			|| profile.recovery_seconds > 365 * 86400
			|| !(1..=3600).contains(&profile.grant_seconds)
			|| !(1..=86400).contains(&profile.approval_seconds)
			|| !(1..=86400).contains(&profile.staging_seconds)
			|| profile.output_bytes == 0
			|| profile.output_bytes > 8 << 20
			|| profile.operation_seconds > profile.maximum_seconds
			|| profile.install_seconds > profile.maximum_seconds
			|| profile.grant_seconds > 3600
		{
			return Err(Error::Invalid(
				"invalid capability execution profile".into(),
			));
		}
		for origin in profile
			.outbound_origins
			.iter()
			.chain(&profile.package_origins)
		{
			let url = reqwest::Url::parse(origin)
				.map_err(|_| Error::Invalid("invalid outbound origin".into()))?;
			if url.scheme() != "https"
				|| url.port_or_known_default() != Some(443)
				|| origin != &url.origin().ascii_serialization()
			{
				return Err(Error::Invalid(
					"outbound origins require canonical HTTPS origins on port 443".into(),
				));
			}
		}
		Ok(Self(Arc::new(profile)))
	}
	pub fn from_env() -> Result<Self> {
		let profile = match std::env::var("AIDASH_CAPABILITY_PROFILE") {
			Ok(path) => serde_json::from_slice(&std::fs::read(path)?)?,
			Err(std::env::VarError::NotPresent) => Profile::default(),
			Err(_) => return Err(Error::Invalid("invalid capability profile path".into())),
		};
		Self::new(profile)
	}
}

pub use crate::apps::execution::capabilities::serializers::core::{
	CoreCapabilities, Profile, RunnerProfile,
};
