//! Request and response contracts.
use crate::apps::federation::transactions::{Status, Vote};
use schemars::JsonSchema;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Serialize, JsonSchema)]
pub struct TransactionHistory {
	pub sequence: i64,
	pub transaction_id: Uuid,
	pub role: String,
	pub phase: String,
	pub detail: String,
	pub created_at: DateTime<Utc>,
}
#[derive(Serialize, JsonSchema)]
pub struct TransactionDetails {
	pub transaction: Status,
	pub participants: Vec<Vote>,
	pub history: Vec<TransactionHistory>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransactionTrust {
	pub node_id: String,
	pub enabled: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct TrustChange {
	#[serde(flatten)]
	pub trust: TransactionTrust,
	pub pending_transactions: Vec<Uuid>,
}
#[derive(Deserialize, JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct PeerRecovery {
	#[validate(length(min = 1))]
	pub(crate) node_id: String,
	#[validate(length(min = 1))]
	pub(crate) credential_env: String,
}
