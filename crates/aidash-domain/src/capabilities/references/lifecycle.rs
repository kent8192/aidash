//! Reference upload and extraction state remain independent of native rows and the parser.
use crate::capabilities::{operations::MountedFile, records::Record};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Upload {
	pub idempotency_key: Uuid,
	pub name: String,
	pub media_type: String,
	pub size: u64,
	pub digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
	pub offset: u64,
	pub data: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reference {
	pub reference_id: Uuid,
	pub revision: i64,
	pub state: String,
	pub name: String,
	pub media_type: String,
	pub size: u64,
	pub digest: String,
	pub uploaded_bytes: u64,
	pub original: Option<MountedFile>,
	pub extraction: Option<MountedFile>,
	pub extraction_state: Option<String>,
}
pub fn validate_upload(input: &Upload, maximum: u64) -> Result<()> {
	if input.size == 0
		|| input.size > maximum
		|| input.name.is_empty()
		|| input.name.len() > 255
		|| input.name.chars().any(char::is_control)
		|| input.name.contains(['/', '\\'])
		|| matches!(input.name.as_str(), "." | "..")
		|| input.digest.len() != 64
		|| !input
			.digest
			.bytes()
			.all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
		|| !matches!(
			input.media_type.as_str(),
			"application/pdf"
				| "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
				| "text/plain"
		) {
		return Err(Error::Invalid("REFERENCE_UPLOAD_LIMIT".into()));
	}
	Ok(())
}
pub fn valid_chunk(offset: u64, maximum: u64, bytes: &[u8]) -> bool {
	!bytes.is_empty()
		&& bytes.len() <= 4 << 20
		&& bytes.len() as u64 == maximum.saturating_sub(offset).min(4 << 20)
		&& offset
			.checked_add(bytes.len() as u64)
			.is_some_and(|end| end <= maximum)
}
pub fn view(record: &Record) -> serde_json::Result<Reference> {
	let input: Upload = serde_json::from_value(record.data["input"].clone())?;
	Ok(Reference {
		reference_id: record.id,
		revision: record.revision,
		state: record.state.clone(),
		name: input.name,
		media_type: input.media_type,
		size: input.size,
		digest: input.digest,
		uploaded_bytes: record.data["uploaded_bytes"].as_u64().unwrap_or(0),
		original: serde_json::from_value(record.data["original"].clone())?,
		extraction: serde_json::from_value(record.data["extraction"].clone())?,
		extraction_state: record.data["extraction_state"].as_str().map(str::to_owned),
	})
}
pub fn finish_extraction(record: &mut Record, state: &str, digest: &str) {
	record.state = "ready".into();
	record.expires_at = None;
	record.data["extraction_state"] = json!(state);
	record.data["receipt_pending"] = json!(true);
	record.data["runner_digest"] = json!(digest);
}
pub fn next_receipt_cursor(receipts: &[(Uuid, Value)]) -> Uuid {
	receipts.last().map_or_else(Uuid::nil, |(id, _)| *id)
}

pub fn text_budget(record: &Record, maximum: usize, output: u64) -> u64 {
	record.data["text_budget"]
		.as_u64()
		.unwrap_or_else(|| (maximum as u64).min(output.saturating_sub(256) / 6))
}
pub fn extraction_state_supported(state: &str) -> bool {
	matches!(
		state,
		"ready"
			| "text_limit"
			| "non_extractable"
			| "unsupported"
			| "malformed"
			| "encrypted"
			| "page_limit"
			| "file_limit"
			| "expanded_size_limit"
	)
}
#[cfg(test)]
mod tests;
