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
	/// Pages of a PDF, counted from 1, whose text an Agent cannot read.
	pub pages_without_text: Vec<u32>,
	pub page_count: Option<u32>,
}
/// One document of a bulk add. The server works out its type and digest.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchFile {
	pub path: String,
	pub data: String,
}
/// What happened to one document of a bulk add.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatchOutcome {
	pub path: String,
	pub reference: Option<Reference>,
	/// An existing Reference with the same content was returned.
	pub reused: bool,
	/// Another path in this request holds the same content.
	pub duplicate_of: Option<String>,
	pub error: Option<String>,
}
impl BatchOutcome {
	pub fn added(path: String, reference: Reference, reused: bool) -> Self {
		Self {
			path,
			reference: Some(reference),
			reused,
			duplicate_of: None,
			error: None,
		}
	}
	pub fn refused(path: String, error: &str) -> Self {
		Self {
			path,
			reference: None,
			reused: false,
			duplicate_of: None,
			error: Some(error.to_owned()),
		}
	}
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BatchResult {
	pub files: Vec<BatchOutcome>,
	/// Entries ready to paste into a `reference_attachments` Source.
	pub references: Vec<crate::capabilities::ReferenceAttachment>,
}
/// How a committed original becomes readable, decided by its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readability {
	/// Agents read the original itself, with no Pod and no second copy.
	Text,
	/// Only the sandboxed parser may open it.
	Parser,
	Unsupported,
}
/// An Excel file is a ZIP archive. Plain text may well begin with "PK".
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
pub fn readability(bytes: &[u8]) -> Readability {
	if bytes.starts_with(b"%PDF-") || bytes.starts_with(ZIP_MAGIC) {
		Readability::Parser
	} else if !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok() {
		Readability::Text
	} else {
		Readability::Unsupported
	}
}
/// The type recorded for a file that arrived without a declared one.
pub fn sniffed_media_type(bytes: &[u8]) -> &'static str {
	if bytes.starts_with(b"%PDF-") {
		"application/pdf"
	} else if bytes.starts_with(ZIP_MAGIC) {
		"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
	} else {
		"text/plain"
	}
}
/// The text of one Reference file an Agent can read: its extraction, or the
/// original when the original is itself the text.
pub fn text_bytes(data: &Value) -> u64 {
	let size = |file: &Value| file["size"].as_u64().unwrap_or(0);
	if data["plain_text"] == true {
		size(&data["original"])
	} else {
		size(&data["extraction"])
	}
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
		pages_without_text: serde_json::from_value(record.data["pages_without_text"].clone())
			.unwrap_or_default(),
		page_count: record.data["page_count"].as_u64().map(|pages| pages as u32),
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

/// Room in the parser's output for everything except the escaped text: its
/// state and the list of pages without text.
pub const OUTPUT_OVERHEAD: u64 = 2048;
pub fn text_budget(record: &Record, maximum: usize, output: u64) -> u64 {
	record.data["text_budget"]
		.as_u64()
		.unwrap_or_else(|| (maximum as u64).min(output.saturating_sub(OUTPUT_OVERHEAD) / 6))
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
