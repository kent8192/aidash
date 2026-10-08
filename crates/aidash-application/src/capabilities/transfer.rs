//! Sender snapshots and file-transfer/1 retries retain authority leases and never republish an acknowledged effect.
use crate::{
	Error, Result,
	ports::capabilities::transfer::{TransferRepository, TransferScope},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		operations::MountedFile as FileEntry,
		records::Record,
		sessions::Area,
		sharing::Share,
		transfer::{Chunk, Description, Identity, Requester, receipt_matches},
	},
	registry::EntityRef,
};
use base64::Engine;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;
pub async fn finish<T>(scope: Box<dyn TransferScope + '_>, result: Result<T>) -> Result<T> {
	match result {
		Ok(value) => scope.finish(Ok(())).await.map(|()| value),
		Err(error) => match scope.finish(Err(error)).await {
			Err(error) => Err(error),
			Ok(()) => Err(Error::External(
				"transfer scope completion invariant".into(),
			)),
		},
	}
}
pub async fn view(scope: &mut dyn TransferScope, id: Uuid) -> Result<Value> {
	let record = scope.record(id, "transfer_out").await?;
	if record.owner != scope.principal() {
		return Err(Error::NotFound("transfer unavailable".into()));
	}
	let workspace = scope
		.workspace(serde_json::from_value(record.data["workspace_id"].clone())?)
		.await?;
	scope.require(&workspace, "workspace.read").await?;
	scope
		.require(
			&scope.resource(
				"working_area",
				&record.area_id.ok_or(Error::Forbidden)?.to_string(),
				json!({"owner":record.owner}),
			),
			"file.read",
		)
		.await?;
	Ok(
		json!({"operation_id":id,"transfer_id":id,"status":if record.state=="delivered"{"completed"}else{record.state.as_str()},"recipient":record.data["description"]["target"],"manifest_digest":record.data["description"]["manifest_digest"],"receipt":record.data["receipt"],"error":aidash_domain::capabilities::errors::CapabilityError::stored(&record.data["error"]),"effects_may_have_occurred":record.data["commit_attempted"]==true}),
	)
}
pub async fn prepare(
	scope: &mut dyn TransferScope,
	run: &RunMetadata,
	area: &Area,
	input: Share,
	digest: &str,
) -> Result<Value> {
	if !scope.limits()?.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if input.expected_revision != area.revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	scope.authorize(area, "file.share").await?;
	scope
		.require(
			&scope.resource("node", &input.recipient.node_id, json!({})),
			"file.transfer",
		)
		.await?;
	// Private originals/text cannot be exported by adding a filename alias.
	if area
		.constraints
		.as_array()
		.ok_or(Error::Forbidden)?
		.iter()
		.any(|c| {
			matches!(
				c["kind"].as_str(),
				Some("reference" | "reference_text" | "received_scope")
			)
		}) {
		return Err(Error::Forbidden);
	}
	let original_subjects = scope.subjects().to_vec();
	let mut delegated = scope.subjects().to_vec();
	delegated.push(aidash_domain::qualified_agent(
		&input.recipient.node_id,
		&input.recipient.agent_id,
		&input.recipient.agent_version,
	));
	scope.set_subjects(delegated);
	let recipient_permission = async {
		scope.authorize(area, "file.share").await?;
		scope.sources(area.workspace_id, &area.constraints).await
	}
	.await;
	scope.set_subjects(original_subjects);
	recipient_permission?;
	let available = serde_json::from_value::<Vec<FileEntry>>(area.manifest.clone())?;
	let mut chosen = vec![];
	let mut seen = std::collections::BTreeSet::new();
	let mut total = 0;
	for selection in &input.files {
		let file = available
			.iter()
			.find(|f| f.file_id == selection.file_id)
			.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
		if file.digest != selection.expected_digest {
			return Err(Error::Conflict("FILE_CHANGED".into()));
		}
		if !seen.insert(file.file_id) || file.size > scope.limits()?.share_file_bytes {
			return Err(Error::Invalid("SHARE_FILE_LIMIT".into()));
		}
		total += file.size;
		if total > scope.limits()?.share_bytes {
			return Err(Error::Invalid("SHARE_BYTE_LIMIT".into()));
		}
		chosen.push(scope.copy_snapshot(file).await?);
	}
	let id = Uuid::new_v4();
	let expires_at = Utc::now() + Duration::seconds(scope.limits()?.staging_seconds as i64);
	let description = Description {
		protocol: "file-transfer/1".into(),
		transfer_id: id,
		source_node: scope.node_id().to_owned(),
		target: input.recipient,
		source_tenant: scope.tenant().to_owned(),
		source_subject: scope.principal().to_owned(),
		source_agent: EntityRef {
			id: run.agent_id.clone(),
			version: run.agent_version.clone(),
		},
		input_digest: digest.into(),
		manifest_digest: aidash_domain::registry::rules::digest(&json!(chosen)),
		files: chosen,
		expires_at,
	};
	scope.insert(id,Some(area.id),"transfer_out","pending",json!({"description":description,"credential_id":scope.credential(),"subjects":scope.subjects(),"workspace_id":area.workspace_id,"thread_id":area.thread_id,"constraints":area.constraints,"run_id":run.id,"offsets":{}}),Some(expires_at)).await?;
	scope
		.cache(
			input.idempotency_key,
			digest,
			&json!({"remote_transfer_id":id}),
		)
		.await?;
	view(scope, id).await
}
pub async fn authority<'a>(
	repository: &'a dyn TransferRepository,
	record: &Record,
) -> Result<Box<dyn TransferScope + 'a>> {
	if !repository.limits().admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	let mut scope = repository.begin_sender(record).await?;
	let result = async {
		scope.set_subjects(serde_json::from_value(record.data["subjects"].clone())?);
		let description: Description = serde_json::from_value(record.data["description"].clone())?;
		if record.expires_at.is_none_or(|t| t <= Utc::now()) {
			return Err(Error::Conflict("TRANSFER_EXPIRED".into()));
		}
        let peer_enabled: Option<bool> = scope.peer_enabled(&description.target.node_id).await?;
        if peer_enabled != Some(true) { return Err(Error::Forbidden); }
        let run = scope
            .run(serde_json::from_value(record.data["run_id"].clone())?)
			.await?;
		if run.control == aidash_domain::RunControl::Cancelled {
			return Err(Error::Forbidden);
		}
		let workspace = scope
			.workspace(serde_json::from_value(record.data["workspace_id"].clone())?)
			.await?;
		scope.set_context(workspace.attributes.clone());
		scope.require(&workspace, "workspace.read").await?;
		let area = record.area_id.ok_or(Error::Forbidden)?;
		scope
			.require(
				&scope.resource(
					"working_area",
					&area.to_string(),
					json!({"owner":record.owner,"agent_id":description.source_agent.id,"thread_id":record.data["thread_id"]}),
				),
				"file.share",
			)
			.await?;
		scope
			.require(
				&scope.resource("node", &description.target.node_id, json!({})),
				"file.transfer",
			)
			.await?;
		let entry = scope.entry( &description.source_agent, "agent.execute").await?;
		scope.check_pinned( &entry).await?;
        scope.require_bound_operation(run.id, "file_share").await?;
		let mut delegated=scope.subjects().to_vec();delegated.push(aidash_domain::qualified_agent(
			&description.target.node_id,
			&description.target.agent_id,
			&description.target.agent_version,
		));scope.set_subjects(delegated);
		scope
			.require(
				&scope.resource(
					"working_area",
					&area.to_string(),
					json!({"owner":record.owner,"agent_id":description.source_agent.id,"thread_id":record.data["thread_id"]}),
				),
				"file.share",
			)
			.await?;
		scope.sources( run.workspace_id, &record.data["constraints"])
			.await?;
		Ok(())
	}
	.await;
	if let Err(e) = result {
		return finish(scope, Err(e)).await;
	}
	Ok(scope)
}
pub async fn describe(
	repository: &dyn TransferRepository,
	node: &str,
	input: Identity,
) -> Result<Description> {
	let record = repository.snapshot(input.transfer_id).await?;
	let description: Description = serde_json::from_value(record.data["description"].clone())?;
	if description.target.node_id != node || description.input_digest != input.input_digest {
		return Err(Error::Forbidden);
	}
	// A queued policy writer may sit between the sender's shared lease and
	// this verification. Bound the callback so it fails closed and releases
	// the sender lease rather than deadlocking a revocation.
	let access = tokio::time::timeout(
		std::time::Duration::from_secs(2),
		authority(repository, &record),
	)
	.await
	.map_err(|_| Error::External("TRANSFER_AUTHORITY_UNAVAILABLE".into()))??;
	finish(access, Ok(description)).await
}
pub async fn delivered(
	repository: &dyn TransferRepository,
	id: Uuid,
	receipt: Value,
) -> Result<()> {
	let mut scope = repository.begin_receipt(id).await?;
	let result = async {
		let mut record = scope.load().await?;
		let description: Description = serde_json::from_value(record.data["description"].clone())?;
		if !receipt_matches(&description, &receipt) {
			return Err(Error::Conflict("INVALID_TRANSFER_RECEIPT".into()));
		};
		record.state = "delivered".into();
		record.data["receipt"] = receipt["receipt"].clone();
		record.data["error"] = Value::Null;
		scope.update(&mut record).await
	}
	.await;
	scope.finish(result).await
}

pub async fn reconcile(repository: &dyn TransferRepository, id: Uuid) -> Result<()> {
	let record = repository.snapshot(id).await?;
	let description: Description = serde_json::from_value(record.data["description"].clone())?;
	let response: Value = repository
		.request(
			&description.target.node_id,
			"/scoped/files/status",
			&json!(Identity {
				transfer_id: id,
				input_digest: description.input_digest.clone()
			}),
		)
		.await?;
	if response["state"] == "committed" {
		return delivered(repository, id, response).await;
	}
	if record.expires_at.is_none_or(|at| at <= Utc::now())
		|| record.data["objects_released"] == true
	{
		return Err(Error::Conflict("TRANSFER_EXPIRED".into()));
	}
	let mut scope = authority(repository, &record).await?;
	let mut record = scope.record(id, "transfer_out").await?;
	if record.state != "delivered" {
		record.state = if record.data["commit_attempted"] == true {
			"committing"
		} else {
			"transferring"
		}
		.into();
		record.data["retry_count"] = json!(0);
		record.data["retry_after"] = Value::Null;
		let result = scope.update(&mut record).await;
		return finish(scope, result).await;
	}
	finish(scope, Ok(())).await
}
pub async fn drive(repository: &dyn TransferRepository, id: Uuid) -> Result<()> {
	let snapshot = repository.snapshot(id).await?;
	let description: Description = serde_json::from_value(snapshot.data["description"].clone())?;
	let identity = Identity {
		transfer_id: id,
		input_digest: description.input_digest.clone(),
	};
	// A lost final reply is reconciled before considering another effect. This
	// never chooses a new transfer ID or republishes a receiver-owned snapshot.
	if snapshot.data["prepared"] == true {
		let status: Value = repository
			.request(
				&description.target.node_id,
				"/scoped/files/status",
				&json!(identity),
			)
			.await?;
		if status["state"] == "committed" {
			return delivered(repository, id, status).await;
		}
	}
	if snapshot.data["prepared"] != true {
		let scope = authority(repository, &snapshot).await?;
		finish(scope, Ok(())).await?;
		let negotiation: Value = repository
			.request(
				&description.target.node_id,
				"/scoped/files/negotiate",
				&json!(Requester {
					tenant: description.source_tenant.clone(),
					subject: description.source_subject.clone(),
					cursor: None,
				}),
			)
			.await?;
		if negotiation["protocol"] != "file-transfer/1"
			|| negotiation["node_id"] != description.target.node_id
			|| negotiation["chunk_bytes"] != 4194304
		{
			return Err(Error::Conflict("FILE_TRANSFER_UNAVAILABLE".into()));
		}
		let accepted: Value = repository
			.request(
				&description.target.node_id,
				"/scoped/files/prepare",
				&json!(identity),
			)
			.await?;
		if accepted["state"] == "committed" {
			return delivered(repository, id, accepted).await;
		}
		if accepted["state"] != "staging"
			|| accepted["input_digest"] != description.input_digest
			|| accepted["manifest_digest"] != description.manifest_digest
		{
			return Err(Error::Conflict("TRANSFER_ADMISSION_CHANGED".into()));
		}
		let mut scope = authority(repository, &snapshot).await?;
		let mut record = scope.record(id, "transfer_out").await?;
		record.state = "transferring".into();
		record.data["prepared"] = json!(true);
		let result = scope.update(&mut record).await;
		return finish(scope, result).await;
	}
	let mut scope = authority(repository, &snapshot).await?;
	let result = Box::pin(async {
		let mut record = scope.record(id, "transfer_out").await?;
		for (index, file) in description.files.iter().enumerate() {
			let offset = record.data["offsets"][index.to_string()]
				.as_u64()
				.unwrap_or(0);
			if offset < file.size {
				let bytes = scope.read_chunk(file, offset).await?;
				if bytes.is_empty() {
					return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
				}
				let accepted: Value = repository
					.request(
						&description.target.node_id,
						"/scoped/files/chunk",
						&json!(Chunk {
							transfer_id: id,
							input_digest: description.input_digest.clone(),
							file: index,
							offset,
							data: base64::engine::general_purpose::STANDARD.encode(&bytes)
						}),
					)
					.await?;
				if accepted["state"] != "staging"
					|| accepted["input_digest"] != description.input_digest
				{
					return Err(Error::Conflict("TRANSFER_CHUNK_UNCONFIRMED".into()));
				}
				record.data["offsets"][index.to_string()] = json!(offset + bytes.len() as u64);
				return scope.update(&mut record).await;
			}
		}
		if record.data["commit_attempted"] != true {
			record.data["commit_attempted"] = json!(true);
			record.state = "committing".into();
			return scope.update(&mut record).await;
		}
		// Hold the current source lease through receiver commit. Both ends
		// check their original identities; model input never supplies a token.
		let receipt: Value = repository
			.request(
				&description.target.node_id,
				"/scoped/files/commit",
				&json!(identity),
			)
			.await?;
		if !receipt_matches(&description, &receipt) {
			return Err(Error::Conflict("INVALID_TRANSFER_RECEIPT".into()));
		}
		record.state = "delivered".into();
		record.data["receipt"] = receipt["receipt"].clone();
		record.data["error"] = Value::Null;
		scope.update(&mut record).await
	})
	.await;
	finish(scope, result).await
}
pub async fn sweep(repository: &dyn TransferRepository) -> Result<()> {
	for id in repository.jobs().await? {
		if let Err(error) = Box::pin(drive(repository, id)).await {
			let terminal = matches!(
				error,
				Error::Forbidden | Error::Unauthorized | Error::NotFound(_)
			);
			repository.record_failure(id, terminal).await?;
			tracing::warn!(%id,%error,"file transfer not acknowledged");
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests;

pub mod receiver;
