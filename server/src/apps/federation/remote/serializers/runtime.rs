// Serializable contracts for remote.

use crate::apps::federation::remote::models::Delegation as DelegationRecord;

impl From<DelegationRecord> for Delegation {
	fn from(record: DelegationRecord) -> Self {
		Self {
			task_id: record.task_id(),
			node_id: record.node_id,
			agent_id: record.agent_id,
			agent_version: record.agent_version,
			delivered: record.delivered,
		}
	}
}

pub use aidash_domain::federation::{Delegation, DiscoveredAgent, Discovery, Offer};
