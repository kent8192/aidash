//! Snapshot delivery checks both parties' current source authority before publishing a receipt.
use crate::{
	Error, Result,
	capabilities::files,
	ports::capabilities::sharing::{Limits, SharingScope},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		operations::{FileScope, MountedFile as FileEntry, available},
		sessions::Area,
		sharing::{Selection, Share},
	},
	registry::{AgentConfig, EntityRef},
};
use serde_json::{Value, json};
use uuid::Uuid;
/// Snapshot selection is immutable; no allocation occurs before all budgets pass.
pub fn select_files(
	available: &[FileEntry],
	selections: &[Selection],
	limits: &Limits,
) -> Result<(Vec<FileEntry>, u64)> {
	let mut chosen = vec![];
	let mut seen = std::collections::BTreeSet::new();
	let mut size = 0;
	for selection in selections {
		let file = available
			.iter()
			.find(|f| f.file_id == selection.file_id)
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
	if input.recipient.node_id != scope.node_id() {
		return scope.remote_prepare(run, area, input, &digest).await;
	}
	if input.expected_revision != area.revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
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
		aidash_domain::qualified_agent(
			scope.node_id(),
			&input.recipient.agent_id,
			&input.recipient.agent_version,
		),
	]);
	let received_authority = async {
		let workspace = scope.workspace(recipient.workspace_id).await?;
		scope.require(&workspace, "workspace.read").await?;
		let entry = scope
			.entry(
				&EntityRef {
					id: input.recipient.agent_id.clone(),
					version: input.recipient.agent_version.clone(),
				},
				"agent.execute",
			)
			.await?;
		let config: AgentConfig = serde_json::from_value(entry.config)?;
		if !config.core_capabilities.sharing {
			return Err(Error::Forbidden);
		}
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
	if entries.iter().map(|f| f.size).sum::<u64>() + size > scope.limits()?.working_bytes
		|| entries.len() + chosen.len() > 4096
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
			provenance: json!({"kind":"received","transfer_id":id,"sender":{"node_id":scope.node_id(),"agent_id":run.agent_id,"agent_version":run.agent_version},"source_digest":file.digest}),
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
		"agent":aidash_domain::qualified_agent(
			scope.node_id(),
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
