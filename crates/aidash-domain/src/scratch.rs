//! Scratch Space settings and authority, independent of storage and execution.
use std::collections::BTreeSet;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Error, Result, nodes::NodeName};

const GIB: u64 = 1024 * 1024 * 1024;

pub type ScratchSpaceId = Uuid;

/// All hosts, or an explicit set of DNS hostnames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", content = "hosts", rename_all = "snake_case")]
pub enum HostAllowlist {
	All,
	Hosts(BTreeSet<String>),
}

impl HostAllowlist {
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
	matches!(url::Host::parse(host), Ok(url::Host::Domain(ref value)) if value == host && !value.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
			allowed_hosts: HostAllowlist::Hosts(BTreeSet::new()),
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScratchCeilings {
	pub capacity_bytes: u64,
	pub cpu_millis: u32,
	pub memory_bytes: u64,
	pub docker_allowed: bool,
	pub all_hosts_allowed: bool,
}

impl Default for ScratchCeilings {
	fn default() -> Self {
		Self {
			capacity_bytes: 1024 * GIB,
			cpu_millis: 4000,
			memory_bytes: 16 * GIB,
			docker_allowed: false,
			all_hosts_allowed: false,
		}
	}
}

impl ScratchSettings {
	pub fn validate(&self, ceilings: &ScratchCeilings, tenant_hosts: &HostAllowlist) -> Result<()> {
		if self.capacity_bytes == 0
			|| self.capacity_bytes > ceilings.capacity_bytes
			|| self.cpu_millis == 0
			|| self.cpu_millis > ceilings.cpu_millis
			|| self.memory_bytes == 0
			|| self.memory_bytes > ceilings.memory_bytes
			|| self.idle_expiry_days == Some(0)
			|| self
				.execution_image
				.as_ref()
				.is_some_and(|s| s.trim().is_empty())
			|| (self.docker_enabled && !ceilings.docker_allowed)
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

	pub fn clear(&mut self) {
		self.state = ScratchState::Cleared;
	}
}

#[cfg(test)]
mod tests;
