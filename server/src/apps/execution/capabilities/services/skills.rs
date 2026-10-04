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
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use uuid::Uuid;

fn content_digest(value: &SkillAttachment) -> String {
	aidash_domain::capabilities::skills::content_digest(value)
}
pub(crate) fn validate(value: &SkillAttachment) -> Result<SkillMetadata> {
	aidash_domain::capabilities::skills::validate(value).map_err(Into::into)
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
pub(crate) fn validate_config(value: &AgentConfig) -> Result<()> {
	aidash_domain::capabilities::skills::validate_config(value).map_err(Into::into)
}

fn escaped_instruction_len(text: &str) -> Result<usize> {
	Ok(serde_json::to_string(text)?.len().saturating_sub(2))
}

fn pinned_context_reserve(pinned: &[Pinned]) -> Result<usize> {
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

pub(crate) async fn context_headroom_reserve(store: &Store, run: &Run) -> Result<usize> {
	let data: Option<Value> = {
		let query_bind_1 = run.id;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("data"))
				.from(Alias::new("core_records"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())))
				.and_where(
					Expr::col(Alias::new("kind")).eq(reinhardt::query::Expr::value("skills")),
				)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&store.pool)
		.await?
	};
	let Some(data) = data else {
		return Ok(0);
	};
	let pinned: Vec<Pinned> = serde_json::from_value(data)?;
	pinned_context_reserve(&pinned)
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
			instruction_json_len,
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

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

pub use crate::apps::execution::capabilities::serializers::skills::{
	SkillAttachment, SkillList, SkillLoad, SkillMetadata, SkillRead,
};

pub(crate) use crate::apps::execution::capabilities::serializers::skills::Pinned;

#[cfg(test)]
#[path = "../tests/services_skills_review_tests.rs"]
mod review_tests;
