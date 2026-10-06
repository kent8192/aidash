//! Upload, immutable mounts, parser dispatch and receipt recovery share current authority.
use crate::{
	Error, Result,
	ports::capabilities::references::{ReferenceRepository, ReferenceScope},
};
use aidash_domain::{
	capabilities::{
		operations::{FileScope, MountedFile as FileEntry},
		records::Record,
		references::lifecycle::{self, Chunk, Reference, Upload, finish_extraction},
	},
	registry::AgentConfig,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;
pub async fn get(scope: &mut dyn ReferenceScope, id: Uuid, action: &str) -> Result<Record> {
	let record = scope.load(id).await?;
	if record.owner != scope.principal() || matches!(record.state.as_str(), "revoked" | "expired") {
		return Err(Error::NotFound("reference unavailable".into()));
	}
	scope
		.require(id, &record.owner, action)
		.await
		.map_err(|_| Error::NotFound("reference unavailable".into()))?;
	Ok(record)
}
pub async fn inspect(scope: &mut dyn ReferenceScope, id: Uuid) -> Result<Reference> {
	Ok(lifecycle::view(&get(scope, id, "reference.read").await?)?)
}
pub async fn start(scope: &mut dyn ReferenceScope, input: Upload) -> Result<Reference> {
	let limits = scope.limits()?;
	if !limits.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	lifecycle::validate_upload(&input, limits.reference_bytes)?;
	scope.require_upload().await?;
	let digest = aidash_domain::registry::rules::digest(&json!(["reference_upload", input]));
	if let Some(previous) = scope.cached(input.idempotency_key, &digest).await? {
		return inspect(
			scope,
			serde_json::from_value(previous["reference_id"].clone())?,
		)
		.await;
	}
	let id = Uuid::new_v4();
	let record = scope
		.insert(
			id,
			"uploading",
			json!({"input":input,"identity":scope.identity(),"chunks":[],"uploaded_bytes":0}),
			Some(Utc::now() + Duration::seconds(limits.staging_seconds as i64)),
		)
		.await?;
	scope
		.cache(input.idempotency_key, &digest, &json!({"reference_id":id}))
		.await?;
	Ok(lifecycle::view(&record)?)
}
pub async fn chunk(scope: &mut dyn ReferenceScope, id: Uuid, input: Chunk) -> Result<Reference> {
	let mut record = get(scope, id, "reference.upload").await?;
	if record.state != "uploading" || record.expires_at.is_some_and(|t| t <= Utc::now()) {
		return Err(Error::Conflict("UPLOAD_UNAVAILABLE".into()));
	}
	let bytes = STANDARD
		.decode(&input.data)
		.map_err(|_| Error::Invalid("INVALID_CHUNK".into()))?;
	let maximum = record.data["input"]["size"]
		.as_u64()
		.ok_or(Error::Forbidden)?;
	if !lifecycle::valid_chunk(input.offset, maximum, &bytes) {
		return Err(Error::Invalid("UPLOAD_CHUNK_LIMIT".into()));
	}
	let chunks = record.data["chunks"]
		.as_array_mut()
		.ok_or(Error::Forbidden)?;
	if let Some(old) = chunks.iter().find(|c| c["offset"] == input.offset) {
		if old["file"]["digest"] != format!("{:x}", sha2::Sha256::digest(&bytes))
			|| old["file"]["size"] != bytes.len()
		{
			return Err(Error::Conflict("CHUNK_CHANGED".into()));
		}
		return Ok(lifecycle::view(&record)?);
	}
	let end: u64 = chunks
		.iter()
		.map(|c| c["file"]["size"].as_u64().unwrap_or(0))
		.sum();
	if input.offset != end {
		return Err(Error::Conflict("UPLOAD_OFFSET_CHANGED".into()));
	}
	let (file_id, digest) = scope.put("reference_staging", &bytes).await?;
	let file = FileEntry {
		file_id,
		path: format!("chunk-{end}"),
		digest,
		size: bytes.len() as u64,
		media_type: "application/octet-stream".into(),
		scope: FileScope::References,
		provenance: json!({"kind":"reference","id":id}),
	};
	chunks.push(json!({"offset":end,"file":file}));
	record.data["uploaded_bytes"] = json!(end + bytes.len() as u64);
	scope.update(&mut record).await?;
	Ok(lifecycle::view(&record)?)
}
pub async fn commit(scope: &mut dyn ReferenceScope, id: Uuid) -> Result<Reference> {
	let mut record = get(scope, id, "reference.upload").await?;
	if record.state != "uploading" {
		return Ok(lifecycle::view(&record)?);
	}
	let input: Upload = serde_json::from_value(record.data["input"].clone())?;
	if record.data["uploaded_bytes"] != input.size
		|| record.expires_at.is_some_and(|t| t <= Utc::now())
	{
		return Err(Error::Conflict("UPLOAD_INCOMPLETE_OR_EXPIRED".into()));
	}
	scope.begin_original(input.size).await?;
	for chunk in record.data["chunks"].as_array().ok_or(Error::Forbidden)? {
		let file: FileEntry = serde_json::from_value(chunk["file"].clone())?;
		let bytes = scope.read(&file).await?;
		scope.write_original(&bytes).await?;
	}
	let (file_id, digest) = scope.finish_original(&input.digest).await?;
	let original = FileEntry {
		file_id,
		path: input.name,
		digest,
		size: input.size,
		media_type: input.media_type,
		scope: FileScope::References,
		provenance: json!({"kind":"reference","id":id}),
	};
	record.data["original"] = json!(original);
	record.data["operation_id"] = json!(Uuid::new_v4());
	record.state = "extracting".into();
	record.expires_at = None;
	scope.update(&mut record).await?;
	Ok(lifecycle::view(&record)?)
}
pub async fn revoke(scope: &mut dyn ReferenceScope, id: Uuid, revision: i64) -> Result<()> {
	let mut record = get(scope, id, "reference.delete").await?;
	if record.revision != revision {
		return Err(Error::Conflict("REFERENCE_CHANGED".into()));
	}
	record.state = "revoked".into();
	record.expires_at = Some(Utc::now());
	scope.update(&mut record).await
}
pub async fn pin(scope: &mut dyn ReferenceScope, config: &AgentConfig) -> Result<()> {
	let limits = scope.limits()?;
	let mut mounts = scope.mounts()?;
	if config.reference_attachments.len() > limits.reference_files
		|| (!config.reference_attachments.is_empty() && !config.core_capabilities.files)
	{
		return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
	}
	let mut files = serde_json::from_value::<Vec<FileEntry>>(mounts.manifest.clone())?;
	let mut text = 0;
	let old = mounts.manifest.clone();
	// A new Agent version replaces its read-only mounts. Existing working copies
	// keep their conservative disclosure constraints; detaching is not declassification.
	files.retain(|file| {
		!matches!(file.scope, FileScope::References) || file.provenance["kind"] != "reference"
	});
	let mut seen = std::collections::BTreeSet::new();
	for attachment in &config.reference_attachments {
		if !seen.insert(attachment.reference_id) {
			return Err(Error::Invalid("DUPLICATE_REFERENCE".into()));
		}
		let record = get(scope, attachment.reference_id, "reference.read").await?;
		if record.state != "ready" {
			return Err(Error::Conflict("REFERENCE_NOT_READY".into()));
		}
		let original: FileEntry = serde_json::from_value(record.data["original"].clone())?;
		if original.size > limits.reference_bytes {
			return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
		}
		if original.digest != attachment.digest {
			return Err(Error::Conflict("REFERENCE_CHANGED".into()));
		}
		let constraint = json!({"kind":"reference","id":record.id});
		if !mounts
			.constraints
			.as_array()
			.ok_or(Error::Forbidden)?
			.contains(&constraint)
		{
			mounts.constraints.as_array_mut().unwrap().push(constraint);
			scope.set_constraints(mounts.constraints.clone())?;
		}
		let extracted: Option<FileEntry> =
			serde_json::from_value(record.data["extraction"].clone())?;
		text += extracted.as_ref().map_or(0, |f| f.size);
		for mut file in std::iter::once(original).chain(extracted) {
			if !files.iter().any(|f| f.file_id == file.file_id) {
				file.path = format!("{}/{}", record.id, file.path);
				files.push(file);
			}
		}
	}
	if text > limits.reference_text_bytes as u64
		|| files.iter().map(|f| f.size).sum::<u64>() > limits.working_bytes
	{
		return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
	}
	if json!(files) != old {
		scope.release_python("reference_mounts_changed").await?;
		scope.set_manifest(json!(files))?;
		scope.publish().await?;
	}
	Ok(())
}
pub async fn drive(repository: &dyn ReferenceRepository, id: Uuid) -> Result<()> {
	let snapshot = repository.snapshot(id).await?;
	let mut scope = repository.begin(&snapshot).await?;
	let limits = scope.limits()?;
	let result=Box::pin(async {
        let mut record=get(&mut *scope,id,"reference.upload").await?;
        if record.state!="extracting" {return Ok(());}
        let original:FileEntry=serde_json::from_value(record.data["original"].clone())?;
        let operation:Uuid=serde_json::from_value(record.data["operation_id"].clone())?;
        let path=format!("/v1/operations/{operation}");
        let digest=aidash_domain::registry::rules::digest(&json!(["extract/1",id,original]));
        if !limits.admission && record.data["instance"].is_null() {
            // No dispatch intent was committed, so rollback can stop this
            // upload without contacting or starting an execution environment.
            record.state="ready".into();record.expires_at=None;
            record.data["extraction_state"]=json!("admission_disabled");
            return scope.update(&mut record).await;
        }
        // JSON can expand each text byte to six escaped bytes. Pin the budget
        // with the dispatch intent so a lost reply never changes its request.
        let text_budget=lifecycle::text_budget(&record,limits.reference_text_bytes,limits.output_bytes);
        if limits.admission && text_budget<4 && record.data["instance"].is_null() {
            record.state="ready".into();record.expires_at=None;
            record.data["extraction_state"]=json!("output_limit");
            return scope.update(&mut record).await;
        }
        let health=if limits.admission {
            match scope.verified_health().await {
                Ok(health)=>health,
                Err(_)=>{
                    // Keep the original and retry after an operator repairs the
                    // deployment; never start a parser under different limits.
                    record.data["extraction_state"]=json!("runtime_unavailable");
                    return scope.update(&mut record).await;
                }
            }
        } else { scope.request("GET","/v1/health",None).await? };
        let image=limits.runner_image.as_deref().ok_or(Error::Forbidden)?;
        if health["verified"]!=true||health["image"]!=image {return Err(Error::Conflict("EXTRACTOR_UNAVAILABLE".into()));}
        if record.data["instance"].is_null() {
            // The admission intent survives a lost HTTP reply. Retrying this
            // immutable operation identity only observes or resumes that parser.
            record.data["instance"]=health["instance"].clone();
            record.data["dispatch_pending"]=json!(true);
            record.data["text_budget"]=json!(text_budget);
            return scope.update(&mut record).await;
        }
        if record.data["instance"]!=health["instance"] {record.state="failed".into();record.data["extraction_state"]=json!("runtime_lost");return scope.update(&mut record).await;}
        let observed=if !limits.admission {
            // An intent may have reached the runner before a lost reply.
            // Cancel by its immutable identity; never submit or stage anew.
            scope.request("POST",&format!("{path}/cancel"),None).await?
        } else if record.data["dispatch_pending"]==true {
            scope.request("POST","/v1/operations",Some(json!({"operation_id":operation,"area_id":id,"epoch":1,"digest":digest,"kind":"shell","code":format!("python -I /opt/aidash/extract.py {} {} {}",text_budget,limits.reference_pages,limits.reference_bytes),"seconds":limits.operation_seconds,"files":[{"file_id":original.file_id,"path":"original","scope":"references","size":original.size,"digest":original.digest}]}))).await?
        } else {scope.request("GET",&path,None).await?};
        match observed["status"].as_str() {
            Some("awaiting_files")=>{
                let mut offset=0;
                while offset<original.size {
                    let bytes=scope.read_chunk(&original,offset).await?;
                    scope.request("POST",&format!("{path}/inputs/{}?offset={offset}",original.file_id),Some(json!({"data":STANDARD.encode(&bytes)}))).await?;
                    offset+=bytes.len() as u64;
                }
                scope.request("POST",&format!("{path}/start"),None).await?;
                record.data["dispatch_pending"]=json!(false);
            }
            Some("completed") if observed["termination_confirmed"]==true=>{
                if observed["truncated"]==true {
                    finish_extraction(&mut record,"output_limit",&digest);
                    return scope.update(&mut record).await;
                }
                if !limits.admission {
                    finish_extraction(&mut record,"admission_disabled",&digest);
                    return scope.update(&mut record).await;
                }
                let bytes=STANDARD.decode(observed["stdout"].as_str().ok_or(Error::Forbidden)?).map_err(|_|Error::Invalid("EXTRACTION_RESULT".into()))?;
                let maximum=(limits.reference_text_bytes*6+256).min(limits.output_bytes as usize);
                if bytes.len()>maximum {
                    finish_extraction(&mut record,"output_limit",&digest);
                    return scope.update(&mut record).await;
                }
                let extracted:Value=match serde_json::from_slice(&bytes) {
                    Ok(value)=>value,
                    Err(_)=>{
                        finish_extraction(&mut record,"extraction_failed",&digest);
                        return scope.update(&mut record).await;
                    }
                };
                let (Some(text),Some(state))=(extracted["text"].as_str(),extracted["state"].as_str()) else {
                    finish_extraction(&mut record,"extraction_failed",&digest);
                    return scope.update(&mut record).await;
                };
                if text.len()>limits.reference_text_bytes || !lifecycle::extraction_state_supported(state) {
                    finish_extraction(&mut record,"output_limit",&digest);
                    return scope.update(&mut record).await;
                }
                if !text.is_empty() {
                    let (file_id,digest)=scope.put("reference_extraction",text.as_bytes()).await?;
                    record.data["extraction"]=json!(FileEntry{file_id,path:"extracted.txt".into(),size:text.len() as u64,digest,media_type:"text/plain; charset=utf-8".into(),scope:FileScope::References,provenance:json!({"kind":"reference","id":id,"locations":"page, sheet/cell, or line labels in the extracted text"})});
                }
                finish_extraction(&mut record,state,&digest);
            }
            Some("failed"|"cancelled") if observed["termination_confirmed"]==true=>{
                finish_extraction(&mut record,if limits.admission {"extraction_failed"} else {"admission_disabled"},&digest);
            }
            Some("uncertain"|"absent")=>{record.state="ready".into();record.expires_at=None;record.data["extraction_state"]=json!(if limits.admission {"extraction_failed"} else {"admission_disabled"});}
            Some("accepted"|"starting"|"running"|"finishing"|"cancelling")=>{record.data["dispatch_pending"]=json!(false);}
            _=>return Err(Error::Conflict("EXTRACTION_STATE_UNAVAILABLE".into())),
        }
        scope.update(&mut record).await
    }).await;
	scope.finish(result).await
}
#[derive(Default)]
pub struct Cursor {
	active: Uuid,
	receipt: Uuid,
}
pub async fn sweep(repository: &dyn ReferenceRepository, cursor: &mut Cursor) -> Result<()> {
	let ids = repository.active(cursor.active).await?;
	if ids.is_empty() {
		cursor.active = Uuid::nil();
	}
	for id in ids {
		cursor.active = id;
		if let Err(error) = Box::pin(drive(repository, id)).await {
			tracing::warn!(%id,%error,"reference extraction pending");
		}
	}
	let receipts = repository.receipts(cursor.receipt).await?;
	cursor.receipt = lifecycle::next_receipt_cursor(&receipts);
	for (id, data) in receipts {
		let operation: Uuid = serde_json::from_value(data["operation_id"].clone())?;
		if repository
			.request(
				"POST",
				&format!("/v1/operations/{operation}/ack"),
				Some(json!({"digest":data["runner_digest"]})),
			)
			.await
			.is_ok()
		{
			repository.acknowledge(id).await?;
		}
	}
	Ok(())
}
use sha2::Digest as _;
