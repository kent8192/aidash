//! Runner/1 transport for a previously admitted Journal cell.
use crate::{Error, Result, ports::capabilities::runner::RunnerTransport};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

/// Persist this identity and the export digest before making any Runner request.
pub struct Cell<'a> {
	pub operation: Uuid,
	pub area: Uuid,
	pub session: Uuid,
	pub runner_instance: &'a str,
	pub file: Uuid,
	pub epoch: i64,
	pub key_digest: &'a str,
	pub tenant: &'a str,
	pub code: &'a str,
	pub seconds: u64,
	pub export: &'a [u8],
}

impl Cell<'_> {
	pub fn request(&self) -> Value {
		json!({"operation_id":self.operation,"area_id":self.area,"session_id":self.session,"runner_instance":self.runner_instance,
			"epoch":self.epoch,"digest":self.key_digest,"usage_tenant":self.tenant,
			"kind":"journal","code":self.code,"seconds":self.seconds,
			"files":[{"file_id":self.file,"path":"journal.jsonl","scope":"journal",
			"size":self.export.len(),"digest":format!("{:x}", Sha256::digest(self.export))}]})
	}
}

/// Submitting the same persisted cell may be retried after a lost reply.
pub async fn submit(transport: &dyn RunnerTransport, cell: &Cell<'_>) -> Result<Value> {
	let request = cell.request();
	let observed = transport
		.request("POST", "/v1/operations", Some(&request))
		.await?;
	if observed["status"] != "awaiting_files" {
		return Ok(observed);
	}
	let path = format!("/v1/operations/{}", cell.operation);
	// The Runner expects a file even for an empty export.
	if cell.export.is_empty() {
		let body = json!({"data":""});
		transport
			.request(
				"POST",
				&format!("{path}/inputs/{}?offset=0", cell.file),
				Some(&body),
			)
			.await?;
	} else {
		for (index, bytes) in cell.export.chunks(4 * 1024 * 1024).enumerate() {
			let body = json!({"data":STANDARD.encode(bytes)});
			transport
				.request(
					"POST",
					&format!(
						"{path}/inputs/{}?offset={}",
						cell.file,
						index * 4 * 1024 * 1024
					),
					Some(&body),
				)
				.await?;
		}
	}
	transport
		.request("POST", &format!("{path}/start"), None)
		.await
}

/// Decode only completed, frozen cells, with a strict total output bound.
pub fn output(observed: &Value, maximum: usize) -> Result<Value> {
	if !matches!(observed["status"].as_str(), Some("completed" | "failed"))
		|| observed["writer_frozen"] != true
	{
		return Err(Error::Conflict("JOURNAL_CELL_NOT_SETTLED".into()));
	}
	let bytes = STANDARD
		.decode(observed["stdout"].as_str().ok_or(Error::Forbidden)?)
		.map_err(|_| Error::Invalid("invalid Journal Query output".into()))?;
	let text = String::from_utf8_lossy(&bytes);
	let end = text
		.char_indices()
		.map(|(i, _)| i)
		.take_while(|i| *i <= maximum)
		.last()
		.unwrap_or(0);
	let end = if text.len() <= maximum {
		text.len()
	} else {
		end
	};
	let mut result = json!({"contract":"aidash.journal.query/1", "stdout": &text[..end], "truncated": observed["truncated"] == true || end < text.len()});
	if observed["status"] == "failed" {
		result["error"] = json!({"message":observed["error"]["message"].as_str().unwrap_or("Cell failed").chars().take(256).collect::<String>()});
	}
	Ok(result)
}

#[cfg(test)]
mod tests;
