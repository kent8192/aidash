use super::contracts::*;
use crate::{
	Error, Result,
	capabilities::{objects::digest, operations},
	store::Store,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

struct Cancellation {
	store: Store,
	id: Uuid,
	digest: String,
	armed: bool,
}
impl Drop for Cancellation {
	fn drop(&mut self) {
		if self.armed {
			let store = self.store.clone();
			let id = self.id;
			let digest = self.digest.clone();
			tokio::spawn(async move {
				let cleanup = async {
					operations::remote(
						&store,
						reqwest::Method::POST,
						&format!("/v1/operations/{id}/cancel"),
						None,
					)
					.await?;
					loop {
						let observed = operations::remote(
							&store,
							reqwest::Method::GET,
							&format!("/v1/operations/{id}"),
							None,
						)
						.await?;
						if observed["termination_confirmed"] == true
							&& matches!(
								observed["status"].as_str(),
								Some("completed" | "failed" | "cancelled")
							) {
							operations::remote(
								&store,
								reqwest::Method::POST,
								&format!("/v1/operations/{id}/ack"),
								Some(json!({"digest":digest})),
							)
							.await?;
							return Ok::<(), Error>(());
						}
						tokio::time::sleep(Duration::from_millis(200)).await;
					}
				};
				let _ = tokio::time::timeout(Duration::from_secs(30), cleanup).await;
			});
		}
	}
}
pub(crate) async fn extract(
	store: &Store,
	bytes: &[u8],
	media: &str,
	encoding: &str,
) -> Result<Value> {
	let health = operations::verified_health(store, false).await?;
	if health["web_extraction_protocol"] != "aidash-web-extraction/1" {
		return Err(Error::Invalid("web_extractor_unavailable".into()));
	}
	let id = Uuid::new_v4();
	let file = Uuid::new_v4();
	let path = format!("/v1/operations/{id}");
	let code = serde_json::to_string(&json!([media, encoding]))?;
	let input_digest = digest(bytes);
	let request_digest = crate::registry::digest(&json!(["web-extraction/1", input_digest, code]));
	let mut cancellation = Cancellation {
		store: store.clone(),
		id,
		digest: request_digest.clone(),
		armed: true,
	};
	let mut observed = operations::remote(store, reqwest::Method::POST, "/v1/operations", Some(json!({
		"operation_id":id,"area_id":id,"epoch":1,
		"digest":request_digest,
		"kind":"web_extract","code":code,"seconds":5,
		"files":[{"file_id":file,"path":"original","scope":"references","size":bytes.len(),"digest":input_digest}]
	}))).await?;
	if observed["status"] != "awaiting_files" {
		return Err(Error::Invalid("web_extractor_unavailable".into()));
	}
	for (index, chunk) in bytes.chunks(1024 * 1024).enumerate() {
		operations::remote(
			store,
			reqwest::Method::POST,
			&format!("{path}/inputs/{file}?offset={}", index * 1024 * 1024),
			Some(json!({"data":STANDARD.encode(chunk)})),
		)
		.await?;
	}
	operations::remote(store, reqwest::Method::POST, &format!("{path}/start"), None).await?;
	loop {
		observed = operations::remote(store, reqwest::Method::GET, &path, None).await?;
		if matches!(
			observed["status"].as_str(),
			Some("completed" | "failed" | "cancelled")
		) {
			break;
		}
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
	if observed["termination_confirmed"] != true
		|| observed["truncated"] == true
		|| observed["exit_code"] != 0
	{
		return Err(Error::Invalid("extraction_failed".into()));
	}
	let encoded = observed["stdout"].as_str().ok_or(Error::Forbidden)?;
	if encoded.len() > 8 * 1024 * 1024 {
		return Err(Error::Invalid("extraction_output_limit".into()));
	}
	let output = STANDARD
		.decode(encoded)
		.map_err(|_| Error::Invalid("extraction_failed".into()))?;
	let value: Value =
		serde_json::from_slice(&output).map_err(|_| Error::Invalid("extraction_failed".into()))?;
	let lines = value["lines"].as_array().ok_or(Error::Forbidden)?;
	if value["isolation"]["child_processes_denied"] != true {
		return Err(Error::Invalid("web_extractor_unavailable".into()));
	}
	let mut size = 0usize;
	if lines.len() > 32768 {
		return Err(Error::Invalid("extraction_output_limit".into()));
	}
	for line in lines {
		let text = line["text"].as_str().ok_or(Error::Forbidden)?;
		size = size.saturating_add(text.len());
		if text.len() > 4096
			|| size > MAX_TEXT
			|| line["page"]
				.as_u64()
				.is_some_and(|page| !(1..=200).contains(&page))
		{
			return Err(Error::Invalid("extraction_output_limit".into()));
		}
	}
	// The Node now owns this bounded parsed value. Release the controller's
	// original bytes and copied stdout; receipts contain metadata only.
	operations::remote(
		store,
		reqwest::Method::POST,
		&format!("{path}/ack"),
		Some(json!({"digest":request_digest})),
	)
	.await?;
	cancellation.armed = false;
	Ok(value)
}
