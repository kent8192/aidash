//! Upload, immutable mounts, parser dispatch and receipt recovery share current authority.
use crate::{
	Error, Result,
	ports::capabilities::references::{ReferenceRepository, ReferenceScope},
};
use aidash_domain::{
	capabilities::{
		ReferenceAttachment,
		operations::{FileScope, MountedFile as FileEntry},
		records::Record,
		references::{
			EXTRACTION_SUFFIX, REFERENCE_SET_ROOT,
			lifecycle::{
				self, BatchFile, BatchOutcome, BatchResult, Chunk, Readability, Reference, Upload,
				finish_extraction,
			},
		},
	},
	registry::AgentConfig,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;
/// Loads a Reference record. Only a write needs the row exclusively. A read
/// stays shared so a person opening a file never waits for a step that holds a
/// copy of it, and a step never waits on its own outer transaction.
async fn load(scope: &mut dyn ReferenceScope, id: Uuid, shared: bool) -> Result<Record> {
	if shared {
		scope
			.load_many(&[id])
			.await?
			.into_iter()
			.next()
			.ok_or_else(|| Error::NotFound("resource unavailable".into()))
	} else {
		scope.load(id).await
	}
}
fn unusable(record: &Record) -> bool {
	matches!(record.state.as_str(), "revoked" | "expired")
}
pub async fn get(scope: &mut dyn ReferenceScope, id: Uuid, action: &str) -> Result<Record> {
	let record = load(scope, id, action == "reference.read").await?;
	if record.owner != scope.principal() || unusable(&record) {
		return Err(Error::NotFound("reference unavailable".into()));
	}
	scope
		.require(id, &record.owner, action)
		.await
		.map_err(|_| Error::NotFound("reference unavailable".into()))?;
	Ok(record)
}
/// The record of a Reference its caller may revoke. Unlike `get`, this does not
/// require the caller to be the uploader: the Tenant's policy decides who holds
/// `reference.delete` on it (#826), so a file stays stoppable after its uploader
/// leaves. Anyone the policy refuses gets the same `NotFound` as for a missing
/// file, so they cannot learn that it exists. Reading stays with the uploader.
async fn revocable(scope: &mut dyn ReferenceScope, id: Uuid, shared: bool) -> Result<Record> {
	let record = load(scope, id, shared).await?;
	if unusable(&record) {
		return Err(Error::NotFound("reference unavailable".into()));
	}
	scope
		.require(id, &record.owner, "reference.delete")
		.await
		.map_err(|_| Error::NotFound("reference unavailable".into()))?;
	Ok(record)
}
/// What a person about to revoke needs to know: the revision to send back and
/// the state. It is the record, so the caller must pick those fields; it never
/// carries content, and it needs `reference.delete`, not `reference.read`.
pub async fn revocation(scope: &mut dyn ReferenceScope, id: Uuid) -> Result<Record> {
	revocable(scope, id, true).await
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
	let bytes = scope.read(&original).await?;
	settle(&mut record, &original, &bytes);
	scope.update(&mut record).await?;
	Ok(lifecycle::view(&record)?)
}
/// Text is ready at once. Only a file the parser must open waits for a Pod.
fn settle(record: &mut Record, original: &FileEntry, bytes: &[u8]) {
	record.data["original"] = json!(original);
	record.expires_at = None;
	match lifecycle::readability(bytes) {
		Readability::Parser => {
			record.data["operation_id"] = json!(Uuid::new_v4());
			record.state = "extracting".into();
		}
		Readability::Text => {
			record.data["plain_text"] = json!(true);
			record.data["extraction_state"] = json!("ready");
			record.state = "ready".into();
		}
		Readability::Unsupported => {
			record.data["extraction_state"] = json!("unsupported");
			record.state = "ready".into();
		}
	}
}
/// Add many documents in one request. A file that cannot be added is reported
/// by itself, so the others still land.
pub async fn add_batch(
	scope: &mut dyn ReferenceScope,
	files: Vec<BatchFile>,
) -> Result<BatchResult> {
	let limits = scope.limits()?;
	if !limits.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if files.is_empty() || files.len() > limits.reference_files {
		return Err(Error::Invalid("REFERENCE_BATCH_LIMIT".into()));
	}
	scope.require_upload().await?;
	let mut result = BatchResult::default();
	let mut paths = std::collections::BTreeSet::new();
	let mut added = std::collections::BTreeMap::<String, String>::new();
	for file in files {
		let path = file.path;
		if !paths.insert(path.clone()) {
			result
				.files
				.push(BatchOutcome::refused(path, "DUPLICATE_PATH"));
			continue;
		}
		let bytes = match prepare_batch_file(&path, &file.data, limits.reference_bytes) {
			Ok(bytes) => bytes,
			Err(code) => {
				result.files.push(BatchOutcome::refused(path, code));
				continue;
			}
		};
		let digest = format!("{:x}", sha2::Sha256::digest(&bytes));
		// One entry per content: a Source binds a Reference once.
		if let Some(first) = added.get(&digest) {
			let mut outcome = BatchOutcome::refused(path, "SAME_CONTENT");
			outcome.duplicate_of = Some(first.clone());
			result.files.push(outcome);
			continue;
		}
		let (record, reused) = match scope.find_existing(&digest).await? {
			Some(record) => (record, true),
			None => (
				store_batch_file(scope, &path, &bytes, &digest).await?,
				false,
			),
		};
		added.insert(digest.clone(), path.clone());
		let view = lifecycle::view(&record)?;
		result.references.push(ReferenceAttachment {
			reference_id: view.reference_id,
			digest,
			path: path.clone(),
		});
		result.files.push(BatchOutcome::added(path, view, reused));
	}
	Ok(result)
}
fn file_name(path: &str) -> &str {
	path.rsplit('/').next().unwrap_or(path)
}
fn prepare_batch_file(
	path: &str,
	data: &str,
	maximum: u64,
) -> std::result::Result<Vec<u8>, &'static str> {
	// The Source refuses a path its extraction could collide with, so refuse it here
	// rather than report a file as added that no Source can bind.
	if !aidash_domain::capabilities::references::valid_path(path) {
		return Err("INVALID_PATH");
	}
	let bytes = STANDARD.decode(data).map_err(|_| "INVALID_DATA")?;
	if bytes.is_empty() || bytes.len() as u64 > maximum || file_name(path).len() > 255 {
		return Err("REFERENCE_UPLOAD_LIMIT");
	}
	Ok(bytes)
}
async fn store_batch_file(
	scope: &mut dyn ReferenceScope,
	path: &str,
	bytes: &[u8],
	digest: &str,
) -> Result<Record> {
	let input = Upload {
		idempotency_key: Uuid::new_v4(),
		name: file_name(path).to_owned(),
		media_type: lifecycle::sniffed_media_type(bytes).to_owned(),
		size: bytes.len() as u64,
		digest: digest.to_owned(),
	};
	lifecycle::validate_upload(&input, scope.limits()?.reference_bytes)?;
	let id = Uuid::new_v4();
	let mut record = scope
		.insert(
			id,
			"uploading",
			json!({"input":input,"identity":scope.identity(),"chunks":[],"uploaded_bytes":input.size}),
			None,
		)
		.await?;
	scope.begin_original(input.size).await?;
	scope.write_original(bytes).await?;
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
	settle(&mut record, &original, bytes);
	scope.update(&mut record).await?;
	Ok(record)
}
pub async fn revoke(scope: &mut dyn ReferenceScope, id: Uuid, revision: i64) -> Result<()> {
	let mut record = revocable(scope, id, false).await?;
	if record.revision != revision {
		return Err(Error::Conflict("REFERENCE_CHANGED".into()));
	}
	record.state = "revoked".into();
	record.expires_at = Some(Utc::now());
	scope.update(&mut record).await
}
/// A Run reads the Reference files its Agent version binds. Run admission has
/// already established that this person may run that version, whose Sources are
/// approved, so neither the uploader nor a `reference.read` grant is asked for
/// here (ADR 0114). Only a revoked or removed file stops it.
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
	// A new Agent version replaces its Reference set. Existing working copies
	// keep their conservative disclosure constraints; detaching is not declassification.
	files.retain(|file| !file.is_reference_file());
	let ids = config
		.reference_attachments
		.iter()
		.map(|attachment| attachment.reference_id)
		.collect::<Vec<_>>();
	if ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len() {
		return Err(Error::Invalid("DUPLICATE_REFERENCE".into()));
	}
	let records = scope
		.load_many(&ids)
		.await?
		.into_iter()
		.map(|record| (record.id, record))
		.collect::<std::collections::BTreeMap<_, _>>();
	for attachment in &config.reference_attachments {
		let record = records
			.get(&attachment.reference_id)
			.filter(|record| !matches!(record.state.as_str(), "revoked" | "expired"))
			.ok_or_else(|| {
				Error::NotFound(format!("reference unavailable: {}", attachment.path))
			})?;
		if record.state != "ready" {
			return Err(Error::Conflict("REFERENCE_NOT_READY".into()));
		}
		let mut original: FileEntry = serde_json::from_value(record.data["original"].clone())?;
		if original.size > limits.reference_bytes {
			return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
		}
		if original.digest != attachment.digest {
			return Err(Error::Conflict("REFERENCE_CHANGED".into()));
		}
		// Blocks export from this Area. Unlike a copy's `reference` constraint it
		// never locks the Area: a revoked file only stops the Runs that still bind it.
		let constraint = json!({"kind":"reference_bound","id":record.id});
		if !mounts
			.constraints
			.as_array()
			.ok_or(Error::Forbidden)?
			.contains(&constraint)
		{
			mounts.constraints.as_array_mut().unwrap().push(constraint);
			scope.set_constraints(mounts.constraints.clone())?;
		}
		text += lifecycle::text_bytes(&record.data);
		let path = format!("{REFERENCE_SET_ROOT}/{}", attachment.path);
		if let Some(mut extracted) =
			serde_json::from_value::<Option<FileEntry>>(record.data["extraction"].clone())?
		{
			extracted.path = format!("{path}{EXTRACTION_SUFFIX}");
			files.push(extracted);
		}
		original.path = path;
		files.push(original);
	}
	if text > limits.reference_text_bytes as u64 {
		return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
	}
	if aidash_domain::capabilities::operations::has_input_path_collision(&files) {
		return Err(Error::Invalid("REFERENCE_PATH_COLLISION".into()));
	}
	// Reconciliation reorders the manifest, which alone is not a new revision.
	let ordered = |mut files: Vec<FileEntry>| {
		files.sort_by_key(|file| file.file_id);
		json!(files)
	};
	if ordered(files.clone()) != ordered(serde_json::from_value(old)?) {
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
        let text_budget=lifecycle::text_budget(&record,limits.reference_file_text_bytes,limits.output_bytes);
        if limits.admission && text_budget<4 && record.data["instance"].is_null() {
            record.state="ready".into();record.expires_at=None;
            record.data["extraction_state"]=json!("output_limit");
            return scope.update(&mut record).await;
        }
        let health=if limits.admission {
            match scope.verified_health().await {
                Ok(health)=>health,
                Err(_)=>{
                    // Keep the original and retry after whoever runs the Deployment repairs the
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
                let maximum=(limits.reference_file_text_bytes*6+lifecycle::OUTPUT_OVERHEAD as usize).min(limits.output_bytes as usize);
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
                if text.len()>limits.reference_file_text_bytes || !lifecycle::extraction_state_supported(state) {
                    finish_extraction(&mut record,"output_limit",&digest);
                    return scope.update(&mut record).await;
                }
                if !text.is_empty() {
                    let (file_id,digest)=scope.put("reference_extraction",text.as_bytes()).await?;
                    record.data["extraction"]=json!(FileEntry{file_id,path:"extracted.txt".into(),size:text.len() as u64,digest,media_type:"text/plain; charset=utf-8".into(),scope:FileScope::References,provenance:json!({"kind":"reference","id":id,"locations":"page or sheet/cell labels in the extracted text"})});
                }
                let pages:Vec<u32>=serde_json::from_value(extracted["pages_without_text"].clone()).unwrap_or_default();
                record.data["pages_without_text"]=json!(pages);
                record.data["page_count"]=extracted["page_count"].clone();
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
		// One unreadable row must not hold back the rest of the batch.
		let operation: Uuid = match serde_json::from_value(data["operation_id"].clone()) {
			Ok(operation) => operation,
			Err(error) => {
				metrics::counter!(
					"aidash_background_service_failures_total",
					"service" => "capabilities.references"
				)
				.increment(1);
				tracing::warn!(%id,%error,"skipping a reference receipt that cannot be read");
				continue;
			}
		};
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

#[cfg(test)]
mod tests;
