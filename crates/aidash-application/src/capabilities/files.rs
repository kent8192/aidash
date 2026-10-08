//! File tools share current authority, immutable content and generation-bound search cursors.
use crate::{
	Error, Result,
	ports::capabilities::files::{Action, FileScopePort, VerifiedFile},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		errors::CapabilityError,
		files::*,
		operations::{FileScope, MountedFile as FileEntry, available},
		sessions::Area,
	},
	model::ModelConfig,
	provider::ContentPart,
	registry::{AgentConfig, EntityRef},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use uuid::Uuid;
fn files(area: &Area) -> Result<Vec<FileEntry>> {
	Ok(serde_json::from_value(area.manifest.clone())?)
}
pub async fn settings(scope: &mut dyn FileScopePort, run: &RunMetadata) -> Result<AgentConfig> {
	let entry = scope
		.entry(
			&EntityRef {
				id: run.agent_id.clone(),
				version: run.agent_version.clone(),
			},
			"agent.execute",
		)
		.await?;
	scope.check_pinned(&entry).await?;
	Ok(AgentConfig::from_snapshot(scope.binding_snapshot()?)?)
}
pub async fn output_file(
	scope: &mut dyn FileScopePort,
	area: &Area,
	id: Uuid,
) -> Result<FileEntry> {
	let row: Option<(String, i64, String)> = scope.output(area, id).await?;
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
pub async fn invoke(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	name: &str,
	input: Value,
	key: &str,
) -> Result<Envelope> {
	settings(scope, run).await?;
	let binding = scope.binding_snapshot()?.operation(name)?.clone();
	if binding.identity.registry_node != scope.local_node() {
		return Err(Error::Forbidden);
	}
	let current = scope
		.entry(&binding.identity.local(), "registry.read")
		.await?;
	scope.check_pinned(&current).await?;
	if aidash_domain::registry::rules::digest(&serde_json::to_value(&current)?) != binding.digest {
		return Err(Error::Conflict(
			"admitted Binding definition changed".into(),
		));
	}
	let mut input = input;
	binding.narrow.apply(&mut input)?;
	scope
		.require(
			&scope.resource(
				"tool",
				&binding.identity.resource_id(),
				crate::authorization::catalog::attributes(&current),
			),
			"tool.invoke",
		)
		.await?;
	if name == "file_share" {
		scope.serialize_sharing().await?;
	}
	let mut area = scope.for_run(run).await?;
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
		available(&area.state)?;
	}
	if matches!(name, "file_read" | "file_search") {
		scope.require_current(&area, run).await?;
	}
	let mut result = match name {
		"python_install" => {
			scope
				.execute(Action::Package, &mut area, input, key)
				.await?
		}
		"code_interpreter" => scope.execute(Action::Python, &mut area, input, key).await?,
		"outbound_get" => {
			scope
				.execute(Action::Outbound, &mut area, input, key)
				.await?
		}
		"file_share" => scope.execute(Action::Share, &mut area, input, key).await?,
		"skill_list" | "skill_load" | "skill_read" => {
			scope
				.execute(Action::Skill(name), &mut area, input, key)
				.await?
		}
		"apply_patch" => scope.execute(Action::Patch, &mut area, input, key).await?,
		"shell" => scope.execute(Action::Shell, &mut area, input, key).await?,
		"shell_poll" | "shell_cancel" | "python_poll" | "python_cancel" => {
			scope
				.execute(
					Action::Control {
						kind: if name.starts_with("python_") {
							"code_interpreter"
						} else {
							"shell"
						},
						cancel: matches!(name, "shell_cancel" | "python_cancel"),
					},
					&mut area,
					input,
					key,
				)
				.await?
		}
		"file_read" => {
			let selection: FileRead =
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
			if matches!(selection.representation, Representation::ModelInput) {
				let file = files(&area)?
					.into_iter()
					.find(|file| file.file_id == selection.file_id)
					.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
				ContentPart::modality_for_media_type(&file.media_type)?;
				let agent = scope
					.effective(&EntityRef {
						id: run.agent_id.clone(),
						version: run.agent_version.clone(),
					})
					.await?;
				let agent: AgentConfig = serde_json::from_value(agent.config)?;
				let entry = scope.entry(&agent.model, "registry.read").await?;
				let effective = scope
					.effective(&EntityRef {
						id: entry.id,
						version: entry.version,
					})
					.await?;
				let model: ModelConfig = serde_json::from_value(effective.config)?;
				model.require_media_types([file.media_type.as_str()])?;
			}
			read(scope, &area, selection).await?
		}
		"file_search" => tokio::time::timeout(
			std::time::Duration::from_secs(scope.limits()?.search_seconds),
			search(
				scope,
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
			*error = serde_json::to_value(CapabilityError::stored(error))?;
		}
	}
	Ok(Envelope {
		operation_id,
		status,
		policy_revision: scope.policy_revision(),
		area_id: area.id,
		generation,
		revision,
		result,
	})
}
pub async fn read(scope: &mut dyn FileScopePort, area: &Area, input: FileRead) -> Result<Value> {
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
	if matches!(input.representation, Representation::ModelInput) {
		if input.expected_digest.as_deref() != Some(file.digest.as_str())
			|| input.offset.is_some()
			|| input.max_bytes.is_some()
		{
			return Err(Error::Invalid(
				"model input requires an exact digest and no read range".into(),
			));
		}
		if file.size > 8 * 1024 * 1024 {
			return Err(Error::Invalid(
				"model media input exceeds byte limit".into(),
			));
		}
		ContentPart::modality_for_media_type(&file.media_type)?;
		return Ok(json!({"metadata":file,"digest":file.digest,"truncated":false}));
	}
	if matches!(input.representation, Representation::Metadata) {
		return Ok(json!({"metadata":file,"digest":file.digest,"truncated":false}));
	}
	let offset = input.offset.unwrap_or(0);
	let limit = input.max_bytes.unwrap_or(scope.limits()?.read_bytes);
	if limit == 0 || limit > scope.limits()?.read_bytes || offset as u64 > file.size {
		return Err(Error::Invalid("INVALID_READ_RANGE".into()));
	}
	let mut object = tokio::time::timeout(
		std::time::Duration::from_secs(scope.limits()?.search_seconds),
		scope.open(&file),
	)
	.await
	.map_err(|_| Error::Invalid("READ_TIME_LIMIT".into()))??;
	let bytes = object.read_range(offset as u64, limit as u64).await?;
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
pub async fn search(
	scope: &mut dyn FileScopePort,
	area: &Area,
	input: FileSearch,
) -> Result<Value> {
	let limit = input.limit.unwrap_or(scope.limits()?.search_matches);
	if input.query.is_empty()
		|| input.query.len() > 1024
		|| !(1..=scope.limits()?.search_matches).contains(&limit)
	{
		return Err(Error::Invalid("INVALID_SEARCH_BUDGET".into()));
	}
	if let Some(path) = &input.path {
		aidash_domain::registry::rules::validate_path(path)?;
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
	let hash = aidash_domain::registry::rules::digest(&json!([
		input.query,
		input.mode,
		input.scope,
		input.path
	]));
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
		if started.elapsed() > std::time::Duration::from_secs(scope.limits()?.search_seconds) {
			return Err(Error::Invalid("SEARCH_TIME_LIMIT".into()));
		}
		let file = &files[cursor.file];
		if serde_json::to_value(file.scope)? != serde_json::to_value(input.scope)?
			|| input.path.as_ref().is_some_and(|path| {
				file.path != *path && !file.path.starts_with(&format!("{path}/"))
			}) {
			cursor.file += 1;
			cursor.line = 0;
			continue;
		}
		let mut reader: Box<dyn VerifiedFile> = if matches!(input.mode, SearchMode::Path) {
			Box::new(MemoryReader {
				bytes: file.path.as_bytes().to_vec(),
				offset: 0,
			})
		} else {
			scope.open(file).await?
		};
		let mut exhausted = true;
		let mut index = 0;
		while let Some(line) = bounded_line(&mut *reader).await? {
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
					|| bytes > scope.limits()?.search_bytes.saturating_sub(2768)
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
					> std::time::Duration::from_secs(scope.limits()?.search_seconds)
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
					|| byte_count > scope.limits()?.search_bytes.saturating_sub(2768)
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
async fn bounded_line(reader: &mut dyn VerifiedFile) -> Result<Option<Vec<u8>>> {
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
struct MemoryReader {
	bytes: Vec<u8>,
	offset: usize,
}
#[async_trait::async_trait]
impl VerifiedFile for MemoryReader {
	async fn read_range(&mut self, offset: u64, limit: u64) -> Result<Vec<u8>> {
		let offset =
			usize::try_from(offset).map_err(|_| Error::Invalid("INVALID_READ_RANGE".into()))?;
		if offset > self.bytes.len() {
			return Err(Error::Invalid("INVALID_READ_RANGE".into()));
		}
		Ok(
			self.bytes[offset..self.bytes.len().min(offset.saturating_add(limit as usize))]
				.to_vec(),
		)
	}
	async fn fill_buf(&mut self) -> Result<&[u8]> {
		Ok(&self.bytes[self.offset..])
	}
	fn consume(&mut self, count: usize) {
		self.offset = self.bytes.len().min(self.offset + count);
	}
}
pub async fn materialize(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	input: Materialize,
) -> Result<Value> {
	if !settings(scope, run).await?.core_capabilities.files {
		return Err(Error::Forbidden);
	}
	let mut area = scope.for_run(run).await?;
	available(&area.state)?;
	scope.authorize(&area, "file.write").await?;
	let digest = aidash_domain::registry::rules::digest(&json!(["materialize", run.id, input]));
	if let Some(cached) = scope.cached(input.idempotency_key, &digest).await? {
		return Ok(cached);
	}
	if scope.current_run(&area).await? != Some(run.id) {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	if input.expected_revision != area.revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	aidash_domain::registry::rules::validate_path(&input.path)?;
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
			None => output_file(scope, &area, *file_id).await?,
		};
		if original.digest != *expected_digest {
			return Err(Error::Conflict("FILE_CHANGED".into()));
		}
		if entries.iter().map(|f| f.size).sum::<u64>() + original.size
			> scope.limits()?.working_bytes
		{
			return Err(Error::Conflict("WORKING_QUOTA".into()));
		}
		scope.begin_pending(area.id, original.size).await?;
		let mut offset = 0;
		while offset < original.size {
			let block = scope.read_chunk(&original, offset).await?;
			if block.is_empty() {
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			offset += block.len() as u64;
			scope.write_pending(&block).await?;
		}
		let (file_id, digest_value) = scope.finish_pending(expected_digest).await?;
		let file = FileEntry {
			file_id,
			path: input.path,
			digest: digest_value,
			scope: FileScope::Working,
			..original
		};
		entries.push(file.clone());
		area.manifest = json!(entries);
		publish(scope, &mut area).await?;
		let result = json!({"area_id":area.id,"revision":area.revision,"file":file});
		scope.cache(input.idempotency_key, &digest, &result).await?;
		return Ok(result);
	}
	let (text, provenance, file_scope) = match input.source {
		MaterializeSource::File { .. } => unreachable!("file copies are streamed above"),
		MaterializeSource::Message { message_id } => {
			let message = scope.message(area.workspace_id, message_id).await?;
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
			let entry = scope
				.entry(
					&EntityRef {
						id: run.agent_id.clone(),
						version: run.agent_version.clone(),
					},
					"agent.execute",
				)
				.await?;
			let references = scope.documents(&entry).await?;
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
	if size > scope.limits()?.working_bytes {
		return Err(Error::Conflict("WORKING_QUOTA".into()));
	}
	let file = scope
		.text_file(area.id, input.path, &text, file_scope, provenance.clone())
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
	publish(scope, &mut area).await?;
	let result = json!({"area_id":area.id,"revision":area.revision,"file":file});
	scope.cache(input.idempotency_key, &digest, &result).await?;
	Ok(result)
}
pub async fn publish(scope: &mut dyn FileScopePort, area: &mut Area) -> Result<()> {
	// Commit the old working-object tombstones with the new manifest. Only
	// superseded working bytes are reclaimed, never outputs or recovery copies.
	let previous: Value = scope.previous_manifest(area).await?;
	let current = files(area)?;
	for old in serde_json::from_value::<Vec<FileEntry>>(previous)? {
		if matches!(old.scope, FileScope::Working)
			&& !current.iter().any(|file| file.file_id == old.file_id)
		{
			scope.supersede(area, old.file_id).await?;
		}
	}
	area.revision += 1;
	scope.persist_manifest(area).await?;
	scope
		.event(
			area.workspace_id,
			"capability.files_changed",
			json!({"area_id":area.id,"revision":area.revision,"generation":area.generation}),
		)
		.await?;
	Ok(())
}
#[cfg(test)]
mod tests;
