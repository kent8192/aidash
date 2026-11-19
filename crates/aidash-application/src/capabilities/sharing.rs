//! Snapshot delivery checks both parties' current source authority before publishing a receipt.
use crate::{
	Error, Result,
	capabilities::files,
	ports::capabilities::sharing::{HomeShareScope, Limits, SharingScope},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		operations::{FileScope, MountedFile as FileEntry, available},
		sessions::Area,
		sharing::{Selection, Share},
	},
	registry::EntityRef,
};
use serde_json::{Value, json};
use sha2::Digest as _;
use uuid::Uuid;
/// The digests no file of `available` may leave the Run with. A Reference file can be read by a
/// Run but never leaves it, and neither can a copy of it as made: the recipient could download
/// what only the uploader may (ADR 0114). A copy is known by its provenance or, once copied
/// again with the same bytes, by its digest. An edited copy is its editor's own file.
pub(crate) fn reference_digests(available: &[FileEntry]) -> std::collections::BTreeSet<&str> {
	available
		.iter()
		.filter(|f| f.provenance["kind"] == "reference")
		.map(|f| f.digest.as_str())
		.collect()
}

/// Refuse an Area whose constraints bar its files from leaving it ([`BARRING`]).
pub(crate) fn require_unbarred(area: &Area) -> Result<()> {
	if area
		.constraints
		.as_array()
		.ok_or(Error::Forbidden)?
		.iter()
		.any(|c| c["kind"].as_str().is_some_and(|k| BARRING.contains(&k)))
	{
		return Err(Error::Forbidden);
	}
	Ok(())
}

/// Snapshot selection is immutable; no allocation occurs before all budgets pass.
pub fn select_files(
	available: &[FileEntry],
	selections: &[Selection],
	limits: &Limits,
) -> Result<(Vec<FileEntry>, u64)> {
	let mut chosen = vec![];
	let mut seen = std::collections::BTreeSet::new();
	let mut size = 0;
	let reference_digests = reference_digests(available);
	for selection in selections {
		let file = available
			.iter()
			.find(|f| {
				f.file_id == selection.file_id && !reference_digests.contains(f.digest.as_str())
			})
			.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
		if file.digest != selection.expected_digest {
			return Err(Error::Conflict(format!("FILE_CHANGED: {}", file.path)));
		}
		if !seen.insert(file.file_id) || file.size > limits.file_bytes {
			return Err(Error::Invalid("SHARE_FILE_LIMIT".into()));
		}
		size += file.size;
		chosen.push(file.clone());
	}
	if size > limits.bytes {
		return Err(Error::Invalid("SHARE_BYTE_LIMIT".into()));
	}
	Ok((chosen, size))
}

/// What a Run executing for another Home sends back: its snapshot, read whole, and the input
/// digest the call is journaled under.
pub struct HomeDelivery {
	pub digest: String,
	pub files: Vec<(FileEntry, Vec<u8>)>,
	pub total: u64,
}
pub enum HomeShare {
	/// The same call already ran: its stored result.
	Replay(Value),
	Deliver(Box<HomeDelivery>),
}
/// Constraints of a Working Area that bar it from sending files out (private originals and text,
/// and files received from other Agents). A Reference set is not one of them for the Home, which
/// the receiving Tenant consented to by listing the tool; the Reference files themselves never
/// leave, as `select_files` keeps them out.
const BARRING: [&str; 3] = ["reference", "reference_text", "received_scope"];
/// `file_share` of a Run executing for another Home, to that Home (ADR 0139, decision 20). Runs
/// in the Run's own transaction and reads every selected file; sending them to the Home is the
/// caller's, outside the transaction, and then recording the result under the call's key.
pub async fn home_prepare(
	scope: &mut dyn HomeShareScope,
	run: &RunMetadata,
	area: &Area,
	input: &Share,
) -> Result<HomeShare> {
	let limits = scope.share_limits()?;
	if input.files.is_empty() || input.files.len() > limits.files {
		return Err(Error::Invalid("SHARE_FILE_LIMIT".into()));
	}
	let digest = aidash_domain::registry::rules::digest(&json!(["file_share", run.id, input]));
	if let Some(previous) = scope.replay(input.idempotency_key, &digest).await? {
		return Ok(HomeShare::Replay(previous));
	}
	if !limits.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if scope.active_run(area).await? != Some(run.id) {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	// A Home's Runs send to that Home only: the Area's own `remote_home` and the Run's say so.
	if !area.is_remote_home()
		|| !area.may_send_to(&input.recipient.node_id)
		|| run.home_node != input.recipient.node_id
	{
		return Err(Error::Forbidden);
	}
	if input.expected_revision != area.revision {
		return Err(Error::Conflict(
			aidash_domain::capabilities::errors::area_revision_changed(area.revision),
		));
	}
	scope.permit(area, "file.share").await?;
	require_unbarred(area)?;
	let mounted: Vec<FileEntry> = serde_json::from_value(area.manifest.clone())?;
	let (chosen, total) = select_files(&mounted, &input.files, &limits)?;
	let mut files = Vec::with_capacity(chosen.len());
	for file in chosen {
		let mut bytes = Vec::with_capacity(usize::try_from(file.size).unwrap_or(0));
		while (bytes.len() as u64) < file.size {
			let chunk = scope.read_range(&file, bytes.len() as u64).await?;
			if chunk.is_empty() {
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			bytes.extend_from_slice(&chunk);
		}
		// The object store verified the bytes on read; the digest is checked again because the
		// Home stores nothing that does not match the manifest it was sent.
		if bytes.len() as u64 != file.size
			|| format!("{:x}", sha2::Sha256::digest(&bytes)) != file.digest
		{
			return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
		}
		files.push((file, bytes));
	}
	Ok(HomeShare::Deliver(Box::new(HomeDelivery {
		digest,
		files,
		total,
	})))
}
/// The result a `file_share` to the Home reports, in the shape of a local share's: the Home's
/// receipt names the delivered files and the Area they landed in.
pub fn home_result(
	id: Uuid,
	area: &Area,
	input: &Share,
	manifest_digest: &str,
	receipt: Value,
) -> Value {
	json!({"operation_id":id,"snapshot_id":id,"transfer_id":id,"status":"completed","manifest_digest":manifest_digest,"recipient":input.recipient,"receipt":receipt,"revision":area.revision,"generation":area.generation})
}

pub async fn share(
	scope: &mut dyn SharingScope,
	run: &RunMetadata,
	area: &Area,
	input: Share,
) -> Result<Value> {
	if input.files.is_empty() || input.files.len() > scope.sharing_limits()?.files {
		return Err(Error::Invalid("SHARE_FILE_LIMIT".into()));
	}
	let digest = aidash_domain::registry::rules::digest(&json!(["file_share", run.id, input]));
	if let Some(previous) = scope.cached(input.idempotency_key, &digest).await? {
		if let Some(id) = previous["remote_transfer_id"].as_str() {
			return scope
				.remote_view(id.parse().map_err(|_| Error::Forbidden)?)
				.await;
		}
		return Ok(previous);
	}
	if !scope.sharing_limits()?.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if scope.current_run(area).await? != Some(run.id) {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	// An Area that belongs to a Home's Runs sends files to that Home only, whether or not it
	// holds a Reference set (ADR 0114 amendment, ADR 0139), and only through `home_prepare`:
	// the Home is served by an admission-bound delivery, never by this scope, and no other Node
	// is ever a target.
	if area.is_remote_home() {
		return Err(Error::Forbidden);
	}
	let own = aidash_domain::nodes::DeploymentId::parse(scope.node_id())?
		.node(run.node.clone())
		.to_string();
	if input.recipient.node_id != own {
		return scope.remote_prepare(run, area, input, &digest).await;
	}
	if input.expected_revision != area.revision {
		return Err(Error::Conflict(
			aidash_domain::capabilities::errors::area_revision_changed(area.revision),
		));
	}
	scope.authorize(area, "file.share").await?;
	let mounted: Vec<FileEntry> = serde_json::from_value(area.manifest.clone())?;
	let (chosen, size) = select_files(&mounted, &input.files, &scope.sharing_limits()?)?;
	let mut recipient: Area = scope
		.recipient(area, &input.recipient)
		.await?
		.ok_or_else(|| Error::NotFound("recipient unavailable".into()))?;
	if recipient.id == area.id {
		return Err(Error::Invalid("SHARE_REQUIRES_OTHER_AGENT".into()));
	}
	available(&recipient.state)?;
	// Receipt must name a version actually admitted in this destination, not
	// merely a discoverable Registry identity.
	let admitted: Option<Uuid> = scope
		.admitted(&recipient, &input.recipient.agent_version)
		.await?;
	if admitted.is_none() {
		return Err(Error::NotFound("recipient unavailable".into()));
	}
	let subjects = scope.subjects().to_vec();
	scope.set_subjects(vec![
		recipient.owner.clone(),
		aidash_domain::agent_subject(
			&recipient.home_node,
			&input.recipient.agent_id,
			&input.recipient.agent_version,
		),
	]);
	let received_authority = async {
		let workspace = scope.workspace(recipient.workspace_id).await?;
		scope.require(&workspace, "workspace.read").await?;
		let _entry = scope
			.entry(
				&EntityRef {
					id: input.recipient.agent_id.clone(),
					version: input.recipient.agent_version.clone(),
				},
				"agent.execute",
			)
			.await?;
		scope
			.require_bound_operation(admitted.expect("checked admitted recipient"), "file_share")
			.await?;
		let resource = scope.resource(
			"working_area",
			&recipient.id.to_string(),
			json!({"owner":recipient.owner,"agent_id":recipient.agent_id,"thread_id":recipient.thread_id}),
		);
		scope.require(&resource, "file.receive").await?;
		// Both the existing recipient data and the incoming source constraints
		// must be readable by every recipient subject at this commit boundary.
		scope
			.source_authority(recipient.workspace_id, &recipient.constraints)
			.await?;
		scope
			.source_authority(area.workspace_id, &area.constraints)
			.await
	}
	.await;
	scope.set_subjects(subjects);
	received_authority?;
	let id = Uuid::new_v4();
	let mut entries = serde_json::from_value::<Vec<FileEntry>>(recipient.manifest.clone())?;
	if aidash_domain::capabilities::operations::counted_files(&entries)
		.map(|f| f.size)
		.sum::<u64>()
		+ size + scope.retained_bytes(&recipient).await?
		> scope.limits()?.working_bytes
		|| aidash_domain::capabilities::operations::counted_files(&entries).count() + chosen.len()
			> 4096
	{
		return Err(Error::Conflict("RECIPIENT_QUOTA".into()));
	}
	let manifest_digest = aidash_domain::registry::rules::digest(&json!(chosen));
	let mut delivered = vec![];
	for file in &chosen {
		scope.begin_received(recipient.id, file.size).await?;
		let mut offset = 0;
		while offset < file.size {
			let bytes = scope.read_chunk(file, offset).await?;
			if bytes.is_empty() {
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			offset += bytes.len() as u64;
			scope.write_pending(&bytes).await?;
		}
		let (file_id, hash) = scope.finish_pending(&file.digest).await?;
		let path = format!("{id}/{}", file.path);
		aidash_domain::registry::rules::validate_path(&path)?;
		let received = FileEntry {
			file_id,
			path,
			digest: hash,
			size: file.size,
			media_type: file.media_type.clone(),
			scope: FileScope::Received,
			provenance: json!({"kind":"received","transfer_id":id,"sender":{"node_id":own,"agent_id":run.agent_id,"agent_version":run.agent_version},"source_digest":file.digest}),
		};
		delivered.push(received.clone());
		entries.push(received);
	}
	recipient.manifest = json!(entries);
	let constraints = recipient
		.constraints
		.as_array_mut()
		.ok_or(Error::Forbidden)?;
	for source in area.constraints.as_array().ok_or(Error::Forbidden)? {
		if !constraints.contains(source) {
			constraints.push(source.clone());
		}
	}
	constraints.push(json!({
		"kind":"received_scope",
		"transfer_id":id,
		"owner":recipient.owner,
		"agent":aidash_domain::agent_subject(
			&recipient.home_node,
			&input.recipient.agent_id,
			&input.recipient.agent_version,
		),
	}));
	files::publish(scope, &mut recipient).await?;
	let result = json!({"operation_id":id,"snapshot_id":id,"transfer_id":id,"status":"completed","manifest_digest":manifest_digest,"recipient":input.recipient,"receipt":{"id":id,"area_id":recipient.id,"revision":recipient.revision,"files":delivered},"revision":area.revision,"generation":area.generation});
	scope.collaboration(id, area.id, result.clone()).await?;

	scope.cache(input.idempotency_key, &digest, &result).await?;
	scope.event(area.workspace_id,"capability.files_shared",json!({"transfer_id":id,"sender_area":area.id,"recipient_area":recipient.id,"manifest_digest":manifest_digest})).await?;
	Ok(result)
}
#[cfg(test)]
mod tests;
