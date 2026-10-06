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

impl From<TransactionHistory> for aidash_domain::transactions::management::History {
	fn from(history: TransactionHistory) -> Self {
		Self {
			sequence: history.sequence,
			transaction_id: history.transaction_id,
			role: history.role,
			phase: history.phase,
			detail: history.detail,
			created_at: history.created_at,
		}
	}
}
impl From<aidash_domain::transactions::management::History> for TransactionHistory {
	fn from(history: aidash_domain::transactions::management::History) -> Self {
		Self {
			sequence: history.sequence,
			transaction_id: history.transaction_id,
			role: history.role,
			phase: history.phase,
			detail: history.detail,
			created_at: history.created_at,
		}
	}
}
impl From<aidash_domain::transactions::management::Details> for TransactionDetails {
	fn from(details: aidash_domain::transactions::management::Details) -> Self {
		Self {
			transaction: details.transaction.into(),
			participants: details.participants.into_iter().map(Into::into).collect(),
			history: details.history.into_iter().map(Into::into).collect(),
		}
	}
}
impl From<TransactionTrust> for aidash_domain::transactions::management::Trust {
	fn from(trust: TransactionTrust) -> Self {
		Self {
			node_id: trust.node_id,
			enabled: trust.enabled,
		}
	}
}
impl From<aidash_domain::transactions::management::Trust> for TransactionTrust {
	fn from(trust: aidash_domain::transactions::management::Trust) -> Self {
		Self {
			node_id: trust.node_id,
			enabled: trust.enabled,
		}
	}
}
impl From<aidash_domain::transactions::management::TrustChange> for TrustChange {
	fn from(change: aidash_domain::transactions::management::TrustChange) -> Self {
		Self {
			trust: change.trust.into(),
			pending_transactions: change.pending_transactions,
		}
	}
}
