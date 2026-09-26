use super::*;
use axum::extract::State;
use sea_orm::sea_query::{LockType, Order};

const MAX_RECIPIENT_VERSIONS_PER_AREA: usize = 50;

fn cap_recipient_versions(versions: &mut Vec<String>) -> bool {
	let truncated = versions.len() > MAX_RECIPIENT_VERSIONS_PER_AREA;
	versions.truncate(MAX_RECIPIENT_VERSIONS_PER_AREA);
	truncated
}

async fn mapped(f: &Federation, source: &str, description: &Description) -> Result<(Access, Area)> {
	let mut access = peer::access(
		f,
		source,
		&description.source_tenant,
		&description.source_subject,
	)
	.await?;
	let result = async {
		if description.protocol != "file-transfer/1"
			|| description.source_node != source
			|| description.target.node_id != f.config.node_id
		{
			return Err(Error::Forbidden);
		}
		access
			.require(
				&access.resource("node", &f.config.node_id, json!({})),
				"file.transfer",
			)
			.await?;
		access.subjects.push(crate::domain::qualified_agent(
			&f.config.node_id,
			&description.target.agent_id,
			&description.target.agent_version,
		));
		let area: Area = sqlx::query_as(
			&sessions::select("core_areas")
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("owner")).eq(Expr::cust("$2")))
				.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::cust("$3")))
				.and_where(Expr::col(Alias::new("agent_id")).eq(Expr::cust("$4")))
				.and_where(Expr::col(Alias::new("home_node")).eq(Expr::cust("$5")))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&access.identity.tenant)
		.bind(&access.identity.subject)
		.bind(description.target.thread_id)
		.bind(&description.target.agent_id)
		.bind(&f.config.node_id)
		.fetch_optional(&mut **access.tx)
		.await?
		.ok_or_else(|| Error::NotFound("recipient unavailable".into()))?;
		sessions::authorize(&mut access, &area, "file.receive").await?;
		access
			.require(
				&access.resource("node", &f.config.node_id, json!({})),
				"file.transfer",
			)
			.await?;
		let entry = catalog::entry(
			&mut access,
			&EntityRef {
				id: description.target.agent_id.clone(),
				version: description.target.agent_version.clone(),
			},
			"agent.execute",
		)
		.await?;
		if !serde_json::from_value::<crate::registry::AgentConfig>(entry.config)?
			.core_capabilities
			.sharing
		{
			return Err(Error::Forbidden);
		}
		let admitted: Option<Uuid> = sqlx::query_scalar(
			&Query::select()
				.column((Alias::new("r"), Alias::new("id")))
				.from_as(Alias::new("runs"), Alias::new("r"))
				.join_as(
					sea_orm::sea_query::JoinType::InnerJoin,
					Alias::new("core_runs"),
					Alias::new("q"),
					Expr::col((Alias::new("r"), Alias::new("id")))
						.equals((Alias::new("q"), Alias::new("run_id"))),
				)
				.and_where(Expr::col((Alias::new("q"), Alias::new("area_id"))).eq(Expr::cust("$1")))
				.and_where(
					Expr::col((Alias::new("q"), Alias::new("generation"))).eq(Expr::cust("$2")),
				)
				.and_where(
					Expr::col((Alias::new("r"), Alias::new("agent_version"))).eq(Expr::cust("$3")),
				)
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.bind(area.id)
		.bind(area.generation)
		.bind(&description.target.agent_version)
		.fetch_optional(&mut **access.tx)
		.await?;
		if admitted.is_none() {
			return Err(Error::NotFound("recipient version unavailable".into()));
		}
		Ok(area)
	}
	.await;
	match result {
		Ok(area) => Ok((access, area)),
		Err(e) => access.finish(Err(e)).await,
	}
}
fn validate(f: &Federation, description: &Description) -> Result<u64> {
	if description.files.is_empty()
		|| description.files.len() > f.store.capabilities.0.limits.share_files
		|| description.expires_at <= Utc::now()
		|| description.expires_at > Utc::now() + Duration::hours(24)
	{
		return Err(Error::Invalid("TRANSFER_MANIFEST_LIMIT".into()));
	}
	let mut paths = std::collections::BTreeSet::new();
	let mut total = 0;
	for file in &description.files {
		objects::validate_path(&file.path)?;
		if !paths.insert(&file.path)
			|| file.size > f.store.capabilities.0.limits.share_file_bytes
			|| file.digest.len() != 64
			|| !file
				.digest
				.bytes()
				.all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
		{
			return Err(Error::Invalid("TRANSFER_MANIFEST_LIMIT".into()));
		}
		total += file.size;
	}
	if total > f.store.capabilities.0.limits.share_bytes
		|| crate::registry::digest(&json!(description.files)) != description.manifest_digest
	{
		return Err(Error::Invalid("TRANSFER_MANIFEST_INTEGRITY".into()));
	}
	Ok(total)
}
fn view(record: &Record) -> Value {
	json!({"protocol":"file-transfer/1","transfer_id":record.id,"input_digest":record.data["description"]["input_digest"],"manifest_digest":record.data["description"]["manifest_digest"],"state":record.state,"receipt":record.data["receipt"]})
}
async fn current(
	f: &Federation,
	source: &str,
	input: &Identity,
) -> Result<(Access, Area, Record, Description)> {
	let record: Record = sqlx::query_as(
		&sessions::select("core_records")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("kind")).eq("transfer_in"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(input.transfer_id)
	.fetch_optional(&f.store.pool)
	.await?
	.ok_or_else(|| Error::NotFound("transfer unavailable".into()))?;
	let description: Description = serde_json::from_value(record.data["description"].clone())?;
	if description.source_node != source || description.input_digest != input.input_digest {
		return Err(Error::Forbidden);
	}
	let (mut access, area) = mapped(f, source, &description).await?;
	let record = records::get(&mut access, input.transfer_id, "transfer_in").await?;
	if record.data["mapped_credential"] != json!(access.identity.credential_id) {
		return Err(Error::Forbidden);
	}
	Ok((access, area, record, description))
}
pub(crate) async fn negotiate(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Requester>,
) -> Result<Json<Value>> {
	let node = crate::api::peer_node(&headers)?;
	let mut access = peer::access(&f, node, &input.tenant, &input.subject).await?;
	let result = access
		.require(
			&access.resource("node", &f.config.node_id, json!({})),
			"file.transfer",
		)
		.await;
	access.finish(result).await?;
	if !f.store.capabilities.0.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	Ok(Json(
		json!({"protocol":"file-transfer/1","node_id":f.config.node_id,"chunk_bytes":4194304,"maximum_files":f.store.capabilities.0.limits.share_files,"maximum_file_bytes":f.store.capabilities.0.limits.share_file_bytes,"maximum_total_bytes":f.store.capabilities.0.limits.share_bytes}),
	))
}
pub(crate) async fn prepare(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Identity>,
) -> Result<Json<Value>> {
	if !f.store.capabilities.0.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	let source = crate::api::peer_node(&headers)?;
	// No receiver policy locks are held across the callback to source authority.
	let description: Description =
		peer::authority_request(&f, source, "/scoped/files/describe", &json!(input)).await?;
	if description.transfer_id != input.transfer_id
		|| description.input_digest != input.input_digest
	{
		return Err(Error::Forbidden);
	}
	let total = validate(&f, &description)?;
	let (mut access, area) = mapped(&f, source, &description).await?;
	let result=async {
        match records::get(&mut access,input.transfer_id,"transfer_in").await {
            Ok(record) => {
                if record.data["description"]!=json!(description)||record.data["mapped_credential"]!=json!(access.identity.credential_id) {return Err(Error::Conflict("TRANSFER_IDENTITY_CHANGED".into()));}
                return Ok(view(&record));
            },
            Err(Error::NotFound(_)) => {},
            Err(error) => return Err(error),
        }
        service::available(&area)?;
        if service::files(&area)?.iter().map(|f|f.size).sum::<u64>()+total>f.store.capabilities.0.working_bytes {return Err(Error::Conflict("RECIPIENT_QUOTA".into()));}
        // Reserve both provisional chunks and final immutable objects. Actual
        // allocation consumes this reservation transactionally, never twice.
        f.store.capabilities.reserve(&mut access,(2*total) as i64).await?;
        let data=json!({"description":description,"generation":area.generation,"mapped_credential":access.identity.credential_id,"chunks":{},"reserved":2*total});
        let record=records::insert(&mut access,input.transfer_id,Some(area.id),"transfer_in","staging",data,Some(description.expires_at)).await?;
        Ok(view(&record))
    }.await;
	access.finish(result).await.map(Json)
}
pub(crate) async fn chunk(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Chunk>,
) -> Result<Json<Value>> {
	if !f.store.capabilities.0.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	let source = crate::api::peer_node(&headers)?;
	let identity = Identity {
		transfer_id: input.transfer_id,
		input_digest: input.input_digest,
	};
	let (mut access, area, mut record, description) = current(&f, source, &identity).await?;
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
			|| input.offset % 4194304 != 0
			|| bytes.len() as u64 != (file.size - input.offset).min(4194304)
		{
			return Err(Error::Invalid("TRANSFER_CHUNK_RANGE".into()));
		}
		let key = format!("{}:{}", input.file, input.offset);
		if let Some(old) = record.data["chunks"].get(&key) {
			if old["digest"] != objects::digest(&bytes) || old["size"] != bytes.len() {
				return Err(Error::Conflict("CHUNK_CHANGED".into()));
			}
			return Ok(view(&record));
		}
		let reserved = record.data["reserved"].as_i64().ok_or(Error::Forbidden)?;
		if reserved < bytes.len() as i64 {
			return Err(Error::Conflict("TRANSFER_RESERVATION_CHANGED".into()));
		}
		f.store
			.capabilities
			.reserve(&mut access, -(bytes.len() as i64))
			.await?;
		let (file_id, digest) = f
			.store
			.capabilities
			.put(&mut access, None, "transfer_staging", &bytes)
			.await?;
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
		records::update(&mut access, &mut record).await?;
		Ok(view(&record))
	}
	.await;
	access.finish(result).await.map(Json)
}
pub(crate) async fn commit(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Identity>,
) -> Result<Json<Value>> {
	if !f.store.capabilities.0.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	let source = crate::api::peer_node(&headers)?;
	// Authenticate the original subject at its authority again, independently
	// of the transport credential. Never hold receiver locks for this callback.
	let fresh: Description =
		peer::authority_request(&f, source, "/scoped/files/describe", &json!(input)).await?;
	let (mut access, mut area, mut record, description) = current(&f, source, &input).await?;
	if json!(fresh) != json!(description) {
		return Err(Error::Forbidden);
	}
	let result=async {
        if record.state=="committed" {return Ok(view(&record));}
        if record.state!="staging"||description.expires_at<=Utc::now()||record.data["generation"]!=area.generation {return Err(Error::Conflict("TRANSFER_UNAVAILABLE".into()));}
        // The source worker retains its current lease through this RPC and
        // the receiver retains its own lease through atomic publication.
        service::available(&area)?;
        let mut entries=service::files(&area)?;
        let total=description.files.iter().map(|f|f.size).sum::<u64>();
        if entries.len()+description.files.len()>4096||entries.iter().map(|f|f.size).sum::<u64>()+total>f.store.capabilities.0.working_bytes {return Err(Error::Conflict("RECIPIENT_QUOTA".into()));}
        let mut delivered=vec![];
        for (index,file) in description.files.iter().enumerate() {
            let reserved=record.data["reserved"].as_i64().ok_or(Error::Forbidden)?;
            if reserved<file.size as i64 {return Err(Error::Conflict("TRANSFER_RESERVATION_CHANGED".into()));}
            f.store.capabilities.reserve(&mut access,-(file.size as i64)).await?;
            record.data["reserved"]=json!(reserved-file.size as i64);
            let mut object=f.store.capabilities.begin_object(&mut access,Some(area.id),"received",file.size).await?;
            let mut offset=0;
            while offset<file.size {
                let chunk:FileEntry=serde_json::from_value(record.data["chunks"][format!("{index}:{offset}")].clone()).map_err(|_|Error::Conflict("TRANSFER_INCOMPLETE".into()))?;
                object.write_block(&f.store.capabilities.read(&mut access,&chunk).await?).await?;offset+=chunk.size;
            }
            let (file_id,digest)=object.finish(&mut access,Some(&file.digest)).await?;
            let received=FileEntry{file_id,digest,path:format!("{}/{}",record.id,file.path),scope:FileScope::Received,provenance:json!({"kind":"received","transfer_id":record.id,"source_node":source,"source_agent":description.source_agent}),..file.clone()};
            objects::validate_path(&received.path)?;delivered.push(received.clone());entries.push(received);
        }
        area.manifest=json!(entries);
        area.constraints.as_array_mut().ok_or(Error::Forbidden)?.push(json!({"kind":"received_scope","transfer_id":record.id,"owner":area.owner,"agent":crate::domain::qualified_agent(&f.config.node_id,&description.target.agent_id,&description.target.agent_version)}));
        service::publish(&f.store,&mut access,&mut area).await?;
        record.state="committed".into();record.expires_at=None;
        record.data["receipt"]=json!({"id":record.id,"node_id":f.config.node_id,"area_id":area.id,"revision":area.revision,"manifest_digest":description.manifest_digest,"files":delivered});
        records::update(&mut access,&mut record).await?;
        f.store.event(&mut access.tx,Some(area.workspace_id),"capability.files_received",json!({"transfer_id":record.id,"area_id":area.id,"source_node":source})).await?;
        Ok(view(&record))
    }.await;
	access.finish(result).await.map(Json)
}
pub(crate) async fn status(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Identity>,
) -> Result<Json<Value>> {
	let source = crate::api::peer_node(&headers)?;
	let record: Record = sqlx::query_as(
		&sessions::select("core_records")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("kind")).eq("transfer_in"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(input.transfer_id)
	.fetch_optional(&f.store.pool)
	.await?
	.ok_or_else(|| Error::NotFound("transfer unavailable".into()))?;
	let description: Description = serde_json::from_value(record.data["description"].clone())?;
	if description.source_node != source || description.input_digest != input.input_digest {
		return Err(Error::Forbidden);
	}
	// A durable receipt survives recipient cleanup and withdrawal of future
	// sharing permission. Its metadata is still scoped to the original mapped
	// subject; status never returns original bytes or creates a new binding.
	let access = peer::access(
		&f,
		source,
		&description.source_tenant,
		&description.source_subject,
	)
	.await?;
	if record.tenant != access.identity.tenant
		|| record.owner != access.identity.subject
		|| record.data["mapped_credential"] != json!(access.identity.credential_id)
	{
		return Err(Error::Forbidden);
	}
	access.finish(Ok(Json(view(&record)))).await
}
pub(crate) async fn recipients(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Requester>,
) -> Result<Json<Value>> {
	let source = crate::api::peer_node(&headers)?;
	let mut access = peer::access(&f, source, &input.tenant, &input.subject).await?;
	let result = recipient_list(&f, &mut access, input.cursor).await;
	access.finish(result).await.map(Json)
}

pub(crate) async fn recipient_list(
	f: &Federation,
	access: &mut Access,
	cursor: Option<Uuid>,
) -> Result<Value> {
	access
		.require(
			&access.resource("node", &f.config.node_id, json!({})),
			"file.transfer",
		)
		.await?;
	let rows: Vec<Area> = sqlx::query_as(
		&sessions::select("core_areas")
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("owner")).eq(Expr::cust("$2")))
			.and_where(Expr::col(Alias::new("state")).eq("active"))
			.and_where(Expr::col(Alias::new("id")).gt(Expr::cust("$3")))
			.order_by(Alias::new("id"), Order::Asc)
			.limit(51)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&access.identity.tenant)
	.bind(&access.identity.subject)
	.bind(cursor.unwrap_or(Uuid::nil()))
	.fetch_all(&mut **access.tx)
	.await?;
	let next_cursor = if rows.len() == 51 {
		Some(rows[49].id)
	} else {
		None
	};
	let mut items = vec![];
	let mut versions_truncated = false;
	for area in rows.into_iter().take(50) {
		match sessions::authorize(access, &area, "file.receive").await {
			Ok(()) => {}
			Err(Error::Forbidden | Error::NotFound(_)) => continue,
			Err(error) => return Err(error),
		}
		let mut versions: Vec<String> = sqlx::query_scalar(
			&Query::select()
				.distinct()
				.column((Alias::new("r"), Alias::new("agent_version")))
				.from_as(Alias::new("runs"), Alias::new("r"))
				.join_as(
					sea_orm::sea_query::JoinType::InnerJoin,
					Alias::new("core_runs"),
					Alias::new("q"),
					Expr::col((Alias::new("q"), Alias::new("run_id")))
						.equals((Alias::new("r"), Alias::new("id"))),
				)
				.and_where(Expr::col((Alias::new("q"), Alias::new("area_id"))).eq(Expr::cust("$1")))
				.and_where(
					Expr::col((Alias::new("q"), Alias::new("generation"))).eq(Expr::cust("$2")),
				)
				.order_by((Alias::new("r"), Alias::new("agent_version")), Order::Asc)
				.limit((MAX_RECIPIENT_VERSIONS_PER_AREA + 1) as u64)
				.to_string(PostgresQueryBuilder),
		)
		.bind(area.id)
		.bind(area.generation)
		.fetch_all(&mut **access.tx)
		.await?;
		versions_truncated |= cap_recipient_versions(&mut versions);
		for version in versions {
			let subjects = access.subjects.clone();
			access.subjects.push(crate::domain::qualified_agent(
				&f.config.node_id,
				&area.agent_id,
				&version,
			));
			let allowed = async {
				sessions::authorize(access, &area, "file.receive").await?;
				let entry = catalog::entry(
					access,
					&EntityRef {
						id: area.agent_id.clone(),
						version: version.clone(),
					},
					"agent.execute",
				)
				.await?;
				if !serde_json::from_value::<crate::registry::AgentConfig>(entry.config)?
					.core_capabilities
					.sharing
				{
					return Err(Error::Forbidden);
				}
				Ok(())
			}
			.await;
			access.subjects = subjects;
			match allowed {
                    Ok(()) => items.push(json!({"node_id":f.config.node_id,"agent_id":area.agent_id,"agent_version":version,"thread_id":area.thread_id,"workspace_id":area.workspace_id})),
                    Err(Error::Forbidden | Error::NotFound(_)) => {}, Err(error) => return Err(error),
                }
		}
	}
	Ok(
		json!({"protocol":"file-transfer/1","items":items,"next_cursor":next_cursor,"versions_truncated":versions_truncated}),
	)
}

#[cfg(test)]
mod recipient_version_tests {
	use super::{MAX_RECIPIENT_VERSIONS_PER_AREA, cap_recipient_versions};

	#[test]
	fn recipient_versions_are_capped_and_report_truncation() {
		for (count, expected_truncated) in [(0, false), (50, false), (51, true)] {
			let mut versions = (0..count).map(|i| format!("0.0.{i}")).collect::<Vec<_>>();
			assert_eq!(cap_recipient_versions(&mut versions), expected_truncated);
			assert_eq!(versions.len(), count.min(MAX_RECIPIENT_VERSIONS_PER_AREA));
		}
	}
}
