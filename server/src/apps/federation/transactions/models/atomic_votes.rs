//! Persistent atomic_votes records.

use crate::apps::federation::transactions::services::states::AtomicVotePhase;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "federation", table_name = "atomic_votes")]
#[derive(Serialize, Deserialize)]
pub struct AtomicVote {
	#[field(primary_key = true, field_type = "uuid")]
	pub transaction_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub node_id: String,
	#[field(field_type = "text", max_length = 64)]
	pub phase: AtomicVotePhase,
}

impl AtomicVote {}
