//! Admission-bound delivery of files from a Run executing for another Home back to that Home
//! (ADR 0139, decision 20).
//!
//! `file-transfer/1` identifies its sender by a Peer Mapping and reads its snapshot from a
//! Workspace the sender holds, so it cannot serve a Run whose Workspace lives at the Home. A
//! delivery is named by the Remote Grant, the admission and the Run's own call key instead. The
//! Home derives everything else (the delegating Run's Working Area, the Workspace, the sender)
//! from the live grant and never from the request.
use crate::capabilities::operations::{FileScope, MountedFile};
use crate::capabilities::sharing::Recipient;
use crate::capabilities::transfer::Description;
use crate::registry::EntityRef;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;

/// The largest chunk a delivery accepts: the chunk size of `/scoped/files/chunk`, which fits the
/// 6 MiB request body of the chunk routes once base64 encoded.
pub const CHUNK_BYTES: u64 = 4 * 1024 * 1024;
/// A message may carry at most this many files, the same as a local `workspace_message`.
pub const MESSAGE_FILES: usize = 8;
/// A message's files may total at most this many bytes, the same as a local `workspace_message`.
pub const MESSAGE_BYTES: u64 = 8 * 1024 * 1024;
/// What a delivery arrives for. Which tool the Run called decides what the Home checks in its
/// Binding and what the Home does with the bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Purpose {
	/// `file_share` to the Home: the files become a delivered share in the Working Area of the
	/// Run that delegated.
	FileShare,
	/// `workspace_message` with files: the files become Thread Attachments of the message the
	/// Home posts for the Agent. `descriptions` is keyed by path, as in the local tool.
	Message {
		content: String,
		#[serde(default)]
		descriptions: BTreeMap<String, String>,
	},
}
impl Purpose {
	/// The tool of the Run's Binding the Home checks.
	pub fn tool(&self) -> &'static str {
		match self {
			Self::FileShare => "file_share",
			Self::Message { .. } => "workspace_message",
		}
	}
}
/// One file of a delivery, as the receiving Node snapshotted it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct File {
	pub path: String,
	pub digest: String,
	pub size: u64,
	pub media_type: String,
}
/// The budgets of one delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
	pub files: usize,
	pub file_bytes: u64,
	pub bytes: u64,
}
impl Limits {
	/// The Deployment's share limits for a file share, the local message limits for a message.
	pub fn of(purpose: &Purpose, share: Limits) -> Self {
		match purpose {
			Purpose::FileShare => share,
			Purpose::Message { .. } => Self {
				files: MESSAGE_FILES.min(share.files),
				file_bytes: MESSAGE_BYTES.min(share.file_bytes),
				bytes: MESSAGE_BYTES.min(share.bytes),
			},
		}
	}
}
/// Check a delivery's manifest against its budgets and return the total size. The same checks
/// run at the receiving Node before anything is sent and at the Home before anything is staged.
pub fn validate(purpose: &Purpose, files: &[File], share: Limits) -> Result<u64> {
	let limits = Limits::of(purpose, share);
	if files.is_empty() || files.len() > limits.files {
		return Err(Error::Invalid("SHARE_FILE_LIMIT".into()));
	}
	let mut paths = std::collections::BTreeSet::new();
	let mut total = 0_u64;
	for file in files {
		crate::registry::rules::validate_path(&file.path)?;
		if !paths.insert(file.path.as_str())
			|| file.size > limits.file_bytes
			|| file.digest.len() != 64
			|| !file
				.digest
				.bytes()
				.all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
		{
			return Err(Error::Invalid("SHARE_FILE_LIMIT".into()));
		}
		total = total.saturating_add(file.size);
	}
	if total > limits.bytes {
		return Err(Error::Invalid("SHARE_BYTE_LIMIT".into()));
	}
	if let Purpose::Message {
		content,
		descriptions,
	} = purpose
	{
		// A message that carries files may have no text, as a local one may; its length is
		// bounded as a message's is.
		if content.len() > 64_000 {
			return Err(Error::Invalid(
				"message must contain 1 to 64000 bytes".into(),
			));
		}
		if descriptions
			.keys()
			.any(|path| !paths.contains(path.as_str()))
		{
			return Err(Error::Invalid(
				"an image description names a file that is not attached".into(),
			));
		}
		// As the local tool: an Agent describes every image it posts.
		if let Some(file) = files.iter().find(|file| {
			file.media_type.starts_with("image/") && !descriptions.contains_key(&file.path)
		}) {
			return Err(Error::Invalid(format!(
				"{} is an image: give it an Image Description in image_descriptions",
				file.path
			)));
		}
	}
	Ok(total)
}
fn namespaced(tag: &str, parts: &[&[u8]]) -> Uuid {
	let mut hash = Sha256::new();
	hash.update(tag.as_bytes());
	hash.update([0]);
	for part in parts {
		hash.update((part.len() as u64).to_be_bytes());
		hash.update(part);
	}
	let digest = hash.finalize();
	let mut bytes = [0; 16];
	bytes.copy_from_slice(&digest[..16]);
	Uuid::from_bytes(bytes)
}
/// The delivery a Run's call key names. A repeated request with the same key finds the same
/// delivery, so a lost reply is answered from what the Home already holds.
pub fn delivery_id(admission: Uuid, key: &str) -> Uuid {
	namespaced(
		"aidash-home-delivery-v1",
		&[admission.as_bytes(), key.as_bytes()],
	)
}
/// The key of one part of a delivery's effect (a Thread Attachment, then the message), so a retried
/// commit stores each part once.
pub fn part_key(id: Uuid, index: usize) -> Uuid {
	namespaced(
		"aidash-home-delivery-part-v1",
		&[id.as_bytes(), &(index as u64).to_be_bytes()],
	)
}
/// What a delivery's input is: the grant, the admission, the key, the purpose and the manifest.
/// A request that reuses the key with different input is refused.
pub fn input_digest(
	grant: Uuid,
	admission: Uuid,
	key: &str,
	purpose: &Purpose,
	files: &[File],
) -> String {
	crate::registry::rules::digest(&json!([
		"home_delivery",
		grant,
		admission,
		key,
		purpose,
		files
	]))
}
/// Whether the Home's reply is the committed receipt of this delivery: its id, and for every
/// file of the manifest, in order, the digest and size that were sent. The receiving Node trusts
/// nothing less before it tells the Run its files arrived.
pub fn receipt_matches(id: Uuid, files: &[File], reply: &serde_json::Value) -> bool {
	let Some(stored) = reply["receipt"]["files"].as_array() else {
		return false;
	};
	reply["state"] == "committed"
		&& reply["transfer_id"] == json!(id)
		&& stored.len() == files.len()
		&& stored
			.iter()
			.zip(files)
			.all(|(got, want)| got["digest"] == want.digest && got["size"] == want.size)
}
/// The manifest entries a delivery stages. Their ids derive from the delivery, so the manifest
/// digest is the same on every request that names the delivery.
pub fn manifest(id: Uuid, files: &[File]) -> Vec<MountedFile> {
	files
		.iter()
		.enumerate()
		.map(|(index, file)| MountedFile {
			file_id: namespaced(
				"aidash-home-delivery-file-v1",
				&[id.as_bytes(), &(index as u64).to_be_bytes()],
			),
			path: file.path.clone(),
			digest: file.digest.clone(),
			size: file.size,
			media_type: file.media_type.clone(),
			scope: FileScope::Received,
			provenance: json!({}),
		})
		.collect()
}
/// Who sends and where the Home puts it, all taken from the grant by the Home.
pub struct Parties {
	/// The receiving Node, as the Home names it.
	pub source_node: String,
	pub tenant: String,
	/// The subject whose authority the delivery is made under.
	pub subject: String,
	/// The Agent version executing the remote Run.
	pub agent: EntityRef,
	/// Where the Home stages the files: the Working Area of the Run that delegated for a file
	/// share; the executing Agent version and the grant's Task for a message, which the Home posts
	/// in the grant's Workspace.
	pub target: Recipient,
}
/// The description the receiving-side `file-transfer/1` code stores for a staged delivery.
pub fn description(
	id: Uuid,
	input_digest: String,
	files: &[File],
	parties: Parties,
	expires_at: DateTime<Utc>,
) -> Description {
	let entries = manifest(id, files);
	Description {
		protocol: "file-transfer/1".into(),
		transfer_id: id,
		source_node: parties.source_node,
		target: parties.target,
		source_tenant: parties.tenant,
		source_subject: parties.subject,
		source_agent: parties.agent,
		input_digest,
		manifest_digest: crate::registry::rules::digest(&json!(entries)),
		files: entries,
		expires_at,
	}
}
#[cfg(test)]
mod tests;
