//! Run-pinned discovery, complete prompt budgeting and explicit instruction loading.
use crate::{
	Error, Result,
	ports::capabilities::skills::{SkillHeadroom, SkillScope},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		SkillAttachment,
		operations::{FileScope, MountedFile as FileEntry},
		sessions::Area,
		skills::{Pinned, SkillList, SkillLoad, SkillRead, content_digest, validate},
	},
	exposure::{DirectSkill, SkillOriginKind},
	registry::{AgentConfig, SkillFile, bindings::SKILL_ASSET_READ},
};
use base64::Engine;
use serde_json::{Value, json};
use uuid::Uuid;
/// The instruction body of a direct Skill, distinct from its packaged assets.
const SKILL_BODY: &str = "SKILL.md";
pub fn escaped_instruction_len(text: &str) -> Result<usize> {
	Ok(serde_json::to_string(text)?.len().saturating_sub(2))
}
pub fn pinned_context_reserve(pinned: &[Pinned]) -> Result<usize> {
	let mut reserve =
		escaped_instruction_len("\nPinned Skills (select by UUID and origin; use skill_load):\n")?;
	for skill in pinned {
		reserve = reserve
			.saturating_add(escaped_instruction_len(&serde_json::to_string(
				&skill.metadata,
			)?)?)
			.saturating_add(escaped_instruction_len("\n")?);
		if skill.loaded {
			let file = skill
				.files
				.iter()
				.find(|file| file.path == "SKILL.md")
				.ok_or(Error::Forbidden)?;
			// Older pinned records lack the exact length. Six escaped JSON bytes
			// per source byte safely bounds all valid UTF-8 instruction text.
			let instruction_len = skill.instruction_json_len.unwrap_or_else(|| {
				usize::try_from(file.size)
					.unwrap_or(usize::MAX)
					.saturating_mul(6)
			});
			reserve = reserve
				.saturating_add(instruction_len)
				.saturating_add(escaped_instruction_len("\n")?);
		}
	}
	Ok(reserve)
}
pub async fn pin(
	scope: &mut dyn SkillScope,
	run: Uuid,
	area: &Area,
	config: &AgentConfig,
) -> Result<()> {
	if !config.core_capabilities.skills {
		return Ok(());
	}
	let mut attachments = config.skill_attachments.clone();
	let manifest = serde_json::from_value::<Vec<FileEntry>>(area.manifest.clone())?;
	for root in &config.skill_roots {
		for entry in &manifest {
			let Some(relative) = entry.path.strip_prefix(&format!("{root}/")) else {
				continue;
			};
			let Some((name, "SKILL.md")) = relative.split_once('/') else {
				continue;
			};
			if name.is_empty() {
				continue;
			}
			let prefix = format!("{root}/{name}/");
			let mut files = vec![];
			let mut total = 0;
			for file in manifest.iter().filter(|f| f.path.starts_with(&prefix)) {
				total += file.size;
				if total > scope.skill_limits()?.bytes as u64
					|| files.len() >= scope.skill_limits()?.files
				{
					return Err(Error::Invalid("SKILL_PACKAGE_LIMIT".into()));
				}
				let bytes = scope.read_skill_file(file).await?;
				let (text, encoding) = match String::from_utf8(bytes) {
					Ok(text) => (text, None),
					Err(error) => (
						base64::engine::general_purpose::STANDARD.encode(error.as_bytes()),
						Some("base64".into()),
					),
				};
				files.push(SkillFile {
					path: file.path[prefix.len()..].into(),
					content: text,
					encoding,
				});
			}
			let at = files
				.iter()
				.position(|f| f.path == "SKILL.md")
				.ok_or(Error::Forbidden)?;
			let instructions = files.remove(at);
			if instructions.encoding.is_some() {
				return Err(Error::Invalid("SKILL_INSTRUCTIONS_REQUIRE_UTF8".into()));
			}
			let instructions = instructions.content;
			files.sort_by(|a, b| a.path.cmp(&b.path));
			let mut a = SkillAttachment {
				skill_id: Uuid::new_v4(),
				origin: format!("area:{}:{root}/{name}", area.id),
				digest: String::new(),
				instructions,
				files,
			};
			a.digest = content_digest(&a);
			validate(&a)?;
			attachments.push(a);
		}
	}
	if attachments.len() > 32 {
		return Err(Error::Invalid("SKILL_BINDING_LIMIT".into()));
	}
	let mut pinned = vec![];
	for a in attachments {
		if a.files.len() + 1 > scope.skill_limits()?.files {
			return Err(Error::Invalid("SKILL_PACKAGE_LIMIT".into()));
		}
		let mut package_bytes = 0;
		let metadata = validate(&a)?;
		let mut files = vec![];
		let mut instruction_json_len = None;
		let inputs = std::iter::once(SkillFile {
			path: "SKILL.md".into(),
			content: a.instructions,
			encoding: None,
		})
		.chain(a.files);
		for file in inputs {
			use base64::Engine;
			let bytes = if file.encoding.as_deref() == Some("base64") {
				base64::engine::general_purpose::STANDARD
					.decode(&file.content)
					.map_err(|_| Error::Invalid("INVALID_SKILL_ENCODING".into()))?
			} else {
				file.content.into_bytes()
			};
			if file.path == "SKILL.md" {
				instruction_json_len = Some(escaped_instruction_len(
					std::str::from_utf8(&bytes).map_err(|_| Error::Forbidden)?,
				)?);
			}
			package_bytes += bytes.len();
			if package_bytes > scope.skill_limits()?.bytes {
				return Err(Error::Invalid("SKILL_PACKAGE_LIMIT".into()));
			}
			let (id, digest) = scope.put_skill(area.id, &bytes).await?;
			files.push(FileEntry {
				file_id: id,
				path: file.path,
				digest,
				size: bytes.len() as u64,
				media_type: "application/octet-stream".into(),
				scope: FileScope::References,
				provenance: json!({"kind":"skill","skill_id":a.skill_id,"origin":a.origin}),
			});
		}
		pinned.push(Pinned {
			metadata,
			files,
			loaded: false,
			instruction_json_len,
		});
	}
	scope.insert_skills(run, area.id, json!(pinned)).await?;
	Ok(())
}
pub async fn invoke(
	scope: &mut dyn SkillScope,
	run: &RunMetadata,
	name: &str,
	input: Value,
) -> Result<Value> {
	let mut record = scope.skill_record(run.id).await?;
	let mut pinned: Vec<Pinned> = serde_json::from_value(record.data.clone())?;
	if name == "skill_list" {
		let p: SkillList =
			serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
		let start = p.cursor.unwrap_or(0);
		let limit = p.limit.unwrap_or(16);
		if start > pinned.len() || !(1..=16).contains(&limit) {
			return Err(Error::Invalid("INVALID_SKILL_PAGE".into()));
		}
		let end = (start + limit).min(pinned.len());
		return Ok(
			json!({"skills":pinned[start..end].iter().map(|p|&p.metadata).collect::<Vec<_>>(),"next_cursor":(end<pinned.len()).then_some(end),"truncated":end<pinned.len()}),
		);
	}
	let (id, digest, path, offset, limit) = if name == "skill_load" {
		let p: SkillLoad =
			serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
		(
			p.skill_id,
			p.expected_digest,
			"SKILL.md".to_owned(),
			0,
			65536,
		)
	} else {
		let p: SkillRead =
			serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
		(
			p.skill_id,
			p.digest,
			p.path,
			p.offset.unwrap_or(0),
			p.max_chars.unwrap_or(scope.limits()?.read_bytes),
		)
	};
	let skill = pinned
		.iter_mut()
		.find(|p| p.metadata.skill_id == id)
		.ok_or_else(|| Error::NotFound("skill unavailable".into()))?;
	if skill.metadata.digest != digest {
		return Err(Error::Conflict("SKILL_CONTENT_CHANGED".into()));
	}
	aidash_domain::registry::rules::validate_path(&path)?;
	let file = skill
		.files
		.iter()
		.find(|f| f.path == path)
		.ok_or_else(|| Error::NotFound("skill file unavailable".into()))?;
	let bytes = scope.read_skill_file(file).await?;
	let Ok(text) = std::str::from_utf8(&bytes) else {
		return Ok(
			json!({"skill":skill.metadata,"metadata":file,"encoding":"binary","truncated":false}),
		);
	};
	let maximum = if name == "skill_load" {
		65536
	} else {
		scope.limits()?.read_bytes
	};
	let (content, next) = text_chunk(text, offset, limit, maximum)?;
	let result = json!({"skill":skill.metadata,"path":path,"content":content,"digest":file.digest,"next_offset":next,"truncated":next.is_some(),"files":if name=="skill_load"{json!(skill.files.iter().map(|f|json!({"path":f.path,"digest":f.digest,"size":f.size})).collect::<Vec<_>>())}else{Value::Null}});
	// Under `deferred@1` all load state lives in the Run's Exposure set.
	if name == "skill_load" && !deferred(scope)? {
		skill.loaded = true;
		record.data = json!(pinned);
		scope.update_skills(&mut record).await?;
	}
	Ok(result)
}
/// Scalar-value cursors are independent of the encoded-byte resource cap.
pub(crate) fn text_chunk(
	text: &str,
	offset: usize,
	max_chars: usize,
	byte_limit: usize,
) -> Result<(&str, Option<usize>)> {
	let total = text.chars().count();
	if max_chars == 0 || offset > total {
		return Err(Error::Invalid("INVALID_READ_RANGE".into()));
	}
	let start = text
		.char_indices()
		.nth(offset)
		.map_or(text.len(), |(index, _)| index);
	let mut end = start;
	let mut count = 0;
	for character in text[start..].chars().take(max_chars) {
		if end - start + character.len_utf8() > byte_limit {
			break;
		}
		end += character.len_utf8();
		count += 1;
	}
	if count == 0 && offset < total {
		return Err(Error::Invalid("READ_BUDGET".into()));
	}
	let next = offset + count;
	Ok((&text[start..end], (next < total).then_some(next)))
}
pub async fn context(scope: &mut dyn SkillScope, run: &RunMetadata) -> Result<String> {
	scope.context_authority(run).await?;
	let support = if deferred(scope)? {
		SKILL_ASSET_READ
	} else {
		"skill_list"
	};
	let resource = scope.resource(
		"tool",
		&scope
			.binding_snapshot()?
			.operation(support)?
			.identity
			.resource_id(),
		json!({}),
	);
	scope.require(&resource, "tool.invoke").await?;
	let record = scope.skill_record(run.id).await?;
	let pinned: Vec<Pinned> = serde_json::from_value(record.data)?;
	let mut text = String::from("\nPinned Skills (select by UUID and origin; use skill_load):\n");
	for skill in pinned {
		text.push_str(&serde_json::to_string(&skill.metadata)?);
		text.push('\n');
		if skill.loaded {
			let file = skill
				.files
				.iter()
				.find(|f| f.path == "SKILL.md")
				.ok_or(Error::Forbidden)?;
			let bytes = scope.read_skill_file(file).await?;
			text.push_str(std::str::from_utf8(&bytes).map_err(|_| Error::Forbidden)?);
			text.push('\n');
		}
	}
	Ok(text)
}
pub async fn mounted(scope: &mut dyn SkillScope, run: Uuid) -> Result<Vec<FileEntry>> {
	let record = match scope.skill_record(run).await {
		Ok(r) => r,
		Err(Error::NotFound(_)) => return Ok(vec![]),
		Err(e) => return Err(e),
	};
	let pinned: Vec<Pinned> = serde_json::from_value(record.data)?;
	Ok(pinned
		.into_iter()
		.flat_map(|p| {
			p.files.into_iter().map(move |mut f| {
				f.scope = FileScope::References;
				f.path = format!("_skills/{}/{}", p.metadata.skill_id, f.path);
				f
			})
		})
		.collect())
}
pub async fn context_headroom_reserve(scope: &mut dyn SkillHeadroom, run: Uuid) -> Result<usize> {
	let Some(data) = scope.pinned_data(run).await? else {
		return Ok(0);
	};
	pinned_context_reserve(&serde_json::from_value::<Vec<Pinned>>(data)?)
}

/// Runs without an admitted snapshot predate Exposure policies and are legacy.
fn deferred(scope: &dyn SkillScope) -> Result<bool> {
	match scope.binding_snapshot() {
		Ok(snapshot) => Ok(AgentConfig::from_snapshot(snapshot)?
			.exposure_policy()
			.is_deferred()),
		Err(_) => Ok(false),
	}
}

/// The pinned record behind deferred exposure, under the same context
/// authority as `context`, here over the `skill_asset_read` Binding. `None`
/// when Skills are off, the support tool is unbound or nothing is pinned.
async fn exposure_record(
	scope: &mut dyn SkillScope,
	run: &RunMetadata,
) -> Result<Option<(AgentConfig, Vec<Pinned>)>> {
	let snapshot = scope.binding_snapshot()?;
	let config = AgentConfig::from_snapshot(snapshot)?;
	let Some(support) = snapshot
		.operation(SKILL_ASSET_READ)
		.ok()
		.map(|binding| binding.identity.resource_id())
	else {
		return Ok(None);
	};
	if !config.core_capabilities.skills {
		return Ok(None);
	}
	scope.context_authority(run).await?;
	let resource = scope.resource("tool", &support, json!({}));
	scope.require(&resource, "tool.invoke").await?;
	let record = match scope.skill_record(run.id).await {
		Ok(record) => record,
		Err(Error::NotFound(_)) => return Ok(None),
		Err(error) => return Err(error),
	};
	Ok(Some((config, serde_json::from_value(record.data)?)))
}

fn pinned_skill(pinned: Vec<Pinned>, skill_id: Uuid, digest: &str) -> Result<Pinned> {
	let skill = pinned
		.into_iter()
		.find(|skill| skill.metadata.skill_id == skill_id)
		.ok_or_else(|| Error::NotFound("skill unavailable".into()))?;
	// Callers pass catalog digests, so a mismatch is drift, not model input.
	if skill.metadata.digest != digest {
		return Err(Error::Conflict("CAPABILITY_CHANGED".into()));
	}
	Ok(skill)
}

async fn instructions(scope: &mut dyn SkillScope, skill: &Pinned) -> Result<String> {
	let file = skill
		.files
		.iter()
		.find(|file| file.path == "SKILL.md")
		.ok_or(Error::Forbidden)?;
	String::from_utf8(scope.read_skill_file(file).await?).map_err(|_| Error::Forbidden)
}

/// Pinned direct Skills as Discoverable capabilities of a `deferred@1` Run.
pub async fn direct(scope: &mut dyn SkillScope, run: &RunMetadata) -> Result<Vec<DirectSkill>> {
	let Some((config, pinned)) = exposure_record(scope, run).await? else {
		return Ok(vec![]);
	};
	let mut skills = Vec::with_capacity(pinned.len());
	for skill in pinned {
		let body_bytes = match skill.instruction_json_len {
			Some(length) => length,
			None => escaped_instruction_len(&instructions(scope, &skill).await?)?,
		};
		let origin = if config
			.skill_attachments
			.iter()
			.any(|attachment| attachment.skill_id == skill.metadata.skill_id)
		{
			SkillOriginKind::Attachment
		} else {
			SkillOriginKind::Root
		};
		skills.push(DirectSkill {
			origin,
			body_bytes,
			files: skill
				.files
				.iter()
				.filter(|file| file.path != SKILL_BODY)
				.map(|file| json!({"path":file.path,"digest":file.digest,"size":file.size}))
				.collect(),
			metadata: skill.metadata,
		});
	}
	Ok(skills)
}

/// Exact SKILL.md text of one pinned direct Skill.
pub async fn direct_body(
	scope: &mut dyn SkillScope,
	run: &RunMetadata,
	skill_id: Uuid,
	digest: &str,
) -> Result<String> {
	let (_, pinned) = exposure_record(scope, run)
		.await?
		.ok_or_else(|| Error::NotFound("skill unavailable".into()))?;
	let skill = pinned_skill(pinned, skill_id, digest)?;
	instructions(scope, &skill).await
}

/// Bytes of one packaged file of a pinned direct Skill. SKILL.md is the
/// instruction body: only `capability_load` makes it resident, under the
/// `skill_bytes` budget, so it is never an asset.
pub async fn direct_file(
	scope: &mut dyn SkillScope,
	run: &RunMetadata,
	skill_id: Uuid,
	digest: &str,
	path: &str,
) -> Result<Vec<u8>> {
	aidash_domain::registry::rules::validate_path(path)?;
	if path == SKILL_BODY {
		return Err(Error::Invalid("SKILL_FILE_UNAVAILABLE".into()));
	}
	let (_, pinned) = exposure_record(scope, run)
		.await?
		.ok_or_else(|| Error::NotFound("skill unavailable".into()))?;
	let skill = pinned_skill(pinned, skill_id, digest)?;
	let file = skill
		.files
		.iter()
		.find(|file| file.path == path)
		.ok_or_else(|| Error::Invalid("SKILL_FILE_UNAVAILABLE".into()))?;
	scope.read_skill_file(file).await
}

/// An Agent's Skill attachment as a Discoverable capability before it is
/// pinned: registration and the Workbench sandbox measure it this way.
pub fn attachment_skill(attachment: &SkillAttachment) -> Result<DirectSkill> {
	use sha2::{Digest, Sha256};
	let metadata = validate(attachment)?;
	let mut files = Vec::with_capacity(attachment.files.len());
	for file in attachment
		.files
		.iter()
		.filter(|file| file.path != SKILL_BODY)
	{
		let bytes = if file.encoding.as_deref() == Some("base64") {
			base64::engine::general_purpose::STANDARD
				.decode(&file.content)
				.map_err(|_| Error::Invalid("INVALID_SKILL_ENCODING".into()))?
		} else {
			file.content.as_bytes().to_vec()
		};
		files.push((file.path.clone(), bytes));
	}
	Ok(DirectSkill {
		metadata,
		origin: SkillOriginKind::Attachment,
		body_bytes: escaped_instruction_len(&attachment.instructions)?,
		files: files
			.iter()
			.map(|(path, bytes)| {
				json!({"path":path,"digest":format!("sha256:{:x}", Sha256::digest(bytes)),"size":bytes.len()})
			})
			.collect(),
	})
}
#[cfg(test)]
mod tests;
