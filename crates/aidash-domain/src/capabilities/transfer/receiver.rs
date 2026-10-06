//! Manifest and recipient-page limits are independent of the receiver's database.
use super::Description;
use crate::{Error, Result, capabilities::records::Record};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub struct ManifestLimits {
	pub files: usize,
	pub file_bytes: u64,
	pub bytes: u64,
}
pub const MAX_RECIPIENT_VERSIONS_PER_AREA: usize = 50;
pub fn cap_recipient_versions(versions: &mut Vec<String>) -> bool {
	let truncated = versions.len() > MAX_RECIPIENT_VERSIONS_PER_AREA;
	versions.truncate(MAX_RECIPIENT_VERSIONS_PER_AREA);
	truncated
}
pub fn validate(
	description: &Description,
	limits: &ManifestLimits,
	now: DateTime<Utc>,
) -> Result<u64> {
	if description.files.is_empty()
		|| description.files.len() > limits.files
		|| description.expires_at <= now
		|| description.expires_at > now + Duration::hours(24)
	{
		return Err(Error::Invalid("TRANSFER_MANIFEST_LIMIT".into()));
	}
	let mut paths = std::collections::BTreeSet::new();
	let mut total = 0;
	for file in &description.files {
		crate::registry::rules::validate_path(&file.path)?;
		if !paths.insert(&file.path)
			|| file.size > limits.file_bytes
			|| file.digest.len() != 64
			|| !file
				.digest
				.bytes()
				.all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
		{
			return Err(Error::Invalid("TRANSFER_MANIFEST_LIMIT".into()));
		}
		total += file.size;
	}
	if total > limits.bytes
		|| crate::registry::rules::digest(&json!(description.files)) != description.manifest_digest
	{
		return Err(Error::Invalid("TRANSFER_MANIFEST_INTEGRITY".into()));
	}
	Ok(total)
}
pub fn view(record: &Record) -> Value {
	json!({"protocol":"file-transfer/1","transfer_id":record.id,"input_digest":record.data["description"]["input_digest"],"manifest_digest":record.data["description"]["manifest_digest"],"state":record.state,"receipt":record.data["receipt"]})
}
pub fn chunk_digest(bytes: &[u8]) -> String {
	format!("{:x}", Sha256::digest(bytes))
}
#[cfg(test)]
mod tests;
