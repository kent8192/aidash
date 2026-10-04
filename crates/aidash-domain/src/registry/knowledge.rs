//! Private reference text invariants and immutable content identity.
use super::ReferenceDocument;
use crate::{Error, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn validate(documents: &[ReferenceDocument]) -> Result<()> {
	if documents.is_empty()
		|| documents.len() > 8
		|| documents.iter().map(|d| d.text.len()).sum::<usize>() > 65536
	{
		return Err(Error::Invalid(
			"attach 1..8 reference documents with at most 64 KiB of extracted text in total".into(),
		));
	}
	for d in documents {
		if d.name.contains('\0') || d.text.contains('\0') {
			return Err(Error::Invalid("reference document names and text must not contain NUL characters; decode text as UTF-8 before upload".into()));
		}
		if d.name.trim().is_empty()
			|| d.name.len() > 255
			|| d.text.trim().is_empty()
			|| !matches!(
				d.media_type.as_str(),
				"application/pdf"
					| "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
					| "text/plain"
			) {
			return Err(Error::Invalid(
				"invalid or empty reference document; scanned PDFs require OCR before upload"
					.into(),
			));
		}
	}
	Ok(())
}

pub fn digest(value: &Value) -> String {
	format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}
#[cfg(test)]
mod tests;
