use serde::{Deserialize, Serialize};
// Serializable core contracts.
use crate::apps::execution::capabilities::services::core::*;
use std::path::PathBuf;

/// One operator-owned execution profile. Agents can never raise these ceilings.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
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

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
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

pub use aidash_domain::capabilities::CoreCapabilities;
