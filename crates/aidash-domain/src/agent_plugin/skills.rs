//! `skills/<dir>/SKILL.md`: exactly one directory level, never searched deeper.
use super::{Component, Finding, PluginFile, Reason};
use crate::registry::{SkillFile, rules::valid_skill_file_path};
use base64::Engine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const SKILLS_DIR: &str = "skills";
const SKILL_FILE: &str = "SKILL.md";
const MAX_SKILLS: usize = 16;
const MAX_NAME_LEN: usize = 64;
const MAX_DESCRIPTION_CHARS: usize = 1024;
const MAX_COMPATIBILITY_CHARS: usize = 500;

// The limits of a direct Skill (capabilities::skills::validate), so that an accepted plugin Skill
// is accepted again when it becomes one.
const MAX_INSTRUCTIONS_BYTES: usize = 65_536;
const MAX_FRONTMATTER_BYTES: usize = 8192;
const MAX_OTHER_FILES: usize = 63;
const MAX_TOTAL_BYTES: usize = 256_000;

/// A Skill as the plugin carries it. `allowed-tools` and `metadata` are checked for type and
/// dropped; `allowed-tools` never grants anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PluginSkill {
	/// The child directory under `skills/`.
	pub directory: String,
	pub name: String,
	pub description: String,
	pub license: Option<String>,
	pub compatibility: Option<String>,
	/// The full SKILL.md text.
	pub instructions: String,
	/// Every other file, relative to the Skill directory and sorted by path. Text stays text;
	/// anything else is base64.
	pub files: Vec<SkillFile>,
}

#[derive(Deserialize)]
struct Frontmatter {
	name: String,
	description: String,
	license: Option<String>,
	compatibility: Option<String>,
	#[serde(rename = "metadata")]
	_metadata: Option<BTreeMap<String, String>>,
	#[serde(rename = "allowed-tools")]
	_allowed_tools: Option<String>,
}

struct Refusal {
	reason: Reason,
	detail: String,
}

impl Refusal {
	fn invalid(detail: impl Into<String>) -> Self {
		Self {
			reason: Reason::SkillInvalid,
			detail: detail.into(),
		}
	}

	fn limit(detail: impl Into<String>) -> Self {
		Self {
			reason: Reason::SkillLimit,
			detail: detail.into(),
		}
	}
}

/// Skills in directory order; those past the 16th are reported without being read.
pub(super) fn discover(files: &[PluginFile]) -> (Vec<PluginSkill>, Vec<Finding>) {
	let mut definitions: BTreeMap<&str, &PluginFile> = BTreeMap::new();
	for file in files {
		let mut segments = file.path.split('/');
		if let (Some(SKILLS_DIR), Some(directory), Some(SKILL_FILE), None) = (
			segments.next(),
			segments.next(),
			segments.next(),
			segments.next(),
		) {
			definitions.insert(directory, file);
		}
	}
	let mut skills = Vec::new();
	let mut findings = Vec::new();
	for (index, (directory, definition)) in definitions.into_iter().enumerate() {
		let component = Component::Skill(directory.to_owned());
		if index >= MAX_SKILLS {
			findings.push(Finding::skipped(
				component,
				Reason::SkillLimit,
				format!("a plugin carries at most {MAX_SKILLS} Skills"),
			));
			continue;
		}
		match load_skill(directory, definition, files) {
			Ok(skill) => skills.push(skill),
			Err(Refusal { reason, detail }) => {
				findings.push(Finding::skipped(component, reason, detail));
			}
		}
	}
	(skills, findings)
}

fn load_skill(
	directory: &str,
	definition: &PluginFile,
	files: &[PluginFile],
) -> Result<PluginSkill, Refusal> {
	let prefix = format!("{SKILLS_DIR}/{directory}/");
	let mut others: Vec<(&str, &PluginFile)> = files
		.iter()
		.filter(|file| file.path != definition.path)
		.filter_map(|file| Some((file.path.strip_prefix(&prefix)?, file)))
		.collect();
	others.sort_by_key(|(relative, _)| *relative);
	check_limits(definition, &others)?;
	let instructions = String::from_utf8(definition.bytes.clone())
		.map_err(|_| Refusal::invalid("SKILL.md is not valid UTF-8"))?;
	let front = frontmatter(&instructions)?;
	check_rules(directory, &front)?;
	Ok(PluginSkill {
		directory: directory.to_owned(),
		name: front.name,
		description: front.description,
		license: front.license,
		compatibility: front.compatibility,
		instructions,
		files: others
			.into_iter()
			.map(|(relative, file)| skill_file(relative, &file.bytes))
			.collect(),
	})
}

fn check_limits(definition: &PluginFile, others: &[(&str, &PluginFile)]) -> Result<(), Refusal> {
	if definition.bytes.len() > MAX_INSTRUCTIONS_BYTES || definition.bytes.contains(&0) {
		return Err(Refusal::limit(format!(
			"SKILL.md must be text of at most {MAX_INSTRUCTIONS_BYTES} bytes"
		)));
	}
	if others.len() > MAX_OTHER_FILES {
		return Err(Refusal::limit(format!(
			"a Skill has at most {MAX_OTHER_FILES} files besides SKILL.md"
		)));
	}
	if let Some((relative, _)) = others
		.iter()
		.find(|(relative, _)| !valid_skill_file_path(relative))
	{
		return Err(Refusal::limit(format!(
			"{relative} is not a valid Skill file path"
		)));
	}
	let total = others
		.iter()
		.fold(definition.bytes.len(), |sum, (_, file)| {
			sum + file.bytes.len()
		});
	if total > MAX_TOTAL_BYTES {
		return Err(Refusal::limit(format!(
			"a Skill has at most {MAX_TOTAL_BYTES} bytes"
		)));
	}
	Ok(())
}

fn frontmatter(instructions: &str) -> Result<Frontmatter, Refusal> {
	let text = instructions.replace("\r\n", "\n");
	let front = text
		.strip_prefix("---\n")
		.and_then(|rest| rest.split_once("\n---"))
		.map(|(front, _)| front)
		.ok_or_else(|| Refusal::invalid("SKILL.md needs YAML frontmatter"))?;
	if front.len() > MAX_FRONTMATTER_BYTES {
		return Err(Refusal::limit(format!(
			"frontmatter has at most {MAX_FRONTMATTER_BYTES} bytes"
		)));
	}
	serde_saphyr::from_str(front)
		.map_err(|error| Refusal::invalid(format!("invalid frontmatter: {error}")))
}

fn check_rules(directory: &str, front: &Frontmatter) -> Result<(), Refusal> {
	let name = front.name.as_str();
	let valid_name = !name.is_empty()
		&& name.len() <= MAX_NAME_LEN
		&& name
			.chars()
			.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
		&& !name.starts_with('-')
		&& !name.ends_with('-')
		&& !name.contains("--");
	if !valid_name {
		return Err(Refusal::invalid(
			"name must be 1-64 characters of a-z, 0-9 and -, without a leading, trailing or double -",
		));
	}
	if name != directory {
		return Err(Refusal::invalid("name must equal the Skill directory"));
	}
	let description = front.description.chars().count();
	if !(1..=MAX_DESCRIPTION_CHARS).contains(&description) {
		return Err(Refusal::invalid(format!(
			"description must be 1-{MAX_DESCRIPTION_CHARS} characters"
		)));
	}
	let compatibility = front
		.compatibility
		.as_ref()
		.map(|text| text.chars().count());
	if compatibility.is_some_and(|count| !(1..=MAX_COMPATIBILITY_CHARS).contains(&count)) {
		return Err(Refusal::invalid(format!(
			"compatibility must be 1-{MAX_COMPATIBILITY_CHARS} characters"
		)));
	}
	Ok(())
}

/// Text that survives a UTF-8 round trip stays text; the rest is base64.
fn skill_file(relative: &str, bytes: &[u8]) -> SkillFile {
	match std::str::from_utf8(bytes) {
		Ok(text) if !text.contains('\0') => SkillFile {
			path: relative.to_owned(),
			content: text.to_owned(),
			encoding: None,
		},
		_ => SkillFile {
			path: relative.to_owned(),
			content: base64::engine::general_purpose::STANDARD.encode(bytes),
			encoding: Some("base64".to_owned()),
		},
	}
}
