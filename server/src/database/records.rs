//! Decode projected database rows without attaching a persistence trait to a domain type.
use super::native::Row as PgRow;
use crate::{Error, Result as NativeResult};
use aidash_domain::{entities::*, run_state::RawRun};

impl Record for aidash_domain::run_input::RunInput {
	fn decode(row: &PgRow) -> NativeResult<Self> {
		Ok(Self {
			seq: row.try_get("seq")?,
			sender: row.try_get("sender")?,
			content: row.try_get("content")?,
			idempotency_key: row.try_get("idempotency_key")?,
			message_id: row.try_get("message_id")?,
			reference_only: row.try_get("reference_only")?,
		})
	}
}

pub trait Record: Sized {
	fn decode(row: &PgRow) -> NativeResult<Self>;
}

fn text<T: serde::de::DeserializeOwned>(row: &PgRow, column: &str) -> NativeResult<T> {
	let value: String = row.try_get(column)?;
	serde_json::from_value(serde_json::Value::String(value)).map_err(Error::from)
}

macro_rules! record {
    ($name:ident { $($field:ident),* $(,)? } $(, $special:ident = $value:expr)*) => {
        impl Record for $name {
            fn decode(row: &PgRow) -> NativeResult<Self> {
                Ok(Self { $($field: row.try_get(stringify!($field))?,)* $($special: ($value)(row)?,)* })
            }
        }
    };
}
record!(Workspace {
	id,
	title,
	goal,
	state,
	revision,
	created_at
});
record!(
	Task {
		id,
		workspace_id,
		title,
		description,
		requirements,
		owner,
		created_by,
		dependencies,
		parent_id,
		revision,
		created_at
	},
	status = |row| text(row, "status")
);
record!(Artifact {
	id,
	workspace_id,
	task_id,
	kind,
	name,
	content,
	created_by,
	idempotency_key,
	created_at
});
record!(Event {
	sequence,
	id,
	node_id,
	workspace_id,
	kind,
	data,
	created_at
});
record!(Conversation {
	created_by,
	id,
	workspace_id,
	target,
	target_kind,
	created_at
});
record!(HumanRequest {
	answered_by,
	id,
	workspace_id,
	run_id,
	kind,
	prompt,
	response,
	created_at
});
record!(Message {
	id,
	workspace_id,
	sender,
	content,
	idempotency_key,
	created_at
});
record!(
	RunMetadata {
		id,
		task_id,
		workspace_id,
		home_node,
		agent_id,
		agent_version,
		step,
		revision,
		observed_input_seq,
		ledger_worker_ready,
		error,
		lease_owner,
		lease_until,
		updated_at
	},
	phase = |row| text(row, "phase"),
	control = |row| text(row, "control")
);

impl Record for TaskStatus {
	fn decode(row: &PgRow) -> NativeResult<Self> {
		text(row, "status")
	}
}
impl Record for RunControl {
	fn decode(row: &PgRow) -> NativeResult<Self> {
		text(row, "control")
	}
}

impl Record for RawRun {
	fn decode(row: &PgRow) -> NativeResult<Self> {
		Ok(Self {
			metadata: RunMetadata::decode(row)?,
			context: row.try_get("context")?,
			pending: row.try_get("pending")?,
		})
	}
}
impl Record for Run {
	fn decode(row: &PgRow) -> NativeResult<Self> {
		<RawRun as Record>::decode(row)?
			.decode()
			.map_err(|error| Error::Invalid(error.to_string()))
	}
}

use aidash_domain::federation::{Delegation, Peer};
record!(Peer {
	node_id,
	endpoint,
	credential_env,
	protocol_version,
	enabled
});
record!(Delegation {
	task_id,
	node_id,
	agent_id,
	agent_version,
	delivered
});
