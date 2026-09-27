//! Direct immutable attachments and Run-pinned discovery inside virtual mounts.
use super::{contracts::*, objects, records, service};
use crate::{
	Error, Result,
	authorization::access::Access,
	domain::Run,
	registry::{AgentConfig, SkillFile},
	store::Store,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillAttachment {
	pub skill_id: Uuid,
	pub origin: String,
	pub digest: String,
	pub instructions: String,
	pub files: Vec<SkillFile>,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillMetadata {
	pub skill_id: Uuid,
	pub name: String,
	pub description: String,
	pub origin: String,
	pub digest: String,
	pub license: Option<String>,
}
#[derive(Debug, Deserialize)]
struct Frontmatter {
	name: String,
	description: String,
	license: Option<String>,
}
fn metadata(attachment: &SkillAttachment) -> Result<SkillMetadata> {
	let text = attachment.instructions.replace("\r\n", "\n");
	let front = text
		.strip_prefix("---\n")
		.and_then(|t| t.split_once("\n---"))
		.map(|p| p.0)
		.ok_or_else(|| Error::Invalid("SKILL_FRONTMATTER_REQUIRED".into()))?;
	if front.len() > 8192 {
		return Err(Error::Invalid("SKILL_METADATA_LIMIT".into()));
	}
	let meta: Frontmatter = serde_saphyr::from_str(front)
		.map_err(|_| Error::Invalid("INVALID_SKILL_METADATA".into()))?;
	if meta.name.is_empty()
		|| meta.name.len() > 128
		|| meta.description.is_empty()
		|| meta.description.len() > 2048
		|| meta.license.as_ref().is_some_and(|s| s.len() > 2048)
	{
		return Err(Error::Invalid("SKILL_METADATA_LIMIT".into()));
	}
	Ok(SkillMetadata {
		skill_id: attachment.skill_id,
		name: meta.name,
		description: meta.description,
		license: meta.license,
		origin: attachment.origin.clone(),
		digest: attachment.digest.clone(),
	})
}
fn content_digest(a: &SkillAttachment) -> String {
	crate::registry::digest(&json!({"instructions":a.instructions,"files":a.files}))
}
pub(crate) fn validate(a: &SkillAttachment) -> Result<SkillMetadata> {
	if a.origin.is_empty()
		|| a.origin.len() > 2048
		|| a.instructions.len() > 65536
		|| a.instructions.contains('\0')
		|| a.files.len() + 1 > 64
	{
		return Err(Error::Invalid("SKILL_PACKAGE_LIMIT".into()));
	}
	let mut total = a.instructions.len();
	let mut paths = BTreeSet::from(["SKILL.md".to_owned()]);
	for file in &a.files {
		if !crate::registry::valid_skill_file_path(&file.path)
			|| !paths.insert(file.path.clone())
			|| file.content.contains('\0')
		{
			return Err(Error::Invalid("INVALID_SKILL_PATH".into()));
		}
		total += file.byte_len()?;
	}
	if total > 256_000 {
		return Err(Error::Invalid("SKILL_PACKAGE_LIMIT".into()));
	}
	if a.digest != content_digest(a) {
		return Err(Error::Conflict("SKILL_CONTENT_CHANGED".into()));
	}
	metadata(a)
}
pub(crate) fn imported(value: crate::skill_import::ImportedSkill) -> Result<SkillAttachment> {
	let mut a = SkillAttachment {
		skill_id: Uuid::new_v4(),
		origin: value.source,
		digest: String::new(),
		instructions: value.instructions,
		files: value.files,
	};
	a.files.sort_by(|a, b| a.path.cmp(&b.path));
	a.digest = content_digest(&a);
	validate(&a)?;
	Ok(a)
}
pub(crate) fn validate_config(config: &AgentConfig) -> Result<()> {
	if config.skill_attachments.len() > 16 || config.skill_roots.len() > 8 {
		return Err(Error::Invalid("SKILL_BINDING_LIMIT".into()));
	}
	if (!config.skill_attachments.is_empty() || !config.skill_roots.is_empty())
		&& !config.core_capabilities.skills
	{
		return Err(Error::Invalid(
			"direct Skills require core_capabilities.skills".into(),
		));
	}
	let mut ids = BTreeSet::new();
	for a in &config.skill_attachments {
		validate(a)?;
		if !ids.insert(a.skill_id) {
			return Err(Error::Invalid("DUPLICATE_SKILL_ID".into()));
		}
	}
	for root in &config.skill_roots {
		objects::validate_path(root)?;
		if root != ".agents/skills" && !root.ends_with("/.agents/skills") {
			return Err(Error::Invalid(
				"Skill discovery requires a mounted .agents/skills root".into(),
			));
		}
	}
	Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Pinned {
	#[serde(default)]
	loaded: bool,
	metadata: SkillMetadata,
	files: Vec<FileEntry>,
}
pub(crate) async fn pin(
	store: &Store,
	access: &mut Access,
	run: Uuid,
	area: &Area,
	config: &AgentConfig,
) -> Result<()> {
	if !config.core_capabilities.skills {
		return Ok(());
	}
	let mut attachments = config.skill_attachments.clone();
	let manifest = service::files(area)?;
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
				if total > store.capabilities.0.limits.skill_bytes as u64
					|| files.len() >= store.capabilities.0.limits.skill_files
				{
					return Err(Error::Invalid("SKILL_PACKAGE_LIMIT".into()));
				}
				let bytes = store.capabilities.read(access, file).await?;
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
		if a.files.len() + 1 > store.capabilities.0.limits.skill_files {
			return Err(Error::Invalid("SKILL_PACKAGE_LIMIT".into()));
		}
		let mut package_bytes = 0;
		let metadata = validate(&a)?;
		let mut files = vec![];
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
			package_bytes += bytes.len();
			if package_bytes > store.capabilities.0.limits.skill_bytes {
				return Err(Error::Invalid("SKILL_PACKAGE_LIMIT".into()));
			}
			let (id, digest) = store
				.capabilities
				.put(access, Some(area.id), "skill", &bytes)
				.await?;
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
		});
	}
	records::insert(
		access,
		run,
		Some(area.id),
		"skills",
		"pinned",
		json!(pinned),
		None,
	)
	.await?;
	Ok(())
}
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillList {
	pub cursor: Option<usize>,
	pub limit: Option<usize>,
}
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillLoad {
	pub skill_id: Uuid,
	pub expected_digest: String,
}
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillRead {
	pub skill_id: Uuid,
	pub digest: String,
	pub path: String,
	pub offset: Option<usize>,
	pub max_bytes: Option<usize>,
}
pub(crate) async fn invoke(
	store: &Store,
	access: &mut Access,
	run: &Run,
	name: &str,
	input: Value,
) -> Result<Value> {
	let mut record = records::get(access, run.id, "skills").await?;
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
			p.max_bytes
				.unwrap_or(store.capabilities.0.limits.read_bytes),
		)
	};
	let skill = pinned
		.iter_mut()
		.find(|p| p.metadata.skill_id == id)
		.ok_or_else(|| Error::NotFound("skill unavailable".into()))?;
	if skill.metadata.digest != digest {
		return Err(Error::Conflict("SKILL_CONTENT_CHANGED".into()));
	}
	objects::validate_path(&path)?;
	let file = skill
		.files
		.iter()
		.find(|f| f.path == path)
		.ok_or_else(|| Error::NotFound("skill file unavailable".into()))?;
	let bytes = store.capabilities.read(access, file).await?;
	let Ok(text) = std::str::from_utf8(&bytes) else {
		return Ok(
			json!({"skill":skill.metadata,"metadata":file,"encoding":"binary","truncated":false}),
		);
	};
	let maximum = if name == "skill_load" {
		65536
	} else {
		store.capabilities.0.limits.read_bytes
	};
	if limit == 0 || limit > maximum || offset > text.len() || !text.is_char_boundary(offset) {
		return Err(Error::Invalid("INVALID_READ_RANGE".into()));
	}
	let mut end = (offset + limit).min(text.len());
	while !text.is_char_boundary(end) {
		end -= 1;
	}
	if end == offset && offset < text.len() {
		return Err(Error::Invalid("READ_BUDGET".into()));
	}
	let result = json!({"skill":skill.metadata,"path":path,"content":&text[offset..end],"digest":file.digest,"next_offset":(end<text.len()).then_some(end),"truncated":end<text.len(),"files":if name=="skill_load"{json!(skill.files.iter().map(|f|json!({"path":f.path,"digest":f.digest,"size":f.size})).collect::<Vec<_>>())}else{Value::Null}});
	if name == "skill_load" {
		skill.loaded = true;
		record.data = json!(pinned);
		records::update(access, &mut record).await?;
	}
	Ok(result)
}
/// Loaded instructions stay outside compactable history. Request budgeting must
/// reject an oversized complete system prompt before asking a provider.
pub(crate) async fn context(store: &Store, access: &mut Access, run: &Run) -> Result<String> {
	super::sessions::context_authority(access, run).await?;
	access
		.require(
			&access.resource("tool", "builtin:skill_list", json!({})),
			"tool.invoke",
		)
		.await?;
	let record = records::get(access, run.id, "skills").await?;
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
			let bytes = store.capabilities.read(access, file).await?;
			text.push_str(std::str::from_utf8(&bytes).map_err(|_| Error::Forbidden)?);
			text.push('\n');
		}
	}
	Ok(text)
}

pub(crate) async fn mounted(access: &mut Access, run: Uuid) -> Result<Vec<FileEntry>> {
	let record = match records::get(access, run, "skills").await {
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
