//! Receiver-side execution preflight. This describes a currently authorized
//! executor; it is never a bearer capability or permission to admit a run.
use super::super::{access::Access, catalog, policy::SubjectKind};
use crate::{
	Error, Result,
	domain::qualified_agent,
	federation::Federation,
	registry::{AgentConfig, EntityRef, Entry, Search, digest},
};
use axum::{Json, extract::State, http::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InspectInput {
	pub tenant: String,
	pub subject: String,
	pub agent: EntityRef,
	pub requirements: Search,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Definition {
	pub entry: EntityRef,
	pub kind: String,
	pub digest: String,
	pub metadata: Entry,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Inspection {
	pub node_id: String,
	pub authority_digest: String,
	pub agent: Entry,
	pub definitions: Vec<Definition>,
}

async fn definition(
	access: &mut Access,
	reference: &EntityRef,
	kind: &str,
	action: &str,
	definitions: &mut BTreeMap<(String, String), Definition>,
) -> Result<Entry> {
	let entry = catalog::entry(access, reference, action).await?;
	access
		.require(&catalog::resource(access, &entry), "registry.read")
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

pub(crate) async fn inspect(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<InspectInput>,
) -> Result<Json<Inspection>> {
	let node = crate::api::peer_node(&headers)?;
	let mut access = super::access(&f, node, &input.tenant, &input.subject).await?;
	let result = inspect_in(&f, &mut access, node, &input).await.map(Json);
	access.finish(result).await
}

pub(crate) async fn inspect_in(
	f: &Federation,
	access: &mut Access,
	node: &str,
	input: &InspectInput,
) -> Result<Inspection> {
	// Receiving external execution is separate from advertising metadata.
	let resource = access.resource("node", &f.config.node_id, json!({}));
	access.require(&resource, "federation.execute").await?;
	let executor = qualified_agent(&f.config.node_id, &input.agent.id, &input.agent.version);
	if access
		.snapshot
		.bundle
		.subjects
		.get(&executor)
		.is_none_or(|subject| subject.kind != SubjectKind::Agent)
	{
		return Err(Error::Forbidden);
	}
	access.subjects.push(executor);
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
	Ok(Inspection {
		node_id: f.config.node_id.clone(),
		authority_digest: digest(
			&json!({"source_node":node,"source_tenant":input.tenant,"source_subject":input.subject,"tenant":access.identity.tenant,"credential_id":access.identity.credential_id,"subjects":access.subjects}),
		),
		agent: entry,
		definitions: definitions.into_values().collect(),
	})
}
