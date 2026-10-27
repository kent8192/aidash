use serde::{Deserialize, Serialize};
// Serializable core contracts.
use crate::apps::execution::capabilities::services::core::*;
use std::path::PathBuf;

/// The Node's execution profile. Agents can never raise these ceilings.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
	pub egress: Option<EgressSettings>,
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
	/// The only Pod port the Preview NetworkPolicy admits, and the absolute
	/// lifetime the Runner allows a `serve` operation (ADR 0070).
	pub preview_port: Option<u16>,
	pub preview_maximum_seconds: Option<i64>,
	pub runner: Option<RunnerProfile>,
}

/// The application's view of the isolated Runner. Fields after `namespace` are
/// read only by the Runner itself; a shared profile file may carry them.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct RunnerProfile {
	pub endpoint: String,
	pub token_env: String,
	pub image: String,
	pub runtime_class: String,
	pub namespace: String,
	#[serde(default)]
	pub node_guard: Option<Vec<String>>,
	#[serde(default)]
	pub journal: Option<PathBuf>,
	#[serde(default)]
	pub kubectl: Option<PathBuf>,
	#[serde(default)]
	pub kubeconfig: Option<PathBuf>,
	#[serde(default)]
	pub listen_host: Option<String>,
	#[serde(default)]
	pub listen_port: Option<u16>,
}

pub use aidash_domain::capabilities::CoreCapabilities;

#[cfg(test)]
#[path = "../tests/serializers_core_tests.rs"]
mod tests;

/// The self-hosted internal gateway; its address is never published by Ingress.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct EgressSettings {
	pub endpoint: String,
	#[serde(default)]
	pub namespace: Option<String>,
	#[serde(default)]
	pub pod_selector: std::collections::BTreeMap<String, String>,
	pub listen: std::net::SocketAddr,
	#[serde(default)]
	pub limits: aidash_domain::egress::Limits,
}
