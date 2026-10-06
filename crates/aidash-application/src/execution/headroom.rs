//! Corrections share the registered model window with task and pinned Skill context.
use crate::{Result, ports::execution::headroom::Definitions};
use aidash_domain::{
	RunMetadata, context::MIN_CONTEXT_RESERVE, model::ModelConfig, registry::AgentConfig,
};
use serde_json::{Value, json};

pub async fn request(scope: &dyn Definitions, run: &RunMetadata) -> Result<usize> {
	let entry = scope
		.definition(run, &run.agent_id, &run.agent_version)
		.await?;
	let agent: AgentConfig = serde_json::from_value(entry.config.clone())?;
	let mut references = Vec::with_capacity(1 + agent.skills.len() + agent.tools.len());
	references.push(
		scope
			.definition(run, &agent.model.id, &agent.model.version)
			.await?,
	);
	for reference in agent.skills.iter().chain(&agent.tools) {
		references.push(
			scope
				.definition(run, &reference.id, &reference.version)
				.await?,
		);
	}
	let private_context = if agent.knowledge_digest.is_some() {
		json!({"reference_documents":scope.documents(&entry).await?})
	} else {
		Value::Null
	};
	let available = scope
		.validation()
		.agent_prompt_headroom(&agent, &references, &private_context)?
		.saturating_sub(MIN_CONTEXT_RESERVE);
	if !agent.core_capabilities.skills {
		return Ok(available);
	}
	Ok(available.saturating_sub(scope.pinned_headroom(run.id).await?))
}

pub async fn message_limit(scope: &dyn Definitions, run: &RunMetadata) -> Result<usize> {
	// Retain most of the window for task, workspace observation and tool history.
	Ok((request(scope, run).await? / 4).min(16_384))
}

pub async fn media_routes(scope: &dyn Definitions, run: &RunMetadata) -> Result<Vec<Vec<String>>> {
	if run.home_node != scope.node() {
		return Ok(vec![]);
	}
	let entry = scope
		.definition(run, &run.agent_id, &run.agent_version)
		.await?;
	let agent: AgentConfig = serde_json::from_value(entry.config)?;
	let entry = scope
		.definition(run, &agent.model.id, &agent.model.version)
		.await?;
	let model: ModelConfig = serde_json::from_value(entry.config)?;
	Ok(model.current_media_input_routes())
}

#[cfg(test)]
mod tests;
