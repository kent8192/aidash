use super::{contracts::*, objects, records, service, sessions};
use crate::{
	Error, Result,
	authorization::{access::Access, catalog},
	domain::Run,
	registry::EntityRef,
	store::Store,
};
use sea_orm::sea_query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Selection {
	pub file_id: Uuid,
	pub expected_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Recipient {
	pub node_id: String,
	pub agent_id: String,
	pub agent_version: String,
	pub thread_id: Uuid,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Share {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	pub files: Vec<Selection>,
	pub recipient: Recipient,
}
pub(crate) async fn serialize(access: &mut Access) -> Result<()> {
	sqlx::query(
		&Query::select()
			.expr(Expr::cust("pg_advisory_xact_lock(hashtextextended($1, 0))"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(format!("core-share:{}", access.identity.tenant))
	.execute(&mut **access.tx)
	.await?;
	Ok(())
}
pub(crate) async fn share(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &Area,
	input: Share,
) -> Result<Value> {
	if input.files.is_empty() || input.files.len() > store.capabilities.0.limits.share_files {
		return Err(Error::Invalid("SHARE_FILE_LIMIT".into()));
	}
	let digest = crate::registry::digest(&json!(["file_share", run.id, input]));
	if let Some(previous) = sessions::cached(access, input.idempotency_key, &digest).await? {
		if let Some(id) = previous["remote_transfer_id"].as_str() {
			return super::transfer::view(access, id.parse().map_err(|_| Error::Forbidden)?).await;
		}
		return Ok(previous);
	}
	if input.recipient.node_id != store.node_id {
		return super::transfer::prepare(store, access, run, area, input, &digest).await;
	}
	if input.expected_revision != area.revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	sessions::authorize(access, area, "file.share").await?;
	let available = service::files(area)?;
	let mut chosen = vec![];
	let mut seen = std::collections::BTreeSet::new();
	let mut size = 0;
	for selection in &input.files {
		let file = available
			.iter()
			.find(|f| f.file_id == selection.file_id)
			.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
		if file.digest != selection.expected_digest {
			return Err(Error::Conflict(format!("FILE_CHANGED: {}", file.path)));
		}
		if !seen.insert(file.file_id) || file.size > store.capabilities.0.limits.share_file_bytes {
			return Err(Error::Invalid("SHARE_FILE_LIMIT".into()));
		}
		size += file.size;
		chosen.push(file.clone());
	}
	if size > store.capabilities.0.limits.share_bytes {
		return Err(Error::Invalid("SHARE_BYTE_LIMIT".into()));
	}
	let mut recipient: Area = sqlx::query_as(
		&sessions::select("core_areas")
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2")))
			.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::cust("$3")))
			.and_where(Expr::col(Alias::new("agent_id")).eq(Expr::cust("$4")))
			.and_where(Expr::col(Alias::new("home_node")).eq(Expr::cust("$5")))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&access.identity.tenant)
	.bind(area.workspace_id)
	.bind(input.recipient.thread_id)
	.bind(&input.recipient.agent_id)
	.bind(&store.node_id)
	.fetch_optional(&mut **access.tx)
	.await?
	.ok_or_else(|| Error::NotFound("recipient unavailable".into()))?;
	if recipient.id == area.id {
		return Err(Error::Invalid("SHARE_REQUIRES_OTHER_AGENT".into()));
	}
	service::available(&recipient)?;
	// Receipt must name a version actually admitted in this destination, not
	// merely a discoverable Registry identity.
	let admitted: Option<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column((Alias::new("runs"), Alias::new("id")))
			.from(Alias::new("runs"))
			.and_where(Expr::col(Alias::new("agent_version")).eq(Expr::cust("$2")))
			.and_where(
				Expr::col(Alias::new("id")).in_subquery(
					Query::select()
						.column(Alias::new("run_id"))
						.from(Alias::new("core_runs"))
						.and_where(Expr::col(Alias::new("area_id")).eq(Expr::cust("$1")))
						.to_owned(),
				),
			)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.bind(recipient.id)
	.bind(&input.recipient.agent_version)
	.fetch_optional(&mut **access.tx)
	.await?;
	if admitted.is_none() {
		return Err(Error::NotFound("recipient unavailable".into()));
	}
	let subjects = access.subjects.clone();
	access.subjects = vec![
		recipient.owner.clone(),
		crate::domain::qualified_agent(
			&store.node_id,
			&input.recipient.agent_id,
			&input.recipient.agent_version,
		),
	];
	let received_authority = async {
		let workspace = access.workspace(recipient.workspace_id).await?;
		access.require(&workspace, "workspace.read").await?;
		let entry = catalog::entry(
			access,
			&EntityRef {
				id: input.recipient.agent_id.clone(),
				version: input.recipient.agent_version.clone(),
			},
			"agent.execute",
		)
		.await?;
		let config: crate::registry::AgentConfig = serde_json::from_value(entry.config)?;
		if !config.core_capabilities.sharing {
			return Err(Error::Forbidden);
		}
		let resource = access.resource(
			"working_area",
			recipient.id,
			json!({"owner":recipient.owner,"agent_id":recipient.agent_id,"thread_id":recipient.thread_id}),
		);
		access.require(&resource, "file.receive").await?;
		// Both the existing recipient data and the incoming source constraints
		// must be readable by every recipient subject at this commit boundary.
		sessions::authorize_sources(access, recipient.workspace_id, &recipient.constraints).await?;
		sessions::authorize_sources(access, area.workspace_id, &area.constraints).await
	}
	.await;
	access.subjects = subjects;
	received_authority?;
	let id = Uuid::new_v4();
	let mut entries = service::files(&recipient)?;
	if entries.iter().map(|f| f.size).sum::<u64>() + size > store.capabilities.0.working_bytes
		|| entries.len() + chosen.len() > 4096
	{
		return Err(Error::Conflict("RECIPIENT_QUOTA".into()));
	}
	let manifest_digest = crate::registry::digest(&json!(chosen));
	let mut delivered = vec![];
	for file in &chosen {
		let mut object = store
			.capabilities
			.begin_object(access, Some(recipient.id), "received", file.size)
			.await?;
		let mut offset = 0;
		while offset < file.size {
			let bytes = store.capabilities.read_chunk(access, file, offset).await?;
			if bytes.is_empty() {
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			offset += bytes.len() as u64;
			object.write_block(&bytes).await?;
		}
		let (file_id, hash) = object.finish(access, Some(&file.digest)).await?;
		let path = format!("{id}/{}", file.path);
		objects::validate_path(&path)?;
		let received = FileEntry {
			file_id,
			path,
			digest: hash,
			size: file.size,
			media_type: file.media_type.clone(),
			scope: FileScope::Received,
			provenance: json!({"kind":"received","transfer_id":id,"sender":{"node_id":store.node_id,"agent_id":run.agent_id,"agent_version":run.agent_version},"source_digest":file.digest}),
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
	service::publish(store, access, &mut recipient).await?;
	let result = json!({"operation_id":id,"snapshot_id":id,"transfer_id":id,"status":"completed","manifest_digest":manifest_digest,"recipient":input.recipient,"receipt":{"id":id,"area_id":recipient.id,"revision":recipient.revision,"files":delivered},"revision":area.revision,"generation":area.generation});
	records::insert(
		access,
		id,
		Some(area.id),
		"collaboration",
		"delivered",
		result.clone(),
		None,
	)
	.await?;
	sessions::cache(access, input.idempotency_key, &digest, &result).await?;
	store.event(&mut access.tx,Some(area.workspace_id),"capability.files_shared",json!({"transfer_id":id,"sender_area":area.id,"recipient_area":recipient.id,"manifest_digest":manifest_digest})).await?;
	Ok(result)
}
