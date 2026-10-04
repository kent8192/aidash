//! Receiver-side execution preflight. This describes a currently authorized
//! executor; it is never a bearer capability or permission to admit a run.
use super::super::{access::Access, catalog, policy::SubjectKind};
use crate::{
	Error, Result,
	domain::qualified_agent,
	federation::Federation,
	registry::{AgentConfig, EntityRef, Entry, digest},
};
use reinhardt::injectable;

use serde_json::json;
use std::collections::BTreeMap;

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
	let generation =
		crate::generation::foreign::inspect(access, node, input.task_id, &input.agent).await?;
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
	if !crate::marketplace::active(access, &entry).await? {
		return Err(Error::Forbidden);
	}
	crate::marketplace::check_pinned(access, &entry).await?;
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
		lineage: crate::generation::remote::lineage(access, &f.config.node_id).await?,
		node_id: f.config.node_id.clone(),
		authority_digest: digest(
			&json!({"source_node":node,"source_tenant":input.tenant,"source_subject":input.subject,"tenant":access.identity.tenant,"credential_id":access.identity.credential_id,"subjects":access.subjects}),
		),
		agent: entry,
		definitions: definitions.into_values().collect(),
		semantic_memory: crate::semantic::remote::VERSION,
		compactor: input.compactor.clone(),
	})
}

pub(crate) use crate::apps::identity::serializers::peer_execution::{
	Definition, InspectInput, Inspection,
};

use http::HeaderMap;

#[derive(Clone)]
pub struct PeerExecution {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_execution(#[inject] runtime: Federation) -> PeerExecution {
	PeerExecution { runtime }
}

impl PeerExecution {
	pub(crate) async fn inspect(
		&self,
		headers: HeaderMap,
		input: InspectInput,
	) -> Result<Inspection> {
		let f = self.runtime.clone();
		let node = crate::apps::identity::services::http_auth::peer_node(&headers)?;
		let mut access = super::access(&f, node, &input.tenant, &input.subject).await?;
		let result = inspect_in(&f, &mut access, node, &input).await;
		access.finish(result).await
	}
}
