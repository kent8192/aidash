//! Definition invariants and immutable configuration rules.
use super::{Entry, SkillFile};
use crate::{Error, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn skill_files(entry: &Entry) -> Result<Vec<SkillFile>> {
	let files: Vec<SkillFile> = serde_json::from_value(
		entry
			.config
			.get("files")
			.cloned()
			.unwrap_or_else(|| json!([])),
	)
	.map_err(|error| Error::Invalid(format!("invalid skill files: {error}")))?;
	Ok(files)
}

pub fn valid_skill_file_path(path: &str) -> bool {
	!path.is_empty()
		&& path.len() <= 240
		&& !path.contains('\\')
		&& !path
			.split('/')
			.any(|part| part.is_empty() || part == "." || part == ".." || part.starts_with('.'))
		&& !path.chars().any(char::is_control)
}

pub fn skill_instructions(entry: &Entry) -> Result<String> {
	let mut instructions = entry.config["instructions"]
		.as_str()
		.ok_or_else(|| Error::Invalid("skill requires instructions".into()))?
		.to_owned();
	let files = skill_files(entry)?;
	if !files.is_empty() {
		instructions.push_str("\n\nRegistered Skill files are available through skill_read. Read a listed path only when needed:\n");
		for file in files {
			if file.encoding.as_deref() == Some("base64") {
				instructions.push_str(&format!("- {} (binary, base64 encoded)\n", file.path));
			} else {
				instructions.push_str(&format!("- {}\n", file.path));
			}
		}
	}
	Ok(instructions)
}

pub fn digest(value: &Value) -> String {
	format!("sha256:{:x}", Sha256::digest(value.to_string().as_bytes()))
}

pub fn overlay_config(target: &mut Value, overrides: &Value) -> Result<()> {
	let object = overrides
		.as_object()
		.ok_or_else(|| Error::Invalid("installation config must be an object".into()))?;
	if object.contains_key("knowledge_digest")
		|| object.contains_key("core_capabilities")
		|| object.contains_key("skill_attachments")
		|| object.contains_key("skill_roots")
		|| object.contains_key("reference_attachments")
	{
		return Err(Error::Invalid(
			"installation config cannot override immutable knowledge or capability settings".into(),
		));
	}
	let target = target
		.as_object_mut()
		.ok_or_else(|| Error::Invalid("entity config must be an object".into()))?;
	for (key, value) in object {
		target.insert(key.clone(), value.clone());
	}
	Ok(())
}

pub fn validate_override_keys(kind: &str, overrides: &Value) -> Result<()> {
	let object = overrides
		.as_object()
		.ok_or_else(|| Error::Invalid("installation config must be an object".into()))?;
	let allowed: &[&str] = match kind {
		"agent" => &[
			"model",
			"instructions",
			"bindings",
			"remove_default",
			"cluster",
			"max_steps",
			"tool_parallelism",
		],
		"model" => &[
			"provider",
			"model_id",
			"endpoint",
			"credential_env",
			"provider_credential",
			"reasoning_effort",
			"context_window",
			"modalities",
			"cost",
			"request_timeout_secs",
		],
		"cluster" => &["coordinator"],
		"bundle" | "memory" | "source" => &[],
		"skill" => &["instructions"],
		// ToolConfig uses a tagged, deny_unknown_fields contract. The merged
		// effective configuration is validated by Registry before any write.
		"tool" => &["transport", "narrow"],
		_ => return Err(Error::Invalid("unsupported installation kind".into())),
	};
	if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
		return Err(Error::Invalid(format!(
			"{kind} installation cannot override {key}"
		)));
	}
	Ok(())
}

pub fn validate_metadata(e: &Entry, local: bool) -> Result<()> {
	if local && e.installation.is_some() {
		return Err(Error::Invalid(
			"installed definitions require the Marketplace revision API".into(),
		));
	}
	let schema = json!({"type":"object","required":["id","version","kind","name","description","capabilities","tags","languages","schema","config"],
        "properties":{
            "id":{"type":"string","pattern":"^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$"},
            "version":{"type":"string"}, "kind":{"enum":["agent","model","tool","skill","cluster","node","compactor","decider","embedding","bundle","memory","source","reranker","tokenizer"]},
            "name":{"type":"object","minProperties":1,"additionalProperties":{"type":"string","minLength":1}},
            "description":{"type":"object","minProperties":1,"additionalProperties":{"type":"string"}},
            "capabilities":{"type":"array","items":{"type":"string"},"uniqueItems":true},
            "tags":{"type":"array","items":{"type":"string"},"uniqueItems":true},
            "languages":{"type":"array","items":{"type":"string","pattern":"^[a-zA-Z]{2,8}(-[a-zA-Z0-9]{1,8})*$"},"uniqueItems":true},
            "schema":{"type":"object"},"config":{"type":"object"}}});
	let validator =
		jsonschema::validator_for(&schema).map_err(|e| Error::Invalid(e.to_string()))?;
	validator
		.validate(&serde_json::to_value(e)?)
		.map_err(|e| Error::Invalid(e.to_string()))?;
	semver::Version::parse(&e.version)
		.map_err(|_| Error::Invalid("version must be semantic versioning".into()))?;
	// Reserve the largest valid node ID (108 bytes), `/agents/`, and `@`.
	// Every accepted agent must fit the 256-byte authorization subject limit.
	if e.kind == "agent" && e.id.len() + e.version.len() > 139 {
		return Err(Error::Invalid(
			"qualified agent identity exceeds 256 bytes".into(),
		));
	}
	for locale in e.name.keys().chain(e.description.keys()) {
		if locale.is_empty()
			|| !locale.split('-').all(|p| {
				!p.is_empty() && p.len() <= 8 && p.chars().all(|c| c.is_ascii_alphanumeric())
			}) {
			return Err(Error::Invalid(
				"metadata locales must be BCP 47 language tags".into(),
			));
		}
	}
	// Remote schema resolution is disabled: metadata must be self contained.
	jsonschema::validator_for(&e.schema)
		.map_err(|e| Error::Invalid(format!("invalid entity schema: {e}")))?;
	Ok(())
}

pub fn validate_skill(e: &Entry) -> Result<()> {
	let instructions = e
		.config
		.get("instructions")
		.and_then(Value::as_str)
		.ok_or_else(|| Error::Invalid("skill requires instructions".into()))?;
	if instructions.trim().is_empty() {
		return Err(Error::Invalid("skill requires instructions".into()));
	}
	if instructions.len() > 65_536 || instructions.contains('\0') {
		return Err(Error::Invalid(
			"skill instructions exceed 64 KiB or contain NUL".into(),
		));
	}
	let files = skill_files(e)?;
	if files.len() > 64 {
		return Err(Error::Invalid("skill files exceed 64 files".into()));
	}
	let mut total_bytes = 0;
	let mut paths = std::collections::HashSet::new();
	for file in files {
		total_bytes += file.byte_len()?;
		if total_bytes > 256_000 {
			return Err(Error::Invalid("skill files exceed 256 KB".into()));
		}
		if !valid_skill_file_path(&file.path)
			|| file.content.contains('\0')
			|| !paths.insert(file.path)
		{
			return Err(Error::Invalid(
				"skill file paths must be unique relative paths".into(),
			));
		}
	}

	Ok(())
}

pub fn validate_path(path: &str) -> Result<()> {
	if path.is_empty()
		|| path.len() > 1024
		|| path.contains('\\')
		|| path.chars().any(char::is_control)
		|| path
			.split('/')
			.any(|p| p.is_empty() || matches!(p, "." | ".."))
	{
		return Err(Error::Invalid(
			"INVALID_PATH: expected a relative regular-file path".into(),
		));
	}
	Ok(())
}
