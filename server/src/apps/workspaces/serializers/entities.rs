//! Wire contracts use the domain models; database conversion stays in this app.
use crate::Result;
pub use aidash_domain::entities::*;
pub use aidash_domain::run_state;

pub fn nonempty(value: &str, name: &str) -> Result<()> {
	aidash_domain::nonempty(value, name).map_err(Into::into)
}

use crate::apps::workspaces::models::{
	Artifact as ArtifactRecord, Task as TaskRecord, Thread as ThreadRecord,
};
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
			requirements: record.requirements.into_inner(),
			owner: record.owner,
			created_by: record.created_by,
			dependencies: record.dependencies,
			parent_id: record.parent_id,
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

impl From<ArtifactRecord> for Artifact {
	fn from(record: ArtifactRecord) -> Self {
		Self {
			id: record.id,
			thread_id: record.thread_id(),
			task_id: record.task_id(),
			kind: match record.kind {
				crate::apps::workspaces::models::states::ArtifactKind::Text => "text",
				crate::apps::workspaces::models::states::ArtifactKind::Json => "json",
				crate::apps::workspaces::models::states::ArtifactKind::FileReference => {
					"file_reference"
				}
				crate::apps::workspaces::models::states::ArtifactKind::Code => "code",
				crate::apps::workspaces::models::states::ArtifactKind::StructuredResult => {
					"structured_result"
				}
			}
			.into(),
			name: record.name,
			content: record.content.into_inner(),
			created_by: record.created_by,
			idempotency_key: record.idempotency_key,
			created_at: record.created_at,
			run_id: record.run_id,
		}
	}
}
