//! Receiver authority callbacks precede local locks; quota, chunks and final receipt commit atomically.
use super::finish;
use crate::{
	Error, Result,
	ports::capabilities::transfer::receiver::{ReceiverRepository, ReceiverScope},
};
use aidash_domain::{
	capabilities::{
		operations::{FileScope, MountedFile as FileEntry, available},
		records::Record,
		sessions::Area,
		transfer::{
			Chunk, Description, Identity, Requester,
			receiver::{ManifestLimits, cap_recipient_versions, chunk_digest, validate, view},
		},
	},
	registry::{AgentConfig, EntityRef},
};
use base64::Engine;
use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;
pub fn require_admission(repository: &dyn ReceiverRepository) -> Result<()> {
	if !repository.limits().admission {
		Err(Error::Conflict("CAPABILITIES_DISABLED".into()))
	} else {
		Ok(())
	}
}
fn manifest_limits(repository: &dyn ReceiverRepository) -> ManifestLimits {
	let l = repository.limits();
	ManifestLimits {
		files: l.share_files,
		file_bytes: l.share_file_bytes,
		bytes: l.share_bytes,
	}
}
async fn mapped<'a>(
	repository: &'a dyn ReceiverRepository,
	source: &str,
	description: &Description,
) -> Result<(Box<dyn ReceiverScope + 'a>, Area)> {
	let mut scope = repository
		.begin_peer(
			source,
			&description.source_tenant,
			&description.source_subject,
		)
		.await?;
	let result = async {
		if description.protocol != "file-transfer/1"
			|| description.source_node != source
			|| description.target.node_id != repository.node_id()
		{
			return Err(Error::Forbidden);
		}
		scope
			.require(
				&scope.resource("node", &repository.node_id(), json!({})),
				"file.transfer",
			)
			.await?;
		let mut subjects = scope.subjects().to_vec();
		subjects.push(aidash_domain::qualified_agent(
			&repository.node_id(),
			&description.target.agent_id,
			&description.target.agent_version,
		));
		scope.set_subjects(subjects);
		let area: Area = scope
			.recipient(description, repository.node_id())
			.await?
			.ok_or_else(|| Error::NotFound("recipient unavailable".into()))?;
		scope.authorize(&area, "file.receive").await?;
		scope
			.require(
				&scope.resource("node", &repository.node_id(), json!({})),
				"file.transfer",
			)
			.await?;
		let entry = scope
			.entry(
				&EntityRef {
					id: description.target.agent_id.clone(),
					version: description.target.agent_version.clone(),
				},
				"agent.execute",
			)
			.await?;
		scope.check_pinned(&entry).await?;
		let config: AgentConfig = serde_json::from_value(entry.config)?;
		if !config.core_capabilities.sharing {
			return Err(Error::Forbidden);
		}
		let admitted: Option<Uuid> = scope
			.admitted(&area, &description.target.agent_version)
			.await?;
		if admitted.is_none() {
			return Err(Error::NotFound("recipient version unavailable".into()));
		}
		Ok(area)
	}
	.await;
	match result {
		Ok(area) => Ok((scope, area)),
		Err(e) => finish(scope, Err(e)).await,
	}
}
async fn current<'a>(
	repository: &'a dyn ReceiverRepository,
	source: &str,
	input: &Identity,
) -> Result<(Box<dyn ReceiverScope + 'a>, Area, Record, Description)> {
	let record: Record = repository.incoming(input.transfer_id).await?;
	let description: Description = serde_json::from_value(record.data["description"].clone())?;
	if description.source_node != source || description.input_digest != input.input_digest {
		return Err(Error::Forbidden);
	}
	let (mut scope, area) = mapped(repository, source, &description).await?;
	let record = scope.record(input.transfer_id, "transfer_in").await?;
	if record.data["mapped_credential"] != json!(scope.credential()) {
		return Err(Error::Forbidden);
	}
	Ok((scope, area, record, description))
}
pub async fn negotiate(
	repository: &dyn ReceiverRepository,
	node: &str,
	input: Requester,
) -> Result<Value> {
	let mut scope = repository
		.begin_peer(node, &input.tenant, &input.subject)
		.await?;
	let result = scope
		.require(
			&scope.resource("node", &repository.node_id(), json!({})),
			"file.transfer",
		)
		.await;
	finish(scope, result).await?;
	if !repository.limits().admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	Ok(
		json!({"protocol":"file-transfer/1","node_id":repository.node_id(),"chunk_bytes":4194304,"maximum_files":repository.limits().share_files,"maximum_file_bytes":repository.limits().share_file_bytes,"maximum_total_bytes":repository.limits().share_bytes}),
	)
}
pub async fn prepare(
	repository: &dyn ReceiverRepository,
	source: &str,
	input: Identity,
) -> Result<Value> {
	if !repository.limits().admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	// No receiver policy locks are held across the callback to source authority.
	let description: Description = serde_json::from_value(
		repository
			.request(source, "/scoped/files/describe", &json!(input))
			.await?,
	)?;
	if description.transfer_id != input.transfer_id
		|| description.input_digest != input.input_digest
	{
		return Err(Error::Forbidden);
	}
	let total = validate(&description, &manifest_limits(repository), Utc::now())?;
	let (mut scope, area) = mapped(repository, source, &description).await?;
	let result=async {
        match scope.record(input.transfer_id,"transfer_in").await {
            Ok(record) => {
                if record.data["description"]!=json!(description)||record.data["mapped_credential"]!=json!(scope.credential()) {return Err(Error::Conflict("TRANSFER_IDENTITY_CHANGED".into()));}
                return Ok(view(&record));
            },
            Err(Error::NotFound(_)) => {},
            Err(error) => return Err(error),
        }
        available(&area.state)?;
        if serde_json::from_value::<Vec<FileEntry>>(area.manifest.clone())?.iter().map(|f|f.size).sum::<u64>()+total>scope.limits()?.working_bytes {return Err(Error::Conflict("RECIPIENT_QUOTA".into()));}
        // Reserve both provisional chunks and final immutable objects. Actual
        // allocation consumes this reservation transactionally, never twice.
        scope.reserve((2*total) as i64).await?;
        let data=json!({"description":description,"generation":area.generation,"mapped_credential":scope.credential(),"chunks":{},"reserved":2*total});
        let record=scope.insert(input.transfer_id,Some(area.id),"transfer_in","staging",data,Some(description.expires_at)).await?;
        Ok(view(&record))
    }.await;
	finish(scope, result).await
}
pub async fn chunk(
	repository: &dyn ReceiverRepository,
	source: &str,
	input: Chunk,
) -> Result<Value> {
	if !repository.limits().admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	let identity = Identity {
		transfer_id: input.transfer_id,
		input_digest: input.input_digest,
	};
	let (mut scope, area, mut record, description) = current(repository, source, &identity).await?;
	let result = async {
		if record.state != "staging"
			|| description.expires_at <= Utc::now()
			|| record.data["generation"] != area.generation
		{
			return Err(Error::Conflict("TRANSFER_UNAVAILABLE".into()));
		}
		let file = description
			.files
			.get(input.file)
			.ok_or_else(|| Error::Invalid("TRANSFER_FILE_INDEX".into()))?;
		let bytes = base64::engine::general_purpose::STANDARD
			.decode(&input.data)
			.map_err(|_| Error::Invalid("INVALID_CHUNK".into()))?;
		if input.offset >= file.size
			|| !input.offset.is_multiple_of(4194304)
			|| bytes.len() as u64 != (file.size - input.offset).min(4194304)
		{
			return Err(Error::Invalid("TRANSFER_CHUNK_RANGE".into()));
		}
		let key = format!("{}:{}", input.file, input.offset);
		if let Some(old) = record.data["chunks"].get(&key) {
			if old["digest"] != chunk_digest(&bytes) || old["size"] != bytes.len() {
				return Err(Error::Conflict("CHUNK_CHANGED".into()));
			}
			return Ok(view(&record));
		}
		let reserved = record.data["reserved"].as_i64().ok_or(Error::Forbidden)?;
		if reserved < bytes.len() as i64 {
			return Err(Error::Conflict("TRANSFER_RESERVATION_CHANGED".into()));
		}
		scope.reserve(-(bytes.len() as i64)).await?;
		let (file_id, digest) = scope.put_staging(&bytes).await?;
		record.data["chunks"][key] = json!(FileEntry {
			file_id,
			digest,
			path: "chunk".into(),
			size: bytes.len() as u64,
			media_type: "application/octet-stream".into(),
			scope: FileScope::Received,
			provenance: Value::Null
		});
		record.data["reserved"] = json!(reserved - bytes.len() as i64);
		scope.update(&mut record).await?;
		Ok(view(&record))
	}
	.await;
	finish(scope, result).await
}
pub async fn commit(
	repository: &dyn ReceiverRepository,
	source: &str,
	input: Identity,
) -> Result<Value> {
	if !repository.limits().admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	// Authenticate the original subject at its authority again, independently
	// of the transport credential. Never hold receiver locks for this callback.
	let fresh: Description = serde_json::from_value(
		repository
			.request(source, "/scoped/files/describe", &json!(input))
			.await?,
	)?;
	let (mut scope, mut area, mut record, description) =
		current(repository, source, &input).await?;
	if json!(fresh) != json!(description) {
		return Err(Error::Forbidden);
	}
	let result=async {
        if record.state=="committed" {return Ok(view(&record));}
        if record.state!="staging"||description.expires_at<=Utc::now()||record.data["generation"]!=area.generation {return Err(Error::Conflict("TRANSFER_UNAVAILABLE".into()));}
        // The source worker retains its current lease through this RPC and
        // the receiver retains its own lease through atomic publication.
        available(&area.state)?;
        let mut entries=serde_json::from_value::<Vec<FileEntry>>(area.manifest.clone())?;
        let total=description.files.iter().map(|f|f.size).sum::<u64>();
        if entries.len()+description.files.len()>4096||entries.iter().map(|f|f.size).sum::<u64>()+total>scope.limits()?.working_bytes {return Err(Error::Conflict("RECIPIENT_QUOTA".into()));}
        let mut delivered=vec![];
        for (index,file) in description.files.iter().enumerate() {
            let reserved=record.data["reserved"].as_i64().ok_or(Error::Forbidden)?;
            if reserved<file.size as i64 {return Err(Error::Conflict("TRANSFER_RESERVATION_CHANGED".into()));}
            scope.reserve(-(file.size as i64)).await?;
            record.data["reserved"]=json!(reserved-file.size as i64);
            scope.begin_received(area.id,file.size).await?;
            let mut offset=0;
            while offset<file.size {
                let chunk:FileEntry=serde_json::from_value(record.data["chunks"][format!("{index}:{offset}")].clone()).map_err(|_|Error::Conflict("TRANSFER_INCOMPLETE".into()))?;
                let bytes=scope.read_file(&chunk).await?;scope.write_pending(&bytes).await?;offset+=chunk.size;
            }
            let (file_id,digest)=scope.finish_pending(&file.digest).await?;
            let received=FileEntry{file_id,digest,path:format!("{}/{}",record.id,file.path),scope:FileScope::Received,provenance:json!({"kind":"received","transfer_id":record.id,"source_node":source,"source_agent":description.source_agent}),..file.clone()};
            aidash_domain::registry::rules::validate_path(&received.path)?;delivered.push(received.clone());entries.push(received);
        }
        area.manifest=json!(entries);
        area.constraints.as_array_mut().ok_or(Error::Forbidden)?.push(json!({"kind":"received_scope","transfer_id":record.id,"owner":area.owner,"agent":aidash_domain::qualified_agent(&repository.node_id(),&description.target.agent_id,&description.target.agent_version)}));
        scope.publish(&mut area).await?;
        record.state="committed".into();record.expires_at=None;
        record.data["receipt"]=json!({"id":record.id,"node_id":repository.node_id(),"area_id":area.id,"revision":area.revision,"manifest_digest":description.manifest_digest,"files":delivered});
        scope.update(&mut record).await?;
        scope.event(area.workspace_id,"capability.files_received",json!({"transfer_id":record.id,"area_id":area.id,"source_node":source})).await?;
        Ok(view(&record))
    }.await;
	finish(scope, result).await
}
pub async fn status(
	repository: &dyn ReceiverRepository,
	source: &str,
	input: Identity,
) -> Result<Value> {
	let record: Record = repository.status_snapshot(input.transfer_id).await?;
	let description: Description = serde_json::from_value(record.data["description"].clone())?;
	if description.source_node != source || description.input_digest != input.input_digest {
		return Err(Error::Forbidden);
	}
	// A durable receipt survives recipient cleanup and withdrawal of future
	// sharing permission. Its metadata is still scoped to the original mapped
	// subject; status never returns original bytes or creates a new binding.
	let scope = repository
		.begin_peer(
			source,
			&description.source_tenant,
			&description.source_subject,
		)
		.await?;
	if record.tenant != scope.tenant()
		|| record.owner != scope.principal()
		|| record.data["mapped_credential"] != json!(scope.credential())
	{
		return Err(Error::Forbidden);
	}
	finish(scope, Ok(view(&record))).await
}
pub async fn recipients(
	repository: &dyn ReceiverRepository,
	source: &str,
	input: Requester,
) -> Result<Value> {
	let mut scope = repository
		.begin_peer(source, &input.tenant, &input.subject)
		.await?;
	let result = recipient_list(&mut *scope, repository.node_id(), input.cursor).await;
	finish(scope, result).await
}
pub async fn recipient_list(
	scope: &mut dyn ReceiverScope,
	node: &str,
	cursor: Option<Uuid>,
) -> Result<Value> {
	scope
		.require(&scope.resource("node", &node, json!({})), "file.transfer")
		.await?;
	let rows: Vec<Area> = scope.recipient_rows(cursor).await?;
	let next_cursor = if rows.len() == 51 {
		Some(rows[49].id)
	} else {
		None
	};
	let mut items = vec![];
	let mut versions_truncated = false;
	for area in rows.into_iter().take(50) {
		match scope.authorize(&area, "file.receive").await {
			Ok(()) => {}
			Err(Error::Forbidden | Error::NotFound(_)) => continue,
			Err(error) => return Err(error),
		}
		let mut versions: Vec<String> = scope.versions(&area).await?;
		versions_truncated |= cap_recipient_versions(&mut versions);
		for version in versions {
			let subjects = scope.subjects().to_vec();
			let mut delegated = scope.subjects().to_vec();
			delegated.push(aidash_domain::qualified_agent(
				&node,
				&area.agent_id,
				&version,
			));
			scope.set_subjects(delegated);
			let allowed = async {
				scope.authorize(&area, "file.receive").await?;
				let entry = scope
					.entry(
						&EntityRef {
							id: area.agent_id.clone(),
							version: version.clone(),
						},
						"agent.execute",
					)
					.await?;
				if !serde_json::from_value::<AgentConfig>(entry.config)?
					.core_capabilities
					.sharing
				{
					return Err(Error::Forbidden);
				}
				Ok(())
			}
			.await;
			scope.set_subjects(subjects);
			match allowed {
                    Ok(()) => items.push(json!({"node_id":node,"agent_id":area.agent_id,"agent_version":version,"thread_id":area.thread_id,"workspace_id":area.workspace_id})),
                    Err(Error::Forbidden | Error::NotFound(_)) => {}, Err(error) => return Err(error),
                }
		}
	}
	Ok(
		json!({"protocol":"file-transfer/1","items":items,"next_cursor":next_cursor,"versions_truncated":versions_truncated}),
	)
}
