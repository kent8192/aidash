use serde::{Deserialize, Serialize};
// Serializable contracts for transactions.

use crate::apps::federation::transactions::models::AtomicParticipant;
pub use aidash_domain::transactions::{Isolation, Manifest, Mutation, Participant};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "AtomicTransaction")]
pub struct Status {
	pub id: Uuid,
	pub digest: String,

	#[schemars(with = "Manifest")]
	pub manifest: Value,
	pub decision: Option<String>,
	pub visible: bool,
	pub complete: bool,
	pub last_error: Option<String>,
	pub created_at: DateTime<Utc>,
}
crate::native_record!(Status {
	id,
	digest,
	manifest,
	decision,
	visible,
	complete,
	last_error,
	created_at
});

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TransactionVote")]
pub struct Vote {
	pub node_id: String,
	pub phase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TransactionParticipantStatus")]
pub struct LocalStatus {
	pub id: Uuid,
	pub coordinator: String,
	pub digest: String,

	#[schemars(with = "Manifest")]
	pub manifest: Value,
	pub phase: String,
	pub updated_at: DateTime<Utc>,
}

impl From<AtomicParticipant> for LocalStatus {
	fn from(row: AtomicParticipant) -> Self {
		Self {
			id: row.id,
			coordinator: row.coordinator,
			digest: row.digest,
			manifest: row.manifest.into_inner(),
			phase: row.phase.as_str().to_owned(),
			updated_at: row.updated_at,
		}
	}
}

use uuid::Uuid;

impl From<Status> for aidash_domain::transactions::authority::Status {
	fn from(status: Status) -> Self {
		Self {
			id: status.id,
			digest: status.digest,
			manifest: status.manifest,
			decision: status.decision,
			visible: status.visible,
			complete: status.complete,
			last_error: status.last_error,
			created_at: status.created_at,
		}
	}
}

impl From<aidash_domain::transactions::authority::Status> for Status {
	fn from(status: aidash_domain::transactions::authority::Status) -> Self {
		Self {
			id: status.id,
			digest: status.digest,
			manifest: status.manifest,
			decision: status.decision,
			visible: status.visible,
			complete: status.complete,
			last_error: status.last_error,
			created_at: status.created_at,
		}
	}
}

impl From<&Status> for aidash_domain::transactions::authority::Status {
	fn from(status: &Status) -> Self {
		status.clone().into()
	}
}

impl From<Vote> for aidash_domain::transactions::coordination::Vote {
	fn from(vote: Vote) -> Self {
		Self {
			node_id: vote.node_id,
			phase: vote.phase,
		}
	}
}

impl From<aidash_domain::transactions::coordination::Vote> for Vote {
	fn from(vote: aidash_domain::transactions::coordination::Vote) -> Self {
		Self {
			node_id: vote.node_id,
			phase: vote.phase,
		}
	}
}

impl From<LocalStatus> for aidash_domain::transactions::coordination::LocalStatus {
	fn from(status: LocalStatus) -> Self {
		Self {
			id: status.id,
			coordinator: status.coordinator,
			digest: status.digest,
			manifest: status.manifest,
			phase: status.phase,
			updated_at: status.updated_at,
		}
	}
}

impl From<aidash_domain::transactions::coordination::LocalStatus> for LocalStatus {
	fn from(status: aidash_domain::transactions::coordination::LocalStatus) -> Self {
		Self {
			id: status.id,
			coordinator: status.coordinator,
			digest: status.digest,
			manifest: status.manifest,
			phase: status.phase,
			updated_at: status.updated_at,
		}
	}
}
