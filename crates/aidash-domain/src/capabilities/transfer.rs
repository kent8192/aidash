//! File-transfer/1 identity and exact durable receipt correspondence.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
// Serializable transfer contracts.
use super::{operations::MountedFile as FileEntry, sharing::Recipient};
use crate::registry::EntityRef;
use chrono::{DateTime, Utc};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Description {
	pub protocol: String,
	pub transfer_id: Uuid,
	pub source_node: String,
	pub target: Recipient,
	pub source_tenant: String,
	pub source_subject: String,
	pub source_agent: EntityRef,
	pub input_digest: String,
	pub manifest_digest: String,
	pub files: Vec<FileEntry>,
	pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
	pub transfer_id: Uuid,
	pub input_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requester {
	pub tenant: String,
	pub subject: String,
	#[serde(default)]
	pub cursor: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
	pub transfer_id: Uuid,
	pub input_digest: String,
	pub file: usize,
	pub offset: u64,
	pub data: String,
}

use uuid::Uuid;

pub fn receipt_matches(description: &Description, response: &Value) -> bool {
	let Some(files) = response["receipt"]["files"].as_array() else {
		return false;
	};
	response["state"] == "committed"
		&& response["transfer_id"] == json!(description.transfer_id)
		&& response["input_digest"] == description.input_digest
		&& response["manifest_digest"] == description.manifest_digest
		&& response["receipt"]["node_id"] == description.target.node_id
		&& files.len() == description.files.len()
		&& files.iter().zip(&description.files).all(|(got, expected)| {
			got["digest"] == expected.digest
				&& got["size"] == expected.size
				&& got["path"] == format!("{}/{}", description.transfer_id, expected.path)
		})
}
#[cfg(test)]
mod tests;
