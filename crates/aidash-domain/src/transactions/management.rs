//! Transaction inspection and peer trust facts, independent of persistence or HTTP.
use super::{authority::Status, coordination::Vote};
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct History {
	pub sequence: i64,
	pub transaction_id: Uuid,
	pub role: String,
	pub phase: String,
	pub detail: String,
	pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Details {
	pub transaction: Status,
	pub participants: Vec<Vote>,
	pub history: Vec<History>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trust {
	pub node_id: String,
	pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustChange {
	pub trust: Trust,
	pub pending_transactions: Vec<Uuid>,
}
