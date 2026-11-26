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
			node: record.node.map(|node| {
				aidash_domain::nodes::NodeName::parse(&node).expect("stored Node name")
			}),
			thread_id: record.thread_id,
			kind: record.kind,
			data: record.data.0,
			created_at: record.created_at,
		}
	}
}

impl From<MessageRecord> for Message {
	fn from(record: MessageRecord) -> Self {
		let thread_id = record.thread_id();
		Self {
			id: record.id,
			thread_id,
			sender: record.sender,
			content: record.content,
			completion: record
				.completion
				.and_then(|value| serde_json::from_value(value.into_inner()).ok()),
			idempotency_key: record.idempotency_key,
			created_at: record.created_at,
			run_id: record.run_id,
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
