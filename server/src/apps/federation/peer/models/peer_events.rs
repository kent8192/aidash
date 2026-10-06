//! Persistent peer_events records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "federation", table_name = "peer_events")]
#[derive(Serialize, Deserialize)]
pub struct PeerEvent {
	#[field(primary_key = true, field_type = "text")]
	pub node_id: String,
	#[field(primary_key = true, field_type = "uuid")]
	pub event_id: uuid::Uuid,
}
