//! Persistent agent_knowledge records.

use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "registry", table_name = "agent_knowledge")]
#[derive(Serialize, Deserialize)]
pub struct AgentKnowledge {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(primary_key = true, field_type = "text", db_column = "agent_id")]
	pub agent_key: String,
	#[field(primary_key = true, field_type = "text")]
	pub agent_version: String,
	#[field]
	pub documents: Json<Value>,
	/// Pinned embedding and image vectors, private to this immutable source row.
	#[field(null = true)]
	pub image_index: Option<Json<Value>>,
	/// Original image bytes are stored separately from the index and metadata.
	#[field(null = true)]
	pub image_0: Option<Vec<u8>>,
	#[field(null = true)]
	pub image_1: Option<Vec<u8>>,
	#[field(null = true)]
	pub image_2: Option<Vec<u8>>,
	#[field(null = true)]
	pub image_3: Option<Vec<u8>>,
	#[field(null = true)]
	pub image_4: Option<Vec<u8>>,
	#[field(null = true)]
	pub image_5: Option<Vec<u8>>,
	#[field(null = true)]
	pub image_6: Option<Vec<u8>>,
	#[field(null = true)]
	pub image_7: Option<Vec<u8>>,
}
