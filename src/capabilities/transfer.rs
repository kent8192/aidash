//! Independent file-transfer/1 protocol over the existing scoped peer identity
//! mapping. Task offers and broad peer credentials are never execution authority.
pub(crate) mod receiver;
use super::{
	contracts::*,
	objects,
	records::{self, Record},
	service, sessions,
	sharing::{Recipient, Share},
};
use crate::{
	Error, Result,
	authorization::{access::Access, catalog, identity::SubjectIdentity, peer},
	domain::Run,
	federation::Federation,
	registry::EntityRef,
	store::Store,
};
use axum::{Json, extract::State, http::HeaderMap};
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Description {
	pub protocol: String,
	pub transfer_id: Uuid,
	pub source_node: String,
	pub target: Recipient,
	pub source_tenant: String,
	pub source_subject: String,
	pub source_agent: EntityRef,
	pub input_digest: String,
	pub manifest_digest: String,
	pub files: Vec<FileEntry>,
	pub expires_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Identity {
	pub transfer_id: Uuid,
	pub input_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Requester {
	pub tenant: String,
	pub subject: String,
	#[serde(default)]
	pub cursor: Option<Uuid>,
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
	pub transfer_id: Uuid,
	pub input_digest: String,
	pub file: usize,
	pub offset: u64,
	pub data: String,
}

pub(crate) async fn view(access: &mut Access, id: Uuid) -> Result<Value> {
	let record = records::get(access, id, "transfer_out").await?;
	if record.owner != access.identity.subject {
		return Err(Error::NotFound("transfer unavailable".into()));
	}
	let workspace = access
		.workspace(serde_json::from_value(record.data["workspace_id"].clone())?)
		.await?;
	access.require(&workspace, "workspace.read").await?;
	access
		.require(
			&access.resource(
				"working_area",
				record.area_id.ok_or(Error::Forbidden)?,
				json!({"owner":record.owner}),
			),
			"file.read",
		)
		.await?;
	Ok(
		json!({"operation_id":id,"transfer_id":id,"status":if record.state=="delivered"{"completed"}else{record.state.as_str()},"recipient":record.data["description"]["target"],"manifest_digest":record.data["description"]["manifest_digest"],"receipt":record.data["receipt"],"error":super::errors::CapabilityError::stored(&record.data["error"]),"effects_may_have_occurred":record.data["commit_attempted"]==true}),
	)
}
pub(crate) async fn prepare(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &Area,
	input: Share,
	digest: &str,
) -> Result<Value> {
	if !store.capabilities.0.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if input.expected_revision != area.revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	sessions::authorize(access, area, "file.share").await?;
	access
		.require(
			&access.resource("node", &input.recipient.node_id, json!({})),
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
	let original_subjects = access.subjects.clone();
	access.subjects.push(crate::domain::qualified_agent(
		&input.recipient.node_id,
		&input.recipient.agent_id,
		&input.recipient.agent_version,
	));
	let recipient_permission = async {
		sessions::authorize(access, area, "file.share").await?;
		sessions::authorize_sources(access, area.workspace_id, &area.constraints).await
	}
	.await;
	access.subjects = original_subjects;
	recipient_permission?;
	let available = service::files(area)?;
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
		if !seen.insert(file.file_id) || file.size > store.capabilities.0.limits.share_file_bytes {
			return Err(Error::Invalid("SHARE_FILE_LIMIT".into()));
		}
		total += file.size;
		if total > store.capabilities.0.limits.share_bytes {
			return Err(Error::Invalid("SHARE_BYTE_LIMIT".into()));
		}
		chosen.push(
			store
				.capabilities
				.copy_object(access, None, "transfer_snapshot", file)
				.await?,
		);
	}
	let id = Uuid::new_v4();
	let expires_at = Utc::now() + Duration::seconds(store.capabilities.0.staging_seconds as i64);
	let description = Description {
		protocol: "file-transfer/1".into(),
		transfer_id: id,
		source_node: store.node_id.clone(),
		target: input.recipient,
		source_tenant: access.identity.tenant.clone(),
		source_subject: access.identity.subject.clone(),
		source_agent: EntityRef {
			id: run.agent_id.clone(),
			version: run.agent_version.clone(),
		},
		input_digest: digest.into(),
		manifest_digest: crate::registry::digest(&json!(chosen)),
		files: chosen,
		expires_at,
	};
	records::insert(access,id,Some(area.id),"transfer_out","pending",json!({"description":description,"credential_id":access.identity.credential_id,"subjects":access.subjects,"workspace_id":area.workspace_id,"thread_id":area.thread_id,"constraints":area.constraints,"run_id":run.id,"offsets":{}}),Some(expires_at)).await?;
	sessions::cache(
		access,
		input.idempotency_key,
		digest,
		&json!({"remote_transfer_id":id}),
	)
	.await?;
	view(access, id).await
}
async fn snapshot(f: &Federation, id: Uuid) -> Result<Record> {
	sqlx::query_as(
		&sessions::select("core_records")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("kind")).eq("transfer_out"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_optional(&f.store.pool)
	.await?
	.ok_or_else(|| Error::NotFound("transfer unavailable".into()))
}
async fn authority(f: &Federation, record: &Record) -> Result<Access> {
	if !f.store.capabilities.0.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	let mut access = Access::begin(
		&f.store,
		&SubjectIdentity {
			credential_id: serde_json::from_value(record.data["credential_id"].clone())?,
			tenant: record.tenant.clone(),
			subject: record.owner.clone(),
		},
	)
	.await?;
	let result = async {
		access.subjects = serde_json::from_value(record.data["subjects"].clone())?;
		let description: Description = serde_json::from_value(record.data["description"].clone())?;
		if record.expires_at.is_none_or(|t| t <= Utc::now()) {
			return Err(Error::Conflict("TRANSFER_EXPIRED".into()));
		}
        let peer_enabled: Option<bool> = sqlx::query_scalar(&Query::select().column(Alias::new("enabled"))
            .from(Alias::new("peers")).and_where(Expr::col(Alias::new("node_id")).eq(Expr::cust("$1")))
            .lock(sea_orm::sea_query::LockType::Share).to_string(PostgresQueryBuilder))
            .bind(&description.target.node_id).fetch_optional(&mut **access.tx).await?;
        if peer_enabled != Some(true) { return Err(Error::Forbidden); }
        let run = access
            .run_for_interaction(serde_json::from_value(record.data["run_id"].clone())?)
			.await?;
		if run.control == "CANCELLED" {
			return Err(Error::Forbidden);
		}
		let workspace = access
			.workspace(serde_json::from_value(record.data["workspace_id"].clone())?)
			.await?;
		access.context = workspace.attributes.clone();
		access.require(&workspace, "workspace.read").await?;
		let area = record.area_id.ok_or(Error::Forbidden)?;
		access
			.require(
				&access.resource(
					"working_area",
					area,
					json!({"owner":record.owner,"agent_id":description.source_agent.id,"thread_id":record.data["thread_id"]}),
				),
				"file.share",
			)
			.await?;
		access
			.require(
				&access.resource("node", &description.target.node_id, json!({})),
				"file.transfer",
			)
			.await?;
		let entry = catalog::entry(&mut access, &description.source_agent, "agent.execute").await?;
		if !serde_json::from_value::<crate::registry::AgentConfig>(entry.config)?
			.core_capabilities
			.sharing
		{
			return Err(Error::Forbidden);
		}
		access.subjects.push(crate::domain::qualified_agent(
			&description.target.node_id,
			&description.target.agent_id,
			&description.target.agent_version,
		));
		access
			.require(
				&access.resource(
					"working_area",
					area,
					json!({"owner":record.owner,"agent_id":description.source_agent.id,"thread_id":record.data["thread_id"]}),
				),
				"file.share",
			)
			.await?;
		sessions::authorize_sources(&mut access, run.workspace_id, &record.data["constraints"])
			.await?;
		Ok(())
	}
	.await;
	if let Err(e) = result {
		return access.finish(Err(e)).await;
	}
	Ok(access)
}
pub(crate) async fn describe(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Identity>,
) -> Result<Json<Description>> {
	let node = crate::api::peer_node(&headers)?;
	let record = snapshot(&f, input.transfer_id).await?;
	let description: Description = serde_json::from_value(record.data["description"].clone())?;
	if description.target.node_id != node || description.input_digest != input.input_digest {
		return Err(Error::Forbidden);
	}
	// A queued policy writer may sit between the sender's shared lease and
	// this verification. Bound the callback so it fails closed and releases
	// the sender lease rather than deadlocking a revocation.
	let access = tokio::time::timeout(std::time::Duration::from_secs(2), authority(&f, &record))
		.await
		.map_err(|_| Error::External("TRANSFER_AUTHORITY_UNAVAILABLE".into()))??;
	access.finish(Ok(Json(description))).await
}

pub fn routes() -> axum::Router<Federation> {
	use axum::routing::post;
	axum::Router::new()
		.route("/scoped/files/negotiate", post(receiver::negotiate))
		.route("/scoped/files/describe", post(describe))
		.route("/scoped/files/prepare", post(receiver::prepare))
		.route("/scoped/files/chunk", post(receiver::chunk))
		.route("/scoped/files/commit", post(receiver::commit))
		.route("/scoped/files/status", post(receiver::status))
		.route("/scoped/files/recipients", post(receiver::recipients))
		.layer(axum::extract::DefaultBodyLimit::max(6 * 1024 * 1024))
}

fn receipt_matches(description: &Description, response: &Value) -> bool {
	let Some(files) = response["receipt"]["files"].as_array() else {
		return false;
	};
	response["state"] == "committed"
		&& response["transfer_id"] == json!(description.transfer_id)
		&& response["input_digest"] == description.input_digest
		&& response["manifest_digest"] == description.manifest_digest
		&& response["receipt"]["node_id"] == description.target.node_id
		&& files.len() == description.files.len()
		&& files.iter().zip(&description.files).all(|(got, expected)| {
			got["digest"] == expected.digest
				&& got["size"] == expected.size
				&& got["path"] == format!("{}/{}", description.transfer_id, expected.path)
		})
}
async fn delivered(f: &Federation, id: Uuid, receipt: Value) -> Result<()> {
	// Recording an authenticated durable receipt is reconciliation of an
	// existing effect. It creates no access grant and must survive expiry of
	// the credential that originally authorized the transfer.
	let mut tx = f.store.pool.begin().await?;
	let mut record: Record = sqlx::query_as(
		&sessions::select("core_records")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("kind")).eq("transfer_out"))
			.lock(sea_orm::sea_query::LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_one(&mut *tx)
	.await?;
	let description: Description = serde_json::from_value(record.data["description"].clone())?;
	if !receipt_matches(&description, &receipt) {
		return Err(Error::Conflict("INVALID_TRANSFER_RECEIPT".into()));
	}
	record.state = "delivered".into();
	record.data["receipt"] = receipt["receipt"].clone();
	record.data["error"] = Value::Null;
	records::update_committed(&mut tx, &mut record).await?;
	tx.commit().await?;
	Ok(())
}
pub(crate) async fn reconcile(f: &Federation, id: Uuid) -> Result<()> {
	let record = snapshot(f, id).await?;
	let description: Description = serde_json::from_value(record.data["description"].clone())?;
	let response: Value = peer::authority_request(
		f,
		&description.target.node_id,
		"/scoped/files/status",
		&json!(Identity {
			transfer_id: id,
			input_digest: description.input_digest.clone()
		}),
	)
	.await?;
	if response["state"] == "committed" {
		return delivered(f, id, response).await;
	}
	if record.expires_at.is_none_or(|at| at <= Utc::now())
		|| record.data["objects_released"] == true
	{
		return Err(Error::Conflict("TRANSFER_EXPIRED".into()));
	}
	let mut access = authority(f, &record).await?;
	let mut record = records::get(&mut access, id, "transfer_out").await?;
	if record.state != "delivered" {
		record.state = if record.data["commit_attempted"] == true {
			"committing"
		} else {
			"transferring"
		}
		.into();
		record.data["retry_count"] = json!(0);
		record.data["retry_after"] = Value::Null;
		let result = records::update(&mut access, &mut record).await;
		return access.finish(result).await;
	}
	access.finish(Ok(())).await
}
async fn drive(f: &Federation, id: Uuid) -> Result<()> {
	let snapshot = snapshot(f, id).await?;
	let description: Description = serde_json::from_value(snapshot.data["description"].clone())?;
	let identity = Identity {
		transfer_id: id,
		input_digest: description.input_digest.clone(),
	};
	// A lost final reply is reconciled before considering another effect. This
	// never chooses a new transfer ID or republishes a receiver-owned snapshot.
	if snapshot.data["prepared"] == true {
		let status: Value = peer::authority_request(
			f,
			&description.target.node_id,
			"/scoped/files/status",
			&json!(identity),
		)
		.await?;
		if status["state"] == "committed" {
			return delivered(f, id, status).await;
		}
	}
	if snapshot.data["prepared"] != true {
		let access = authority(f, &snapshot).await?;
		access.finish(Ok(())).await?;
		let negotiation: Value = peer::authority_request(
			f,
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
		let accepted: Value = peer::authority_request(
			f,
			&description.target.node_id,
			"/scoped/files/prepare",
			&json!(identity),
		)
		.await?;
		if accepted["state"] == "committed" {
			return delivered(f, id, accepted).await;
		}
		if accepted["state"] != "staging"
			|| accepted["input_digest"] != description.input_digest
			|| accepted["manifest_digest"] != description.manifest_digest
		{
			return Err(Error::Conflict("TRANSFER_ADMISSION_CHANGED".into()));
		}
		let mut access = authority(f, &snapshot).await?;
		let mut record = records::get(&mut access, id, "transfer_out").await?;
		record.state = "transferring".into();
		record.data["prepared"] = json!(true);
		let result = records::update(&mut access, &mut record).await;
		return access.finish(result).await;
	}
	let mut access = authority(f, &snapshot).await?;
	let result = Box::pin(async {
		let mut record = records::get(&mut access, id, "transfer_out").await?;
		for (index, file) in description.files.iter().enumerate() {
			let offset = record.data["offsets"][index.to_string()]
				.as_u64()
				.unwrap_or(0);
			if offset < file.size {
				let bytes = f
					.store
					.capabilities
					.read_chunk(&mut access, file, offset)
					.await?;
				if bytes.is_empty() {
					return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
				}
				let accepted: Value = peer::authority_request(
					f,
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
				return records::update(&mut access, &mut record).await;
			}
		}
		if record.data["commit_attempted"] != true {
			record.data["commit_attempted"] = json!(true);
			record.state = "committing".into();
			return records::update(&mut access, &mut record).await;
		}
		// Hold the current source lease through receiver commit. Both ends
		// check their original identities; model input never supplies a token.
		let receipt: Value = peer::authority_request(
			f,
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
		records::update(&mut access, &mut record).await
	})
	.await;
	access.finish(result).await
}
pub async fn run(f: Federation, mut stopping: tokio::sync::watch::Receiver<bool>) -> Result<()> {
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		let ids: Vec<Uuid> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("core_records"))
				.and_where(Expr::col(Alias::new("kind")).eq("transfer_out"))
				.and_where(Expr::col(Alias::new("state")).is_in([
					"pending",
					"transferring",
					"committing",
				]))
				.and_where(Expr::cust(
					"COALESCE((data->>'retry_after')::timestamptz, '-infinity'::timestamptz) < CURRENT_TIMESTAMP",
				))
				.limit(8)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&f.store.pool)
		.await?;
		for id in ids {
			if let Err(error) = Box::pin(drive(&f, id)).await {
				let terminal = matches!(
					error,
					Error::Forbidden | Error::Unauthorized | Error::NotFound(_)
				);
				let mut tx = f.store.pool.begin().await?;
				sqlx::query(&Query::update().table(Alias::new("core_records")).value(Alias::new("state"),Expr::cust("CASE WHEN $2 THEN 'blocked' WHEN COALESCE((data->>'retry_count')::int,0) >= 11 THEN 'uncertain' ELSE state END")).value(Alias::new("data"),Expr::cust("data || jsonb_build_object('error','TRANSFER_PENDING_OR_DENIED','retry_count',COALESCE((data->>'retry_count')::int,0)+1,'retry_after',CURRENT_TIMESTAMP + INTERVAL '5 seconds')")).and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1"))).and_where(Expr::col(Alias::new("state")).ne("delivered")).to_string(PostgresQueryBuilder)).bind(id).bind(terminal).execute(&mut *tx).await?;
				tx.commit().await?;
				tracing::warn!(%id,%error,"file transfer not acknowledged");
			}
		}
		tokio::select! {_=stopping.changed()=>{},_=tokio::time::sleep(std::time::Duration::from_millis(300))=>{}}
	}
}

pub(crate) use receiver::recipient_list;
