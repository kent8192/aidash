use serde::{Deserialize, Serialize};
// Serializable contracts for transactions.

use crate::apps::federation::transactions::models::AtomicParticipant;
pub use aidash_domain::transactions::{Isolation, Manifest, Mutation, Participant};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "AtomicTransaction")]
#[derive(sqlx::FromRow)]
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
