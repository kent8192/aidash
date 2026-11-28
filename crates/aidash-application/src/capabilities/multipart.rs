//! Shared offset, replay and assembly rules for the uploads/chunks/commit flow.
use crate::{Error, Result};
use aidash_domain::capabilities::references::lifecycle::{Chunk, valid_chunk};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

pub use aidash_domain::capabilities::references::lifecycle::CHUNK_BYTES;

/// An identical replay needs no write. Otherwise only the next offset is accepted.
pub fn next_chunk(
	input: &Chunk,
	size: u64,
	uploaded_bytes: u64,
	previous: Option<(&str, u64)>,
) -> Result<Option<Vec<u8>>> {
	let bytes = STANDARD
		.decode(&input.data)
		.map_err(|_| Error::Invalid("INVALID_CHUNK".into()))?;
	if !valid_chunk(input.offset, size, &bytes) {
		return Err(Error::Invalid("UPLOAD_CHUNK_LIMIT".into()));
	}
	if let Some((digest, size)) = previous {
		if size != bytes.len() as u64 || digest != format!("{:x}", Sha256::digest(&bytes)) {
			return Err(Error::Conflict("CHUNK_CHANGED".into()));
		}
		return Ok(None);
	}
	if input.offset != uploaded_bytes {
		return Err(Error::Conflict("UPLOAD_OFFSET_CHANGED".into()));
	}
	Ok(Some(bytes))
}

#[async_trait]
pub trait Assembly: Send {
	type Part: Send;
	async fn read_part(&mut self, part: Self::Part) -> Result<Vec<u8>>;
	async fn append(&mut self, bytes: &[u8]) -> Result<()>;
}

pub async fn assemble<P: Send>(sink: &mut dyn Assembly<Part = P>, parts: Vec<P>) -> Result<()> {
	for part in parts {
		let bytes = sink.read_part(part).await?;
		sink.append(&bytes).await?;
	}
	Ok(())
}

#[cfg(test)]
mod tests;
