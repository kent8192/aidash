//! Harness-owned files and execution capabilities. No Registry tool identity grants authority.
pub mod api;
pub(crate) mod approvals;
pub(crate) mod cleanup;
pub(crate) mod configuration;
pub mod contracts;
pub(crate) mod errors;
pub mod limits;
pub(crate) mod management;
pub(crate) mod network;
pub mod objects;
pub mod operations;
pub(crate) mod packages;
pub(crate) mod patch;
pub(crate) mod python;
pub(crate) mod reclamation;
pub(crate) mod records;
pub(crate) mod references;
pub(crate) mod service;
pub(crate) mod sessions;
pub(crate) mod sharing;
pub(crate) mod skills;
pub(crate) mod thread_lifecycle;
pub(crate) mod tools;
pub mod transfer;

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CoreCapabilities {
	pub files: bool,
	pub shell: bool,
	pub python: bool,
	pub patch: bool,
	pub skills: bool,
	pub sharing: bool,
}
impl CoreCapabilities {
	pub fn enabled(&self) -> bool {
		self.files || self.shell || self.python || self.patch || self.skills || self.sharing
	}
	pub fn permits(&self, name: &str) -> bool {
		match name {
			"file_search" | "file_read" => self.files,
			"shell" | "shell_poll" | "shell_cancel" => self.shell,
			"code_interpreter" | "python_install" | "python_poll" | "python_cancel" => self.python,
			"apply_patch" => self.patch,
			"skill_list" | "skill_load" | "skill_read" => self.skills,
			"file_share" => self.sharing,
			"outbound_get" => self.shell || self.python,
			_ => false,
		}
	}
}

/// One operator-owned execution profile. Agents can never raise these ceilings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
	pub limits: limits::ContentLimits,
	pub admission: bool,
	pub outbound_origins: Vec<String>,
	pub package_origins: Vec<String>,
	pub storage: PathBuf,
	pub working_bytes: u64,
	pub retained_bytes: u64,
	pub temporary_bytes: u64,
	pub cpu: u32,
	pub memory_bytes: u64,
	pub processes: u32,
	pub operation_seconds: u64,
	pub maximum_seconds: u64,
	pub install_seconds: u64,
	pub idle_seconds: u64,
	pub recovery_seconds: u64,
	pub grant_seconds: u64,
	pub approval_seconds: u64,
	pub output_bytes: u64,
	pub staging_seconds: u64,
	pub runner: Option<RunnerProfile>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerProfile {
	pub endpoint: String,
	#[serde(default)]
	pub node_guard: Option<Vec<String>>,
	pub token_env: String,
	pub image: String,
	pub runtime_class: String,
	pub namespace: String,
	pub journal: PathBuf,
	pub kubectl: PathBuf,
	pub kubeconfig: PathBuf,
	pub listen_host: String,
	pub listen_port: u16,
}
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
