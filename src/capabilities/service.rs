use super::{contracts::*, objects, sessions};
use crate::{
	Error, Result,
	authorization::{access::Access, catalog},
	domain::Run,
	registry::{AgentConfig, EntityRef},
	store::Store,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncSeekExt};

pub(crate) fn files(area: &Area) -> Result<Vec<FileEntry>> {
	Ok(serde_json::from_value(area.manifest.clone())?)
}
pub(crate) async fn output_file(
	access: &mut Access,
	area: &Area,
	id: uuid::Uuid,
) -> Result<FileEntry> {
	let row: Option<(String, i64, String)> = sqlx::query_as(
		&Query::select()
			.columns(["digest", "size", "kind"].map(Alias::new))
			.from(Alias::new("core_objects"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("area_id")).eq(Expr::cust("$2")))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$3")))
			.and_where(Expr::col(Alias::new("kind")).is_in(["output", "display", "network_output"]))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(area.id)
	.bind(&area.tenant)
	.fetch_optional(&mut **access.tx)
	.await?;
	let (digest, size, kind) = row.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
	Ok(FileEntry {
		file_id: id,
		path: id.to_string(),
		digest,
		size: size as u64,
		scope: FileScope::Working,
		media_type: match kind.as_str() {
			"output" => "text/plain; charset=utf-8",
			"display" => "image/png",
			_ => "application/octet-stream",
		}
		.into(),
		provenance: json!({"kind":kind,"area_id":area.id}),
	})
}
pub(crate) fn available(area: &Area) -> Result<()> {
	if area.state != "active" {
		return Err(Error::Conflict(
			if area.state == "running" {
				"AREA_BUSY"
			} else {
				"AREA_UNAVAILABLE"
			}
			.into(),
		));
	}
	Ok(())
}
pub(crate) async fn settings(access: &mut Access, run: &Run) -> Result<AgentConfig> {
	let entry = catalog::entry(
		access,
		&EntityRef {
			id: run.agent_id.clone(),
			version: run.agent_version.clone(),
		},
		"agent.execute",
	)
	.await?;
	Ok(serde_json::from_value(entry.config)?)
}
pub(crate) async fn invoke(
	store: &Store,
	access: &mut Access,
	run: &Run,
	name: &str,
	input: Value,
	key: &str,
) -> Result<Envelope> {
	let config = settings(access, run).await?;
	if !config.core_capabilities.permits(name) {
		return Err(Error::Forbidden);
	}
	access
		.require(
			&access.resource("tool", format!("builtin:{name}"), json!({})),
			"tool.invoke",
		)
		.await?;
	if name == "file_share" {
		super::sharing::serialize(access).await?;
	}
	let mut area = sessions::for_run(access, run).await?;
	if !matches!(
		name,
		"shell"
			| "shell_poll"
			| "shell_cancel"
			| "code_interpreter"
			| "python_install"
			| "python_poll"
			| "python_cancel"
	) {
		available(&area)?;
	}
	if matches!(name, "file_read" | "file_search") {
		sessions::require_current_run(access, &area, run).await?;
	}
	let mut result = match name {
		"python_install" => {
			super::packages::prepare(
				store,
				access,
				run,
				&mut area,
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?,
			)
			.await?
		}
		"code_interpreter" => {
			super::python::prepare(
				store,
				access,
				run,
				&mut area,
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?,
				key,
			)
			.await?
		}
		"outbound_get" => {
			super::approvals::prepare(
				store,
				access,
				run,
				&area,
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?,
			)
			.await?
		}
		"file_share" => {
			super::sharing::share(
				store,
				access,
				run,
				&area,
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?,
			)
			.await?
		}
		"skill_list" | "skill_load" | "skill_read" => {
			super::skills::invoke(store, access, run, name, input).await?
		}
		"apply_patch" => {
			super::patch::apply(
				store,
				access,
				run,
				&mut area,
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?,
			)
			.await?
		}
		"shell" => {
			super::operations::prepare(
				store,
				access,
				run,
				&mut area,
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?,
				key,
			)
			.await?
		}
		"shell_poll" | "shell_cancel" | "python_poll" | "python_cancel" => {
			super::operations::poll(
				store,
				access,
				run,
				&area,
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?,
				if name.starts_with("python_") {
					"code_interpreter"
				} else {
					"shell"
				},
				matches!(name, "shell_cancel" | "python_cancel"),
			)
			.await?
		}
		"file_read" => {
			read(
				store,
				access,
				&area,
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?,
			)
			.await?
		}
		"file_search" => tokio::time::timeout(
			std::time::Duration::from_secs(store.capabilities.0.limits.search_seconds),
			search(
				store,
				access,
				&area,
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?,
			),
		)
		.await
		.map_err(|_| Error::Invalid("SEARCH_TIME_LIMIT".into()))??,
		_ => return Err(Error::Forbidden),
	};
	let operation_id = result
		.get_mut("operation_id")
		.map(Value::take)
		.and_then(|v| v.as_str().map(str::to_owned))
		.unwrap_or_else(|| key.to_owned());
	let status = result
		.get_mut("status")
		.map(Value::take)
		.and_then(|v| v.as_str().map(str::to_owned))
		.unwrap_or_else(|| "completed".into());
	let generation = result["generation"].as_i64().unwrap_or(area.generation);
	let revision = result["revision"].as_i64().unwrap_or(area.revision);
	if let Some(object) = result.as_object_mut() {
		for field in [
			"operation_id",
			"status",
			"policy_revision",
			"area_id",
			"generation",
			"revision",
		] {
			object.remove(field);
		}
		if let Some(error) = object.get_mut("error") {
			*error = serde_json::to_value(super::errors::CapabilityError::stored(error))?;
		}
	}
	Ok(Envelope {
		operation_id,
		status,
		policy_revision: access.snapshot.revision,
		area_id: area.id,
		generation,
		revision,
		result: serde_json::from_value(result)?,
	})
}
async fn read(store: &Store, access: &mut Access, area: &Area, input: FileRead) -> Result<Value> {
	let file = files(area)?
		.into_iter()
		.find(|f| f.file_id == input.file_id)
		.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
	if input
		.expected_digest
		.as_ref()
		.is_some_and(|digest| digest != &file.digest)
	{
		return Err(Error::Conflict("FILE_CHANGED".into()));
	}
	if matches!(input.representation, Representation::Metadata) {
		return Ok(json!({"metadata":file,"digest":file.digest,"truncated":false}));
	}
	let offset = input.offset.unwrap_or(0);
	let limit = input
		.max_bytes
		.unwrap_or(store.capabilities.0.limits.read_bytes);
	if limit == 0 || limit > store.capabilities.0.limits.read_bytes || offset as u64 > file.size {
		return Err(Error::Invalid("INVALID_READ_RANGE".into()));
	}
	let mut object = tokio::time::timeout(
		std::time::Duration::from_secs(store.capabilities.0.limits.search_seconds),
		store.capabilities.verified(access, &file),
	)
	.await
	.map_err(|_| Error::Invalid("READ_TIME_LIMIT".into()))??;
	object.seek(std::io::SeekFrom::Start(offset as u64)).await?;
	let mut bytes = vec![];
	object.take(limit as u64).read_to_end(&mut bytes).await?;
	let text = match std::str::from_utf8(&bytes) {
		Ok(text) => text,
		Err(error) if error.error_len().is_none() => {
			std::str::from_utf8(&bytes[..error.valid_up_to()]).map_err(|_| Error::Forbidden)?
		}
		Err(_) => {
			return Err(Error::Invalid(
				"REPRESENTATION_UNAVAILABLE_OR_INVALID_UTF8_OFFSET".into(),
			));
		}
	};
	let end = offset + text.len();
	if end == offset && (offset as u64) < file.size {
		return Err(Error::Invalid(
			"READ_BUDGET: max_bytes cannot contain the next UTF-8 character".into(),
		));
	}
	Ok(
		json!({"content":text,"metadata":file,"digest":file.digest,"encoding":"utf8","next_offset":((end as u64)<file.size).then_some(end),"truncated":(end as u64)<file.size}),
	)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
	area: uuid::Uuid,
	generation: i64,
	revision: i64,
	query: String,
	file: usize,
	line: usize,
}
async fn search(
	store: &Store,
	access: &mut Access,
	area: &Area,
	input: FileSearch,
) -> Result<Value> {
	let limit = input
		.limit
		.unwrap_or(store.capabilities.0.limits.search_matches);
	if input.query.is_empty()
		|| input.query.len() > 1024
		|| !(1..=store.capabilities.0.limits.search_matches).contains(&limit)
	{
		return Err(Error::Invalid("INVALID_SEARCH_BUDGET".into()));
	}
	if let Some(path) = &input.path {
		objects::validate_path(path)?;
	}
	let pattern = match input.mode {
		SearchMode::Regex => Some(
			regex::RegexBuilder::new(&input.query)
				.size_limit(65536)
				.build()
				.map_err(|_| Error::Invalid("INVALID_REGEX".into()))?,
		),
		_ => None,
	};
	let hash = crate::registry::digest(&json!([input.query, input.mode, input.scope, input.path]));
	let mut cursor = match &input.cursor {
		Some(encoded) if encoded.len() <= 2048 => serde_json::from_slice::<Cursor>(
			&URL_SAFE_NO_PAD
				.decode(encoded)
				.map_err(|_| Error::Invalid("INVALID_CURSOR".into()))?,
		)
		.map_err(|_| Error::Invalid("INVALID_CURSOR".into()))?,
		Some(_) => return Err(Error::Invalid("INVALID_CURSOR".into())),
		None => Cursor {
			area: area.id,
			generation: area.generation,
			revision: area.revision,
			query: hash.clone(),
			file: 0,
			line: 0,
		},
	};
	if cursor.area != area.id
		|| cursor.generation != area.generation
		|| cursor.revision != area.revision
		|| cursor.query != hash
	{
		return Err(Error::Conflict("STALE_CURSOR".into()));
	}
	let files = files(area)?;
	let started = std::time::Instant::now();
	let mut matches = vec![];
	let mut unavailable: Vec<Value> = vec![];
	while cursor.file < files.len() {
		if started.elapsed()
			> std::time::Duration::from_secs(store.capabilities.0.limits.search_seconds)
		{
			return Err(Error::Invalid("SEARCH_TIME_LIMIT".into()));
		}
		let file = &files[cursor.file];
		if serde_json::to_value(&file.scope)? != serde_json::to_value(&input.scope)?
			|| input.path.as_ref().is_some_and(|path| {
				file.path != *path && !file.path.starts_with(&format!("{path}/"))
			}) {
			cursor.file += 1;
			cursor.line = 0;
			continue;
		}
		let mut reader: Box<dyn AsyncBufRead + Unpin + Send> =
			if matches!(input.mode, SearchMode::Path) {
				Box::new(std::io::Cursor::new(file.path.as_bytes().to_vec()))
			} else {
				Box::new(tokio::io::BufReader::new(
					store.capabilities.verified(access, file).await?,
				))
			};
		let mut exhausted = true;
		let mut index = 0;
		while let Some(line) = bounded_line(&mut reader).await? {
			let index = {
				let current = index;
				index += 1;
				current
			};
			if index < cursor.line {
				continue;
			}
			let Ok(line) = std::str::from_utf8(&line) else {
				if input.path.as_deref() == Some(&file.path) {
					return Err(Error::Invalid(
						"REPRESENTATION_UNAVAILABLE: invalid UTF-8".into(),
					));
				}
				let item = json!({"file_id":file.file_id,"error":"REPRESENTATION_UNAVAILABLE"});
				let bytes = matches
					.iter()
					.chain(&unavailable)
					.map(|v: &Value| v.to_string().len())
					.sum::<usize>() + item.to_string().len();
				if matches.len() + unavailable.len() >= limit
					|| bytes
						> store
							.capabilities
							.0
							.limits
							.search_bytes
							.saturating_sub(2768)
				{
					cursor.line = index;
					exhausted = false;
					break;
				}
				unavailable.push(item);
				break;
			};
			if index % 64 == 0
				&& started.elapsed()
					> std::time::Duration::from_secs(store.capabilities.0.limits.search_seconds)
			{
				return Err(Error::Invalid("SEARCH_TIME_LIMIT".into()));
			}
			let found = pattern
				.as_ref()
				.map_or_else(|| line.contains(&input.query), |p| p.is_match(line));
			if found {
				let mut end = line.len().min(512);
				while !line.is_char_boundary(end) {
					end -= 1;
				}
				let item = json!({"file_id":file.file_id,"path":file.path,"digest":file.digest,"location":{"line":index+1,"source":file.provenance},"snippet":&line[..end]});
				let byte_count = matches
					.iter()
					.chain(&unavailable)
					.map(|v: &Value| v.to_string().len())
					.sum::<usize>() + item.to_string().len();
				if matches.len() + unavailable.len() >= limit
					|| byte_count
						> store
							.capabilities
							.0
							.limits
							.search_bytes
							.saturating_sub(2768)
				{
					if matches.is_empty() && unavailable.is_empty() {
						return Err(Error::Invalid(
							"SEARCH_RESULT_LIMIT: one match exceeds the configured page budget"
								.into(),
						));
					}
					cursor.line = index;
					exhausted = false;
					break;
				}
				matches.push(item);
			}
			cursor.line = index + 1;
		}
		if !exhausted {
			break;
		}
		cursor.file += 1;
		cursor.line = 0;
	}
	let next = (cursor.file < files.len()).then(|| {
		URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).expect("cursor serialization"))
	});
	Ok(
		json!({"matches":matches,"unavailable":unavailable,"truncated":next.is_some(),"next_cursor":next}),
	)
}

async fn bounded_line(reader: &mut (dyn AsyncBufRead + Unpin + Send)) -> Result<Option<Vec<u8>>> {
	let mut output = vec![];
	loop {
		let bytes = reader.fill_buf().await?;
		if bytes.is_empty() {
			return Ok((!output.is_empty()).then_some(output));
		}
		let end = bytes.iter().position(|b| *b == b'\n');
		let count = end.map_or(bytes.len(), |n| n + 1);
		if output.len() + count > 65536 {
			return Err(Error::Invalid(
				"SEARCH_LINE_LIMIT: select a smaller text representation".into(),
			));
		}
		output.extend_from_slice(&bytes[..count]);
		reader.consume(count);
		if end.is_some() {
			output.pop();
			return Ok(Some(output));
		}
	}
}
pub(crate) async fn materialize(
	store: &Store,
	access: &mut Access,
	run: &Run,
	input: Materialize,
) -> Result<Value> {
	if !settings(access, run).await?.core_capabilities.files {
		return Err(Error::Forbidden);
	}
	let mut area = sessions::for_run(access, run).await?;
	available(&area)?;
	sessions::authorize(access, &area, "file.write").await?;
	let digest = crate::registry::digest(&json!(["materialize", run.id, input]));
	if let Some(cached) = sessions::cached(access, input.idempotency_key, &digest).await? {
		return Ok(cached);
	}
	if sessions::status(access, &area).await?.active_run_id != Some(run.id) {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	if input.expected_revision != area.revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	objects::validate_path(&input.path)?;
	let mut entries = files(&area)?;
	if entries.len() >= 4096 || entries.iter().any(|f| f.path == input.path) {
		return Err(Error::Conflict("FILE_EXISTS_OR_LIMIT".into()));
	}
	if let MaterializeSource::File {
		file_id,
		expected_digest,
	} = &input.source
	{
		let original = match entries.iter().find(|f| f.file_id == *file_id).cloned() {
			Some(file) => file,
			None => output_file(access, &area, *file_id).await?,
		};
		if original.digest != *expected_digest {
			return Err(Error::Conflict("FILE_CHANGED".into()));
		}
		if entries.iter().map(|f| f.size).sum::<u64>() + original.size
			> store.capabilities.0.working_bytes
		{
			return Err(Error::Conflict("WORKING_QUOTA".into()));
		}
		let mut object = store
			.capabilities
			.begin_object(access, Some(area.id), "working", original.size)
			.await?;
		let mut offset = 0;
		while offset < original.size {
			let block = store
				.capabilities
				.read_chunk(access, &original, offset)
				.await?;
			if block.is_empty() {
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			offset += block.len() as u64;
			object.write_block(&block).await?;
		}
		let (file_id, digest_value) = object.finish(access, Some(expected_digest)).await?;
		let file = FileEntry {
			file_id,
			path: input.path,
			digest: digest_value,
			scope: FileScope::Working,
			..original
		};
		entries.push(file.clone());
		area.manifest = json!(entries);
		publish(store, access, &mut area).await?;
		let result = json!({"area_id":area.id,"revision":area.revision,"file":file});
		sessions::cache(access, input.idempotency_key, &digest, &result).await?;
		return Ok(result);
	}
	let (text, provenance, scope) = match input.source {
		MaterializeSource::File { .. } => unreachable!("file copies are streamed above"),
		MaterializeSource::Message { message_id } => {
			let message = access
				.workspace_record(area.workspace_id, "message", message_id)
				.await?;
			(
				message["content"]
					.as_str()
					.ok_or(Error::Forbidden)?
					.to_owned(),
				json!({"kind":"message","id":message_id}),
				FileScope::Working,
			)
		}
		MaterializeSource::ReferenceText { index } => {
			let entry = catalog::entry(
				access,
				&EntityRef {
					id: run.agent_id.clone(),
					version: run.agent_version.clone(),
				},
				"agent.execute",
			)
			.await?;
			let registry = crate::registry::Registry::new(store.pool.clone(), &store.node_id);
			let references = crate::knowledge::load(&registry.db, &entry).await?;
			let document = references
				.get(index)
				.ok_or_else(|| Error::NotFound("reference unavailable".into()))?;
			(
				document["text"]
					.as_str()
					.ok_or(Error::Forbidden)?
					.to_owned(),
				json!({"kind":"reference_text","agent":{"id":run.agent_id,"version":run.agent_version},"index":index,"name":document["name"]}),
				FileScope::References,
			)
		}
	};
	let size = entries.iter().map(|f| f.size).sum::<u64>() + text.len() as u64;
	if size > store.capabilities.0.working_bytes {
		return Err(Error::Conflict("WORKING_QUOTA".into()));
	}
	let file = store
		.capabilities
		.text_file(
			access,
			area.id,
			input.path,
			&text,
			scope,
			provenance.clone(),
		)
		.await?;
	entries.push(file.clone());
	area.manifest = json!(entries);
	let constraints = area.constraints.as_array_mut().ok_or(Error::Forbidden)?;
	if matches!(
		provenance["kind"].as_str(),
		Some("message" | "reference_text")
	) && !constraints.contains(&provenance)
	{
		constraints.push(provenance);
	}
	publish(store, access, &mut area).await?;
	let result = json!({"area_id":area.id,"revision":area.revision,"file":file});
	sessions::cache(access, input.idempotency_key, &digest, &result).await?;
	Ok(result)
}
pub(crate) async fn publish(store: &Store, access: &mut Access, area: &mut Area) -> Result<()> {
	// Commit the old working-object tombstones with the new manifest. Only
	// superseded working bytes are reclaimed, never outputs or recovery copies.
	let previous: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("manifest"))
			.from(Alias::new("core_areas"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(area.id)
	.fetch_one(&mut **access.tx)
	.await?;
	let current = files(area)?;
	for old in serde_json::from_value::<Vec<FileEntry>>(previous)? {
		if matches!(old.scope, FileScope::Working)
			&& !current.iter().any(|file| file.file_id == old.file_id)
		{
			sqlx::query(
				&Query::update()
					.table(Alias::new("core_objects"))
					.value(Alias::new("kind"), "superseded_working")
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.and_where(Expr::col(Alias::new("area_id")).eq(Expr::cust("$2")))
					.and_where(Expr::col(Alias::new("kind")).eq("working"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(old.file_id)
			.bind(area.id)
			.execute(&mut **access.tx)
			.await?;
		}
	}
	area.revision += 1;
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_areas"))
			.values([
				(Alias::new("manifest"), Expr::cust("$2")),
				(Alias::new("constraints"), Expr::cust("$3")),
				(Alias::new("revision"), Expr::cust("$4")),
			])
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(area.id)
	.bind(&area.manifest)
	.bind(&area.constraints)
	.bind(area.revision)
	.execute(&mut **access.tx)
	.await?;
	store
		.event(
			&mut access.tx,
			Some(area.workspace_id),
			"capability.files_changed",
			json!({"area_id":area.id,"revision":area.revision,"generation":area.generation}),
		)
		.await?;
	Ok(())
}
