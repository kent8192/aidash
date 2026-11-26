//! Wire contracts use the domain models; database conversion stays in this app.
use crate::Result;
pub use aidash_domain::entities::*;
pub use aidash_domain::run_state;

pub fn nonempty(value: &str, name: &str) -> Result<()> {
	aidash_domain::nonempty(value, name).map_err(Into::into)
}

use crate::apps::workspaces::models::{Task as TaskRecord, Thread as ThreadRecord};
impl From<TaskRecord> for Task {
	fn from(record: TaskRecord) -> Self {
		Self {
			id: record.id,
			thread_id: record.thread_id,
			title: record.title,
			description: record.description,
			status: serde_json::from_value(
				serde_json::to_value(record.status).expect("status serializes"),
			)
			.expect("matching task state variants"),
			goal: record.goal,
			requirements: record.requirements.into_inner(),
			owner: record.owner,
			created_by: record.created_by,
			dependencies: record.dependencies,
			parent_id: record.parent_id,
			started_by_duty: record.started_by_duty,
			revision: record.revision,
			created_at: record.created_at,
		}
	}
}

impl From<ThreadRecord> for Thread {
	fn from(record: ThreadRecord) -> Self {
		Self {
			id: record.id,
			scratch_space_id: record.scratch_space_id,
			node: aidash_domain::nodes::NodeName::parse(&record.node).expect("stored Node name"),
			title: record.title,
			created_by: record.created_by,
			revision: record.revision,
			created_at: record.created_at,
		}
	}
}
