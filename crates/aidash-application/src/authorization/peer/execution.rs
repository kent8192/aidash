//! Pinned dependency checks are shared by HTTP preflight, admission and worker refresh.
use crate::{Error, Result, ports::authorization::peer::PeerInspectionScope};
use aidash_domain::{
	federation::execution::{Definition, Inspection, admission::InspectInput},
	policy::SubjectKind,
	qualified_agent,
	registry::{AgentConfig, EntityRef, Entry, rules::digest},
};
use serde_json::json;
use std::collections::BTreeMap;
async fn definition(
	access: &mut dyn PeerInspectionScope,
	reference: &EntityRef,
	kind: &str,
	action: &str,
	definitions: &mut BTreeMap<(String, String), Definition>,
) -> Result<Entry> {
	let entry = access.entry(reference, action).await?;
	access
		.require(&access.entry_resource(&entry), "registry.read")
		.await?;
	if entry.kind != kind {
		return Err(Error::Invalid(
			"executor dependency has the wrong kind".into(),
		));
	}
	definitions.insert(
		(entry.id.clone(), entry.version.clone()),
		Definition {
			entry: reference.clone(),
			kind: entry.kind.clone(),
			digest: digest(&serde_json::to_value(&entry)?),
			metadata: entry.clone(),
		},
	);
	Ok(entry)
}
pub async fn inspect(
	access: &mut dyn PeerInspectionScope,
	node: &str,
	input: &InspectInput,
) -> Result<Inspection> {
	// Receiving external execution is separate from advertising metadata.
	let resource = access.resource("node", access.node_id(), json!({}));
	access.require(&resource, "federation.execute").await?;
	let executor = qualified_agent(access.node_id(), &input.agent.id, &input.agent.version);
	if access
		.bundle()
		.subjects
		.get(&executor)
		.is_none_or(|subject| subject.kind != SubjectKind::Agent)
	{
		return Err(Error::Forbidden);
	}
	let generation = access.generation(node, input).await?;
	access.push_subject(executor);
	access.require(&resource, "federation.execute").await?;
	let mut definitions = BTreeMap::new();
	let entry = definition(
		access,
		&input.agent,
		"agent",
		"agent.execute",
		&mut definitions,
	)
	.await?;
	if !access.active_installation(&entry).await? {
		return Err(Error::Forbidden);
	}
	access.pinned_installation(&entry).await?;
	if !input.requirements.matches(&entry) {
		return Err(Error::Invalid(
			"executor does not satisfy task requirements".into(),
		));
	}
	let agent: AgentConfig = serde_json::from_value(entry.config.clone())?;
	definition(
		access,
		&agent.model,
		"model",
		"model.infer",
		&mut definitions,
	)
	.await?;
	for tool in &agent.tools {
		definition(access, tool, "tool", "tool.invoke", &mut definitions).await?;
	}
	for skill in &agent.skills {
		definition(access, skill, "skill", "skill.use", &mut definitions).await?;
	}
	if let Some(cluster) = &agent.cluster {
		definition(
			access,
			cluster,
			"cluster",
			"cluster.execute",
			&mut definitions,
		)
		.await?;
	}
	if let Some(compactor) = &input.compactor {
		definition(
			access,
			compactor,
			"compactor",
			"compaction.invoke",
			&mut definitions,
		)
		.await?;
	}
	Ok(Inspection {
		generation,
		lineage: access.lineage().await?,
		node_id: access.node_id().to_owned(),
		authority_digest: digest(
			&json!({"source_node":node,"source_tenant":input.tenant,"source_subject":input.subject,"tenant":access.identity().tenant,"credential_id":access.identity().credential_id,"subjects":access.subjects()}),
		),
		agent: entry,
		definitions: definitions.into_values().collect(),
		semantic_memory: aidash_domain::semantic::remote::VERSION,
		compactor: input.compactor.clone(),
	})
}

#[cfg(test)]
mod tests;
