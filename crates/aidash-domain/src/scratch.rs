//! Scratch Space settings and authority, independent of storage and execution.
use std::{collections::BTreeSet, fmt};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Error, Result, nodes::NodeName};

const GIB: u64 = 1024 * 1024 * 1024;
const MAX_CAPACITY_BYTES: u64 = 1024 * GIB;

pub type ScratchSpaceId = Uuid;

/// All hosts, or an explicit set of canonical DNS names or IP addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", content = "hosts", rename_all = "snake_case")]
pub enum HostAllowlist {
	All,
	Hosts(BTreeSet<String>),
}

/// Public package and source registries enabled for a new Scratch Space by default.
pub fn default_public_hosts() -> BTreeSet<String> {
	crate::egress::DEFAULT_ALLOWED_HOSTS
		.iter()
		.map(|host| (*host).to_owned())
		.collect()
}

impl HostAllowlist {
	/// Restrict destinations to those permitted by both policies.
	pub fn intersection(&self, other: &Self) -> Self {
		match (self, other) {
			(Self::All, value) | (value, Self::All) => value.clone(),
			(Self::Hosts(left), Self::Hosts(right)) => {
				Self::Hosts(left.intersection(right).cloned().collect())
			}
		}
	}
}

fn valid_host(host: &str) -> bool {
	match url::Host::parse(host) {
		Ok(url::Host::Domain(value)) => {
			value == host
				&& value.len() <= 253
				&& value.split('.').all(|label| {
					!label.is_empty()
						&& label.len() <= 63
						&& !label.starts_with('-')
						&& !label.ends_with('-')
						&& label
							.bytes()
							.all(|b| b.is_ascii_alphanumeric() || b == b'-')
				})
		}
		Ok(url::Host::Ipv4(address)) => address.to_string() == host,
		Ok(url::Host::Ipv6(address)) => format!("[{address}]") == host,
		Err(_) => false,
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScratchSettings {
	pub capacity_bytes: u64,
	/// None means the Scratch Space never expires from idleness.
	pub idle_expiry_days: Option<u32>,
	pub allowed_hosts: HostAllowlist,
	pub execution_image: Option<String>,
	/// Registry identifiers of External services available to users of the space.
	pub external_services: BTreeSet<String>,
	pub cpu_millis: u32,
	pub memory_bytes: u64,
	pub docker_enabled: bool,
	pub daily_sent_bytes: u64,
	pub daily_received_bytes: u64,
}

impl Default for ScratchSettings {
	fn default() -> Self {
		Self {
			capacity_bytes: 100 * GIB,
			idle_expiry_days: Some(14),
			allowed_hosts: HostAllowlist::Hosts(default_public_hosts()),
			execution_image: None,
			external_services: BTreeSet::new(),
			cpu_millis: 4000,
			memory_bytes: 16 * GIB,
			docker_enabled: false,
			daily_sent_bytes: 100 * GIB,
			daily_received_bytes: 100 * GIB,
		}
	}
}

/// Effective Deployment ceilings and operator decisions for this Tenant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScratchCeilings {
	pub capacity_bytes: u64,
	pub cpu_millis: u32,
	pub memory_bytes: u64,
	pub daily_sent_bytes: u64,
	pub daily_received_bytes: u64,
	pub docker_allowed: bool,
	pub all_hosts_allowed: bool,
}

impl Default for ScratchCeilings {
	fn default() -> Self {
		Self {
			capacity_bytes: MAX_CAPACITY_BYTES,
			cpu_millis: 4000,
			memory_bytes: 16 * GIB,
			daily_sent_bytes: 1024 * GIB,
			daily_received_bytes: 1024 * GIB,
			docker_allowed: false,
			all_hosts_allowed: true,
		}
	}
}

impl ScratchCeilings {
	/// Whether this Tenant may select all hosts for a Scratch Space.
	pub fn allows_all_hosts(&self, tenant_hosts: &HostAllowlist) -> bool {
		self.all_hosts_allowed && matches!(tenant_hosts, HostAllowlist::All)
	}
}

/// Every setting that must change before the request can be accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema, thiserror::Error)]
pub struct ScratchSettingsRefusal {
	/// Whether both the Deployment switch and the Tenant host policy allow all hosts.
	pub all_hosts_allowed: bool,
	pub violations: Vec<ScratchSettingsViolation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScratchSettingsViolation {
	OutsideTenantHosts {
		hosts: BTreeSet<String>,
	},
	InvalidHosts {
		hosts: BTreeSet<String>,
	},
	AllHostsNotAllowed {
		deployment_all_hosts_allowed: bool,
		tenant_policy_allows_all_hosts: bool,
	},
	NumericLimit {
		field: &'static str,
		value: u64,
		min: u64,
		max: u64,
		deployment_admin_can_raise: bool,
	},
	DockerNotAllowed,
	InvalidExecutionImage,
}

impl fmt::Display for ScratchSettingsRefusal {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		use ScratchSettingsViolation as V;
		write!(
			f,
			"Scratch Space settings refused. This Tenant {} choose all hosts.",
			if self.all_hosts_allowed {
				"may"
			} else {
				"may not"
			}
		)?;
		for violation in &self.violations {
			write!(f, " ")?;
			match violation {
				V::OutsideTenantHosts { hosts } => write!(
					f,
					"The Tenant host policy excludes {}. Use hosts permitted by this Tenant or ask a Deployment Administrator to update the Tenant host policy.",
					hosts
						.iter()
						.map(String::as_str)
						.collect::<Vec<_>>()
						.join(", ")
				)?,
				V::InvalidHosts { hosts } => write!(
					f,
					"{hosts:?} contains invalid host entries. Use canonical DNS names or IP addresses without a URL scheme, port, path or wildcard."
				)?,
				V::AllHostsNotAllowed {
					deployment_all_hosts_allowed,
					tenant_policy_allows_all_hosts,
				} => write!(
					f,
					"The Deployment all-hosts switch is {deployment_all_hosts_allowed}. The Tenant policy's all-hosts permission is {tenant_policy_allows_all_hosts}. List permitted hosts or ask a Deployment Administrator to enable the missing permission."
				)?,
				V::NumericLimit {
					field,
					value,
					min,
					max,
					deployment_admin_can_raise,
				} => {
					write!(f, "{field} is {value}. Choose a value from {min} to {max}.")?;
					if *field == "idle_expiry_days" {
						write!(f, " Use null to disable idle expiry.")?;
					}
					if *deployment_admin_can_raise {
						write!(
							f,
							" A higher value requires a Deployment Administrator to raise this ceiling."
						)?;
					}
				}
				V::DockerNotAllowed => write!(
					f,
					"Docker is disabled for this Tenant. Turn it off for this Scratch Space or ask a Deployment Administrator to enable it for the Tenant."
				)?,
				V::InvalidExecutionImage => write!(
					f,
					"Use an execution image pinned by its SHA-256 digest or select the default image with null."
				)?,
			}
		}
		Ok(())
	}
}

impl ScratchSettings {
	/// Default public destinations narrowed by the Tenant's host policy.
	pub fn for_tenant(tenant_hosts: &HostAllowlist) -> Self {
		let defaults = default_public_hosts();
		let hosts = match tenant_hosts {
			HostAllowlist::All => defaults,
			HostAllowlist::Hosts(hosts) => defaults.intersection(hosts).cloned().collect(),
		};
		Self {
			allowed_hosts: HostAllowlist::Hosts(hosts),
			..Self::default()
		}
	}

	/// Settings enforced for use under the current operator and Tenant limits.
	pub fn effective_for(&self, ceilings: &ScratchCeilings, tenant_hosts: &HostAllowlist) -> Self {
		let mut effective = self.clone();
		effective.capacity_bytes = effective.capacity_bytes.min(ceilings.capacity_bytes);
		effective.cpu_millis = effective.cpu_millis.min(ceilings.cpu_millis);
		effective.memory_bytes = effective.memory_bytes.min(ceilings.memory_bytes);
		effective.daily_sent_bytes = effective.daily_sent_bytes.min(ceilings.daily_sent_bytes);
		effective.daily_received_bytes = effective
			.daily_received_bytes
			.min(ceilings.daily_received_bytes);
		effective.allowed_hosts = effective.allowed_hosts.intersection(tenant_hosts);
		effective
	}

	/// Default settings restricted to the current operator and Tenant limits.
	pub fn for_policy(ceilings: &ScratchCeilings, tenant_hosts: &HostAllowlist) -> Self {
		Self::for_tenant(tenant_hosts).effective_for(ceilings, tenant_hosts)
	}

	pub fn validate(&self, ceilings: &ScratchCeilings, tenant_hosts: &HostAllowlist) -> Result<()> {
		use ScratchSettingsViolation as V;
		let mut violations = Vec::new();
		let all_hosts_allowed = ceilings.allows_all_hosts(tenant_hosts);
		match &self.allowed_hosts {
			HostAllowlist::All if !all_hosts_allowed => violations.push(V::AllHostsNotAllowed {
				deployment_all_hosts_allowed: ceilings.all_hosts_allowed,
				tenant_policy_allows_all_hosts: matches!(tenant_hosts, HostAllowlist::All),
			}),
			HostAllowlist::All => {}
			HostAllowlist::Hosts(hosts) => {
				let invalid: BTreeSet<_> = hosts
					.iter()
					.filter(|host| !valid_host(host))
					.cloned()
					.collect();
				if let HostAllowlist::Hosts(allowed) = tenant_hosts {
					let outside: BTreeSet<_> = hosts
						.difference(allowed)
						.filter(|host| !invalid.contains(*host))
						.cloned()
						.collect();
					if !outside.is_empty() {
						violations.push(V::OutsideTenantHosts { hosts: outside });
					}
				}
				if !invalid.is_empty() {
					violations.push(V::InvalidHosts { hosts: invalid });
				}
			}
		}
		for (field, value, ceiling, hard_max) in [
			(
				"capacity_bytes",
				self.capacity_bytes,
				ceilings.capacity_bytes,
				MAX_CAPACITY_BYTES,
			),
			(
				"cpu_millis",
				self.cpu_millis.into(),
				ceilings.cpu_millis.into(),
				u32::MAX.into(),
			),
			(
				"memory_bytes",
				self.memory_bytes,
				ceilings.memory_bytes,
				u64::MAX,
			),
			(
				"daily_sent_bytes",
				self.daily_sent_bytes,
				ceilings.daily_sent_bytes,
				u64::MAX,
			),
			(
				"daily_received_bytes",
				self.daily_received_bytes,
				ceilings.daily_received_bytes,
				u64::MAX,
			),
		] {
			let max = ceiling.min(hard_max);
			if !(1..=max).contains(&value) {
				violations.push(V::NumericLimit {
					field,
					value,
					min: 1,
					max,
					deployment_admin_can_raise: value > max && value <= hard_max,
				});
			}
		}
		if let Some(days) = self.idle_expiry_days
			&& !(1..=365).contains(&days)
		{
			violations.push(V::NumericLimit {
				field: "idle_expiry_days",
				value: days.into(),
				min: 1,
				max: 365,
				deployment_admin_can_raise: false,
			});
		}
		if self.docker_enabled && !ceilings.docker_allowed {
			violations.push(V::DockerNotAllowed);
		}
		if self.execution_image.as_ref().is_some_and(|s| {
			let Some((name, digest)) = s.rsplit_once("@sha256:") else {
				return true;
			};
			name.trim().is_empty()
				|| name != name.trim()
				|| digest.len() != 64
				|| !digest
					.bytes()
					.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
		}) {
			violations.push(V::InvalidExecutionImage);
		}
		if violations.is_empty() {
			Ok(())
		} else {
			Err(Error::ScratchSettings(Box::new(ScratchSettingsRefusal {
				all_hosts_allowed,
				violations,
			})))
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScratchState {
	Active,
	Cleared,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScratchSpace {
	pub id: ScratchSpaceId,
	pub node: NodeName,
	pub name: String,
	pub creator: String,
	pub members: BTreeSet<String>,
	pub state: ScratchState,
	pub revision: i64,
	/// Monotonic wipe request observed by the Execution Pod runtime.
	pub wipe_generation: i64,
	pub settings: ScratchSettings,
	pub created_at: DateTime<Utc>,
	pub last_used_at: DateTime<Utc>,
}

impl ScratchSpace {
	pub fn may_use(&self, person: &str) -> bool {
		self.state == ScratchState::Active
			&& (self.creator == person || self.members.contains(person))
	}

	/// Agents inherit the authority of the person who started their conversation.
	pub fn agent_may_use(&self, conversation_starter: &str) -> bool {
		self.may_use(conversation_starter)
	}

	pub fn may_modify(&self, person: &str) -> bool {
		self.may_use(person)
	}

	pub fn expires_at(&self) -> Option<DateTime<Utc>> {
		self.settings.idle_expiry_days.and_then(|days| {
			self.last_used_at
				.checked_add_signed(Duration::days(i64::from(days)))
		})
	}

	pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
		self.expires_at().is_some_and(|expiry| now >= expiry)
	}

	pub fn tombstone(&mut self) {
		self.state = ScratchState::Cleared;
	}
}

#[cfg(test)]
mod tests;
