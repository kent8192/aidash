//! Scratch Space settings and authority, independent of storage and execution.
use std::collections::BTreeSet;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Error, Result, nodes::NodeName};

const GIB: u64 = 1024 * 1024 * 1024;

pub type ScratchSpaceId = Uuid;

/// All hosts, or an explicit set of canonical DNS names or IP addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", content = "hosts", rename_all = "snake_case")]
pub enum HostAllowlist {
	All,
	Hosts(BTreeSet<String>),
}

/// Public package and source registries enabled for a new Tenant by default.
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

	fn narrows(&self, tenant: &Self, all_allowed: bool) -> bool {
		match (self, tenant) {
			(Self::All, Self::All) => all_allowed,
			(Self::All, Self::Hosts(_)) => false,
			(Self::Hosts(hosts), Self::All) => hosts.iter().all(|h| valid_host(h)),
			(Self::Hosts(hosts), Self::Hosts(allowed)) => {
				hosts.iter().all(|h| valid_host(h) && allowed.contains(h))
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
			capacity_bytes: 1024 * GIB,
			cpu_millis: 4000,
			memory_bytes: 16 * GIB,
			daily_sent_bytes: 1024 * GIB,
			daily_received_bytes: 1024 * GIB,
			docker_allowed: false,
			all_hosts_allowed: false,
		}
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
		if self.capacity_bytes == 0
			|| self.capacity_bytes > ceilings.capacity_bytes
			|| self.capacity_bytes > 1024 * GIB
			|| self.cpu_millis == 0
			|| self.cpu_millis > ceilings.cpu_millis
			|| self.memory_bytes == 0
			|| self.memory_bytes > ceilings.memory_bytes
			|| self
				.idle_expiry_days
				.is_some_and(|days| !(1..=365).contains(&days))
			|| self.daily_sent_bytes == 0
			|| self.daily_sent_bytes > ceilings.daily_sent_bytes
			|| self.daily_received_bytes == 0
			|| self.daily_received_bytes > ceilings.daily_received_bytes
			|| self.execution_image.as_ref().is_some_and(|s| {
				let Some((name, digest)) = s.rsplit_once("@sha256:") else {
					return true;
				};
				name.trim().is_empty()
					|| name != name.trim()
					|| digest.len() != 64
					|| !digest
						.bytes()
						.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
			}) || (self.docker_enabled && !ceilings.docker_allowed)
			|| !self
				.allowed_hosts
				.narrows(tenant_hosts, ceilings.all_hosts_allowed)
		{
			return Err(Error::Invalid(
				"Scratch Space settings exceed allowed limits or are invalid".into(),
			));
		}
		Ok(())
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
