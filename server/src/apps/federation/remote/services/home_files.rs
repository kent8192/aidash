//! Deliveries to the Home and the Home's person, for a Run executing for another Home (ADR 0139,
//! decisions 20 and 22).
//!
//! Files go back to the Home through admission-bound deliveries, not `file-transfer/1`: the
//! Home identifies the sender by the live grant and the admission, picks the target from its own
//! records, and answers from what the delivery's key already made, so every request here may be
//! repeated after a lost reply.
use super::runtime::Home;
use crate::{Error, Result};
use aidash_domain::{
	HumanRequest,
	capabilities::operations::MountedFile,
	federation::delivery::{self, CHUNK_BYTES, File, Purpose},
};
use base64::Engine as _;
use serde_json::{Value, json};
use uuid::Uuid;

impl Home {
	/// Send `files` to the Home and return its committed receipt. The key is the Run's call key,
	/// so the same call after a crash finds the delivery the first made. Nothing is held open
	/// between requests: a delivery the Run abandons is expired by the Home.
	pub(crate) async fn deliver(
		&self,
		key: &str,
		purpose: &Purpose,
		files: &[(MountedFile, Vec<u8>)],
	) -> Result<Value> {
		let grant =
			crate::authorization::peer::admission::run_grant(&self.federation.store, &self.run)
				.await?
				.ok_or(Error::Forbidden)?;
		let sender = self
			.admitted_sender("home_delivery", &json!({"key":key}))
			.await?;
		let manifest: Vec<File> = files
			.iter()
			.map(|(file, _)| File {
				path: file.path.clone(),
				digest: file.digest.clone(),
				size: file.size,
				media_type: file.media_type.clone(),
			})
			.collect();
		let id = delivery::delivery_id(self.run.id, key);
		let open = json!({
			"grant_id":grant,
			"admission_id":self.run.id,
			"key":key,
			"purpose":purpose,
			"files":manifest,
		});
		let staged = self.delivery_request(&sender, "prepare", &open).await?;
		if delivery::receipt_matches(id, &manifest, &staged) {
			// The same call, after its delivery committed: the receipt is the answer.
			return Ok(staged);
		}
		if staged["state"] != "staging" || staged["transfer_id"] != json!(id) {
			return Err(Error::Conflict("DELIVERY_NOT_STAGING".into()));
		}
		for (index, (_, bytes)) in files.iter().enumerate() {
			let mut offset = 0_usize;
			while offset < bytes.len() {
				let end = bytes.len().min(offset + CHUNK_BYTES as usize);
				let reply = self
					.delivery_request(
						&sender,
						"chunks",
						&json!({
							"grant_id":grant,
							"admission_id":self.run.id,
							"key":key,
							"tool":purpose.tool(),
							"file":index,
							"offset":offset,
							"data":base64::engine::general_purpose::STANDARD.encode(&bytes[offset..end]),
						}),
					)
					.await?;
				if reply["state"] != "staging" {
					return Err(Error::Conflict("DELIVERY_CHUNK_UNCONFIRMED".into()));
				}
				offset = end;
			}
		}
		let receipt = self.delivery_request(&sender, "commit", &open).await?;
		if !delivery::receipt_matches(id, &manifest, &receipt) {
			return Err(Error::Conflict("INVALID_DELIVERY_RECEIPT".into()));
		}
		Ok(receipt)
	}
	async fn delivery_request(
		&self,
		sender: &aidash_domain::federation::route::Sender,
		route: &str,
		body: &Value,
	) -> Result<Value> {
		crate::authorization::peer::authority_request_keeping_not_found::<Value>(
			&self.federation,
			sender,
			&self.run.home_node,
			&format!("/scoped/execution/deliveries/{route}"),
			body,
		)
		.await
	}
	/// `outbound_get`: the Home's person at the root of the chain is asked to allow the fetch.
	pub(crate) async fn forward_outbound_request(
		&self,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		self.command(
			"outbound_request",
			json!({"run_id":self.run.id,"key":key,"prompt":prompt}),
		)
		.await
	}
	/// The state of the request put to the Home's person, whether answered, and how.
	pub(crate) async fn forward_outbound_read(&self, id: Uuid) -> Result<HumanRequest> {
		self.command("outbound_read", json!({"run_id":self.run.id,"id":id}))
			.await
	}
}
