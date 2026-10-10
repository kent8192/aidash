//! Deferred exposure tools: discovery, Load/Unload and Skill asset reads over the
//! Run's Discoverable capabilities. They never mutate the Run; Load/Unload
//! return an `exposure_update` that the Executor stages for the next request.
use super::{ToolContext, required};
use crate::{Error, Result};
use aidash_domain::{
	Run,
	exposure::{
		self, Capability, CapabilityIdentity, CapabilityKind, DeferredBudgets, DirectSkill,
	},
	provider::ToolSpec,
	registry::{AgentConfig, rules},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The exposure budgets and Discoverable capabilities of a `deferred@1` Run.
pub fn catalog(
	run: &Run,
	specifications: &BTreeMap<String, ToolSpec>,
	direct: &[DirectSkill],
) -> Result<(DeferredBudgets, Vec<Capability>)> {
	let snapshot = run
		.context
		.binding_snapshot
		.as_ref()
		.ok_or_else(|| Error::Invalid("Run has no admitted Binding snapshot".into()))?;
	let budgets = *AgentConfig::from_snapshot(snapshot)?
		.exposure_policy()
		.budgets()
		.ok_or_else(|| {
			Error::Invalid("capability exposure requires the deferred@1 Exposure policy".into())
		})?;
	Ok((
		budgets,
		exposure::catalog(snapshot, specifications, direct)?,
	))
}

pub(super) async fn invoke(name: &str, ctx: &ToolContext<'_>, input: Value) -> Result<Value> {
	let direct = ctx.operations.direct_skills().await?;
	if name == "skill_asset_read" {
		// Tool specifications never affect Skill aliases.
		let (_, catalog) = catalog(ctx.run, &BTreeMap::new(), &direct)?;
		return asset(ctx, &catalog, &input).await;
	}
	let specifications = ctx.operations.binding_specifications().await?;
	let (budgets, catalog) = catalog(ctx.run, &specifications, &direct)?;
	// Earlier Load/Unload results of this response are staged, not exposed.
	let state = &ctx.run.context.exposure.effective();
	Ok(match name {
		"capability_search" => exposure::search(
			&catalog,
			state,
			input["query"].as_str().unwrap_or_default(),
			input["cursor"].as_str(),
		)?,
		"capability_describe" => {
			exposure::describe(&catalog, &budgets, required(&input, "alias")?)?
		}
		"capability_load" => exposure::load(
			&budgets,
			&catalog,
			state,
			required(&input, "alias")?,
			required(&input, "digest")?,
			ctx.run.step,
		)?,
		"capability_unload" => exposure::unload(&catalog, state, required(&input, "alias")?)?,
		_ => return Err(Error::Invalid("unknown tool".into())),
	})
}

/// Read one packaged file of a Registry or direct Skill, keyed by its
/// capability alias and digest. Binary files return metadata only.
async fn asset(ctx: &ToolContext<'_>, catalog: &[Capability], input: &Value) -> Result<Value> {
	let alias = required(input, "alias")?;
	let digest = required(input, "digest")?;
	let path = required(input, "path")?;
	let capability = catalog
		.iter()
		.find(|capability| capability.alias == alias && capability.kind == CapabilityKind::Skill)
		.ok_or_else(|| Error::Invalid("UNKNOWN_CAPABILITY".into()))?;
	if capability.digest != digest {
		return Err(Error::Invalid("CAPABILITY_CHANGED".into()));
	}
	rules::validate_path(path)?;
	let metadata = capability.detail["files"]
		.as_array()
		.and_then(|files| files.iter().find(|file| file["path"] == path))
		.cloned()
		.ok_or_else(|| Error::Invalid("SKILL_FILE_UNAVAILABLE".into()))?;
	// A declared base64 asset is binary whatever its decoded bytes are.
	let bytes = match &capability.identity {
		CapabilityIdentity::Registry(reference) => {
			let binding = ctx
				.run
				.context
				.binding_snapshot
				.as_ref()
				.and_then(|snapshot| {
					snapshot
						.bindings
						.iter()
						.find(|binding| &binding.identity == reference)
				})
				.ok_or_else(|| Error::Invalid("UNKNOWN_CAPABILITY".into()))?;
			let file = rules::skill_files(&binding.definition)?
				.into_iter()
				.find(|file| file.path == path)
				.ok_or_else(|| Error::Invalid("SKILL_FILE_UNAVAILABLE".into()))?;
			(file.encoding.as_deref() != Some("base64")).then(|| file.content.into_bytes())
		}
		CapabilityIdentity::DirectSkill { skill_id, .. } => {
			ctx.operations
				.direct_skill_file(*skill_id, digest, path)
				.await?
		}
	};
	let Some(text) = bytes
		.as_deref()
		.and_then(|bytes| std::str::from_utf8(bytes).ok())
	else {
		return Ok(json!({
			"alias": alias,
			"path": path,
			"metadata": metadata,
			"encoding": "binary",
			"truncated": false,
		}));
	};
	let byte_limit = ctx.operations.skill_read_bytes()?;
	let offset = input["offset"].as_u64().unwrap_or(0) as usize;
	let max_chars = input["max_chars"]
		.as_u64()
		.map_or(byte_limit, |value| value as usize);
	let (content, next) =
		crate::capabilities::skills::text_chunk(text, offset, max_chars, byte_limit)?;
	Ok(json!({
		"alias": alias,
		"path": path,
		"digest": metadata["digest"],
		"offset": offset,
		"content": content,
		"next_offset": next,
		"truncated": next.is_some(),
	}))
}

#[cfg(test)]
mod tests;
