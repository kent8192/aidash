//! Decode projected database rows without attaching a persistence trait to a domain type.
use super::native::Row as PgRow;
use crate::{Error, Result as NativeResult};
use aidash_domain::{entities::*, run_state::RawRun};

impl Record for aidash_domain::run_input::RunInput {
	fn decode(row: &PgRow) -> NativeResult<Self> {
		let idempotency_key: String = row.try_get("idempotency_key")?;
		let is_data = idempotency_key.starts_with(aidash_domain::run_input::SUBNODE_RESULT_PREFIX);
		Ok(Self {
			seq: row.try_get("seq")?,
			sender: row.try_get("sender")?,
			content: row.try_get("content")?,
			idempotency_key,
			message_id: row.try_get("message_id")?,
			reference_only: row.try_get("reference_only")?,
			is_data,
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
record!(Thread {
	id,
	scratch_space_id,
	node,
	title,
	created_by,
	revision,
	created_at
});
record!(
	Task {
		id,
		thread_id,
		title,
		description,
		goal,
		requirements,
		owner,
		created_by,
		dependencies,
		parent_id,
		started_by_duty,
		revision,
		created_at
	},
	status = |row| text(row, "status")
);
record!(Event {
	sequence,
	id,
	node,
	thread_id,
	kind,
	data,
	created_at
});
record!(Conversation {
	scratch_space_id,
	created_by,
	id,
	thread_id,
	target,
	target_kind,
	created_at
});
record!(
	HumanRequest {
		answered_by,
		id,
		thread_id,
		run_id,
		kind,
		prompt,
		response,
		created_at
	},
	answer_by = |_| NativeResult::Ok(None)
);
record!(
	Message {
		id,
		thread_id,
		sender,
		content,
		idempotency_key,
		created_at,
		run_id
	},
	completion = |row: &PgRow| row
		.try_get::<Option<serde_json::Value>>("completion")?
		.map(serde_json::from_value)
		.transpose()
		.map_err(Error::from)
);
record!(
	RunMetadata {
		id,
		task_id,
		thread_id,
		node,
		home_node,
		agent_id,
		agent_version,
		step,
		revision,
		observed_input_seq,
		ledger_worker_ready,
		control_revision,
		error,
		lease_owner,
		lease_until,
		updated_at
	},
	phase = |row| text(row, "phase"),
	control = |row| text(row, "control"),
	control_source = |row| text(row, "control_source")
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

use aidash_domain::federation::Delegation;
record!(Delegation {
	task_id,
	node_id,
	agent_id,
	agent_version
});
