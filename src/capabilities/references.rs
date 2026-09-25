//! Private original uploads. Registry metadata contains only immutable bindings;
//! parsing happens in the dedicated runner and never in the API process.
use super::{
	contracts::*,
	objects, operations,
	records::{self, Record},
	service, sessions,
};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	registry::AgentConfig,
	store::Store,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
	pub reference_id: Uuid,
	pub digest: String,
}
pub(crate) fn validate_config(config: &AgentConfig) -> Result<()> {
	let bindings = &config.reference_attachments;
	let mut seen = std::collections::BTreeSet::new();
	if bindings.len() > 8
		|| !bindings.is_empty() && !config.core_capabilities.files
		|| bindings.iter().any(|r| {
			!seen.insert(r.reference_id)
				|| r.digest.len() != 64
				|| !r.digest.bytes().all(|b| b.is_ascii_hexdigit())
		}) {
		return Err(Error::Invalid("INVALID_REFERENCE_BINDINGS".into()));
	}
	Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Upload {
	pub idempotency_key: Uuid,
	pub name: String,
	pub media_type: String,
	pub size: u64,
	pub digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
	pub offset: u64,
	pub data: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Reference {
	pub reference_id: Uuid,
	pub revision: i64,
	pub state: String,
	pub name: String,
	pub media_type: String,
	pub size: u64,
	pub digest: String,
	pub uploaded_bytes: u64,
	pub original: Option<FileEntry>,
	pub extraction: Option<FileEntry>,
	pub extraction_state: Option<String>,
}
fn view(record: &Record) -> Result<Reference> {
	let input: Upload = serde_json::from_value(record.data["input"].clone())?;
	Ok(Reference {
		reference_id: record.id,
		revision: record.revision,
		state: record.state.clone(),
		name: input.name,
		media_type: input.media_type,
		size: input.size,
		digest: input.digest,
		uploaded_bytes: record.data["uploaded_bytes"].as_u64().unwrap_or(0),
		original: serde_json::from_value(record.data["original"].clone())?,
		extraction: serde_json::from_value(record.data["extraction"].clone())?,
		extraction_state: record.data["extraction_state"].as_str().map(str::to_owned),
	})
}
pub(crate) async fn get(access: &mut Access, id: Uuid, action: &str) -> Result<Record> {
	let record = records::get(access, id, "reference").await?;
	if record.owner != access.identity.subject
		|| matches!(record.state.as_str(), "revoked" | "expired")
	{
		return Err(Error::NotFound("reference unavailable".into()));
	}
	access
		.require(
			&access.resource("reference", id, json!({"owner":record.owner})),
			action,
		)
		.await
		.map_err(|_| Error::NotFound("reference unavailable".into()))?;
	Ok(record)
}
pub(crate) async fn inspect(access: &mut Access, id: Uuid) -> Result<Reference> {
	view(&get(access, id, "reference.read").await?)
}
pub(crate) async fn start(store: &Store, access: &mut Access, input: Upload) -> Result<Reference> {
	if !store.capabilities.0.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if input.size == 0
		|| input.size > store.capabilities.0.limits.reference_bytes
		|| input.name.is_empty()
		|| input.name.len() > 255
		|| input.name.chars().any(char::is_control)
		|| input.name.contains(['/', '\\'])
		|| input.digest.len() != 64
		|| !input
			.digest
			.bytes()
			.all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
		|| !matches!(
			input.media_type.as_str(),
			"application/pdf"
				| "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
				| "text/plain"
		) {
		return Err(Error::Invalid("REFERENCE_UPLOAD_LIMIT".into()));
	}
	access
		.require(
			&access.resource("reference", "new", json!({})),
			"reference.upload",
		)
		.await?;
	let digest = crate::registry::digest(&json!(["reference_upload", input]));
	if let Some(previous) = sessions::cached(access, input.idempotency_key, &digest).await? {
		return inspect(
			access,
			serde_json::from_value(previous["reference_id"].clone())?,
		)
		.await;
	}
	let id = Uuid::new_v4();
	let record = records::insert(
		access,
		id,
		None,
		"reference",
		"uploading",
		json!({"input":input,"identity":{"credential_id":access.identity.credential_id,"tenant":access.identity.tenant,"subject":access.identity.subject},"chunks":[],"uploaded_bytes":0}),
		Some(Utc::now() + Duration::seconds(store.capabilities.0.staging_seconds as i64)),
	)
	.await?;
	sessions::cache(
		access,
		input.idempotency_key,
		&digest,
		&json!({"reference_id":id}),
	)
	.await?;
	view(&record)
}
pub(crate) async fn chunk(
	store: &Store,
	access: &mut Access,
	id: Uuid,
	input: Chunk,
) -> Result<Reference> {
	let mut record = get(access, id, "reference.upload").await?;
	if record.state != "uploading" || record.expires_at.is_some_and(|t| t <= Utc::now()) {
		return Err(Error::Conflict("UPLOAD_UNAVAILABLE".into()));
	}
	let bytes = STANDARD
		.decode(&input.data)
		.map_err(|_| Error::Invalid("INVALID_CHUNK".into()))?;
	let maximum = record.data["input"]["size"]
		.as_u64()
		.ok_or(Error::Forbidden)?;
	if bytes.is_empty()
		|| bytes.len() > 4 << 20
		|| input
			.offset
			.checked_add(bytes.len() as u64)
			.is_none_or(|end| end > maximum)
	{
		return Err(Error::Invalid("UPLOAD_CHUNK_LIMIT".into()));
	}
	let chunks = record.data["chunks"]
		.as_array_mut()
		.ok_or(Error::Forbidden)?;
	if let Some(old) = chunks.iter().find(|c| c["offset"] == input.offset) {
		if old["file"]["digest"] != objects::digest(&bytes) || old["file"]["size"] != bytes.len() {
			return Err(Error::Conflict("CHUNK_CHANGED".into()));
		}
		return view(&record);
	}
	let end: u64 = chunks
		.iter()
		.map(|c| c["file"]["size"].as_u64().unwrap_or(0))
		.sum();
	if input.offset != end {
		return Err(Error::Conflict("UPLOAD_OFFSET_CHANGED".into()));
	}
	let (file_id, digest) = store
		.capabilities
		.put(access, None, "reference_staging", &bytes)
		.await?;
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
	records::update(access, &mut record).await?;
	view(&record)
}
pub(crate) async fn commit(store: &Store, access: &mut Access, id: Uuid) -> Result<Reference> {
	let mut record = get(access, id, "reference.upload").await?;
	if record.state != "uploading" {
		return view(&record);
	}
	let input: Upload = serde_json::from_value(record.data["input"].clone())?;
	if record.data["uploaded_bytes"] != input.size
		|| record.expires_at.is_some_and(|t| t <= Utc::now())
	{
		return Err(Error::Conflict("UPLOAD_INCOMPLETE_OR_EXPIRED".into()));
	}
	let mut object = store
		.capabilities
		.begin_object(access, None, "reference_original", input.size)
		.await?;
	for chunk in record.data["chunks"].as_array().ok_or(Error::Forbidden)? {
		let file: FileEntry = serde_json::from_value(chunk["file"].clone())?;
		object
			.write_block(&store.capabilities.read(access, &file).await?)
			.await?;
	}
	let (file_id, digest) = object.finish(access, Some(&input.digest)).await?;
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
	records::update(access, &mut record).await?;
	view(&record)
}
pub(crate) async fn revoke(access: &mut Access, id: Uuid, revision: i64) -> Result<()> {
	let mut record = get(access, id, "reference.delete").await?;
	if record.revision != revision {
		return Err(Error::Conflict("REFERENCE_CHANGED".into()));
	}
	record.state = "revoked".into();
	record.expires_at = Some(Utc::now());
	records::update(access, &mut record).await
}
pub(crate) async fn pin(
	store: &Store,
	access: &mut Access,
	area: &mut Area,
	config: &AgentConfig,
) -> Result<()> {
	if config.reference_attachments.len() > store.capabilities.0.limits.reference_files
		|| (!config.reference_attachments.is_empty() && !config.core_capabilities.files)
	{
		return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
	}
	let mut files = service::files(area)?;
	let mut text = 0;
	let old = area.manifest.clone();
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
		let record = get(access, attachment.reference_id, "reference.read").await?;
		if record.state != "ready" {
			return Err(Error::Conflict("REFERENCE_NOT_READY".into()));
		}
		let original: FileEntry = serde_json::from_value(record.data["original"].clone())?;
		if original.size > store.capabilities.0.limits.reference_bytes {
			return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
		}
		if original.digest != attachment.digest {
			return Err(Error::Conflict("REFERENCE_CHANGED".into()));
		}
		let constraint = json!({"kind":"reference","id":record.id});
		if !area
			.constraints
			.as_array()
			.ok_or(Error::Forbidden)?
			.contains(&constraint)
		{
			area.constraints.as_array_mut().unwrap().push(constraint);
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
	if text > store.capabilities.0.limits.reference_text_bytes as u64
		|| files.iter().map(|f| f.size).sum::<u64>() > store.capabilities.0.working_bytes
	{
		return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
	}
	if json!(files) != old {
		super::python::release(store, access, area, "reference_mounts_changed").await?;
		area.manifest = json!(files);
		service::publish(store, access, area).await?;
	}
	Ok(())
}

async fn drive(store: &Store, id: Uuid) -> Result<()> {
	let snapshot: Record = sqlx::query_as(
		&sessions::select("core_records")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_one(&store.pool)
	.await?;
	let identity = SubjectIdentity {
		credential_id: serde_json::from_value(snapshot.data["identity"]["credential_id"].clone())?,
		tenant: snapshot.tenant.clone(),
		subject: snapshot.owner.clone(),
	};
	let mut access = Access::begin(store, &identity).await?;
	let result=Box::pin(async {
        let mut record=get(&mut access,id,"reference.upload").await?;
        if record.state!="extracting" {return Ok(());}
        let original:FileEntry=serde_json::from_value(record.data["original"].clone())?;
        let operation:Uuid=serde_json::from_value(record.data["operation_id"].clone())?;
        let path=format!("/v1/operations/{operation}");
        let digest=crate::registry::digest(&json!(["extract/1",id,original]));
        if !store.capabilities.0.admission && record.data["instance"].is_null() {
            // No dispatch intent was committed, so rollback can stop this
            // upload without contacting or starting an execution environment.
            record.state="ready".into();record.expires_at=None;
            record.data["extraction_state"]=json!("admission_disabled");
            return records::update(&mut access,&mut record).await;
        }
        let health=operations::remote(store,reqwest::Method::GET,"/v1/health",None).await?;
        let profile=store.capabilities.0.runner.as_ref().ok_or(Error::Forbidden)?;
        if health["verified"]!=true||health["image"]!=profile.image {return Err(Error::Conflict("EXTRACTOR_UNAVAILABLE".into()));}
        if record.data["instance"].is_null() {
            // The admission intent survives a lost HTTP reply. Retrying this
            // immutable operation identity only observes or resumes that parser.
            record.data["instance"]=health["instance"].clone();
            record.data["dispatch_pending"]=json!(true);
            return records::update(&mut access,&mut record).await;
        }
        if record.data["instance"]!=health["instance"] {record.state="failed".into();record.data["extraction_state"]=json!("runtime_lost");return records::update(&mut access,&mut record).await;}
        let observed=if !store.capabilities.0.admission {
            // An intent may have reached the runner before a lost reply.
            // Cancel by its immutable identity; never submit or stage anew.
            operations::remote(store,reqwest::Method::POST,&format!("{path}/cancel"),None).await?
        } else if record.data["dispatch_pending"]==true {
            operations::remote(store,reqwest::Method::POST,"/v1/operations",Some(json!({"operation_id":operation,"area_id":id,"epoch":1,"digest":digest,"kind":"shell","code":format!("python -I /opt/aidash/extract.py {} {} {}",store.capabilities.0.limits.reference_text_bytes,store.capabilities.0.limits.reference_pages,store.capabilities.0.limits.reference_bytes),"seconds":store.capabilities.0.operation_seconds,"files":[{"file_id":original.file_id,"path":"original","scope":"references","size":original.size,"digest":original.digest}]}))).await?
        } else {operations::remote(store,reqwest::Method::GET,&path,None).await?};
        match observed["status"].as_str() {
            Some("awaiting_files")=>{
                let mut offset=0;
                while offset<original.size {
                    let bytes=store.capabilities.read_chunk(&mut access,&original,offset).await?;
                    operations::remote(store,reqwest::Method::POST,&format!("{path}/inputs/{}?offset={offset}",original.file_id),Some(json!({"data":STANDARD.encode(&bytes)}))).await?;
                    offset+=bytes.len() as u64;
                }
                operations::remote(store,reqwest::Method::POST,&format!("{path}/start"),None).await?;
                record.data["dispatch_pending"]=json!(false);
            }
            Some("completed") if observed["termination_confirmed"]==true=>{
                if !store.capabilities.0.admission {
                    record.state="ready".into();record.expires_at=None;
                    record.data["extraction_state"]=json!("admission_disabled");
                    record.data["receipt_pending"]=json!(true);record.data["runner_digest"]=json!(digest);
                    return records::update(&mut access,&mut record).await;
                }
                let bytes=STANDARD.decode(observed["stdout"].as_str().ok_or(Error::Forbidden)?).map_err(|_|Error::Invalid("EXTRACTION_RESULT".into()))?;
                if bytes.len()>262144 {return Err(Error::Conflict("EXTRACTION_RESULT_LIMIT".into()));}
                let extracted:Value=serde_json::from_slice(&bytes)?;
                let text=extracted["text"].as_str().ok_or(Error::Forbidden)?;
                let state=extracted["state"].as_str().ok_or(Error::Forbidden)?;
                if text.len()>store.capabilities.0.limits.reference_text_bytes || !matches!(state,"ready"|"text_limit"|"non_extractable"|"unsupported"|"malformed"|"encrypted"|"page_limit"|"file_limit"|"expanded_size_limit") {
                    return Err(Error::Conflict("EXTRACTION_RESULT_LIMIT".into()));
                }
                if !text.is_empty() {
                    let (file_id,digest)=store.capabilities.put(&mut access,None,"reference_extraction",text.as_bytes()).await?;
                    record.data["extraction"]=json!(FileEntry{file_id,path:"extracted.txt".into(),size:text.len() as u64,digest,media_type:"text/plain; charset=utf-8".into(),scope:FileScope::References,provenance:json!({"kind":"reference","id":id,"locations":"page, sheet/cell, or line labels in the extracted text"})});
                }
                record.state="ready".into();record.expires_at=None;record.data["extraction_state"]=json!(state);
                record.data["receipt_pending"]=json!(true);record.data["runner_digest"]=json!(digest);
            }
            Some("failed"|"cancelled") if observed["termination_confirmed"]==true=>{
                record.state="ready".into();record.expires_at=None;
                record.data["extraction_state"]=json!(if store.capabilities.0.admission {"extraction_failed"} else {"admission_disabled"});
                record.data["receipt_pending"]=json!(true);record.data["runner_digest"]=json!(digest);
            }
            Some("uncertain"|"absent")=>{record.state="ready".into();record.expires_at=None;record.data["extraction_state"]=json!(if store.capabilities.0.admission {"extraction_failed"} else {"admission_disabled"});}
            Some("accepted"|"starting"|"running"|"finishing"|"cancelling")=>{record.data["dispatch_pending"]=json!(false);}
            _=>return Err(Error::Conflict("EXTRACTION_STATE_UNAVAILABLE".into())),
        }
        records::update(&mut access,&mut record).await
    }).await;
	access.finish(result).await
}
pub(crate) async fn run(
	store: Store,
	mut stopping: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		let ids: Vec<Uuid> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("core_records"))
				.and_where(Expr::col(Alias::new("kind")).eq("reference"))
				.and_where(Expr::col(Alias::new("state")).eq("extracting"))
				.limit(8)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&store.pool)
		.await?;
		for id in ids {
			if let Err(error) = Box::pin(drive(&store, id)).await {
				tracing::warn!(%id,%error,"reference extraction pending");
			}
		}
		let receipts: Vec<(Uuid, Value)> = sqlx::query_as(
			&Query::select()
				.columns(["id", "data"].map(Alias::new))
				.from(Alias::new("core_records"))
				.and_where(Expr::col(Alias::new("kind")).eq("reference"))
				.and_where(Expr::cust("data->'receipt_pending' = 'true'::jsonb"))
				.limit(8)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&store.pool)
		.await?;
		for (id, data) in receipts {
			let operation: Uuid = serde_json::from_value(data["operation_id"].clone())?;
			if operations::remote(
				&store,
				reqwest::Method::POST,
				&format!("/v1/operations/{operation}/ack"),
				Some(json!({"digest":data["runner_digest"]})),
			)
			.await
			.is_ok()
			{
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_records"))
						.value(
							Alias::new("data"),
							Expr::cust("jsonb_set(data,'{receipt_pending}','false'::jsonb)"),
						)
						.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
						.to_string(PostgresQueryBuilder),
				)
				.bind(id)
				.execute(&store.pool)
				.await?;
			}
		}
		tokio::select! {_=stopping.changed()=>{},_=tokio::time::sleep(std::time::Duration::from_millis(500))=>{}}
	}
}
