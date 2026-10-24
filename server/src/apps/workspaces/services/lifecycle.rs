//! Pure task lifecycle rules, independent of transport and persistence.

impl TaskStatus {
	pub fn can_transition(&self, next: &Self) -> bool {
		fn domain(value: &TaskStatus) -> aidash_domain::TaskStatus {
			match value {
				TaskStatus::Open => aidash_domain::TaskStatus::Open,
				TaskStatus::Claimed => aidash_domain::TaskStatus::Claimed,
				TaskStatus::Running => aidash_domain::TaskStatus::Running,
				TaskStatus::Completed => aidash_domain::TaskStatus::Completed,
				TaskStatus::Failed => aidash_domain::TaskStatus::Failed,
				TaskStatus::Blocked => aidash_domain::TaskStatus::Blocked,
				TaskStatus::Cancelled => aidash_domain::TaskStatus::Cancelled,
				TaskStatus::Abandoned => aidash_domain::TaskStatus::Abandoned,
			}
		}
		domain(self).can_transition(&domain(next))
	}
}

pub use crate::apps::workspaces::serializers::services::{ChildTaskSummary, TaskStatus};
