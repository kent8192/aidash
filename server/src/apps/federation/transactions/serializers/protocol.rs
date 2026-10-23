//! Request and response contracts.
use crate::apps::federation::transactions::{Status, Vote};
use schemars::JsonSchema;

use chrono::{DateTime, Utc};
use serde::Serialize;
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
