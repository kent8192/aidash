//! Explicit conversion from persistence records to the public activity contract.
use super::entities::Run;
use super::entities::{Event, Message};
use crate::apps::execution::models::{Event as EventRecord, Run as RunRecord};
use crate::apps::workspaces::models::Message as MessageRecord;

impl From<EventRecord> for Event {
	fn from(record: EventRecord) -> Self {
		Self {
			sequence: record.sequence,
			id: record.id,
			node_id: record.node_id,
			workspace_id: record.workspace_id,
			kind: record.kind,
			data: record.data.0,
			created_at: record.created_at,
		}
	}
}

impl From<MessageRecord> for Message {
	fn from(record: MessageRecord) -> Self {
		let workspace_id = record.workspace_id();
		Self {
			id: record.id,
			workspace_id,
			sender: record.sender,
			content: record.content,
			idempotency_key: record.idempotency_key,
			created_at: record.created_at,
		}
	}
}

impl TryFrom<RunRecord> for Run {
	type Error = crate::Error;
	fn try_from(record: RunRecord) -> crate::Result<Self> {
		decode_run(serde_json::to_value(record)?)
	}
}

pub(crate) fn decode_run(row: serde_json::Value) -> crate::Result<Run> {
	Ok(serde_json::from_value::<crate::domain::run_state::RawRun>(row)?.decode()?)
}
