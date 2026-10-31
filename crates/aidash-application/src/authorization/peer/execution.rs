//! Pinned dependency checks are shared by HTTP preflight, admission and worker refresh.
use crate::{Error, Result, ports::authorization::peer::PeerInspectionScope};
use aidash_domain::{
	federation::execution::{Definition, Inspection, admission::InspectInput},
	policy::SubjectKind,
	registry::{EntityRef, Entry, rules::digest},
};
use serde_json::json;
use std::collections::BTreeMap;
async fn definition(
	access: &mut dyn PeerInspectionScope,
	reference: &EntityRef,
	kind: &str,
	action: &str,
	users: &[EntityRef],
	definitions: &mut BTreeMap<(String, String), Definition>,
) -> Result<Entry> {
	let entry = if users.is_empty() {
		access.entry(reference, action).await
	} else {
		access.closure_entry(reference, action, users).await
	}
	.map_err(|error| match error {
		Error::Forbidden => Error::RemoteExecutionRefused(
			aidash_domain::federation::execution::Refusal::CatalogApproval,
		),
		Error::AuthorityRefused(_) => {
			Error::RemoteExecutionRefused(aidash_domain::federation::execution::Refusal::Policy)
		}
		other => other,
	})?;
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
async fn inspect_in(
	access: &mut dyn PeerInspectionScope,
	node: &str,
	input: &InspectInput,
	pinned: Option<&Inspection>,
) -> Result<Inspection> {
	// Receiving external execution is separate from advertising metadata.
	let resource = access.receiver_resource().await?;
	access
		.require(&resource, "federation.execute")
		.await
		.map_err(|error| match error {
			Error::Forbidden => {
				Error::RemoteExecutionRefused(aidash_domain::federation::execution::Refusal::Policy)
			}
			other => other,
		})?;
	// The agent runs on the receiving Node, whose address names its subject.
	let executor = aidash_domain::agent_subject(
		&access.receiver_address(),
		&input.agent.id,
		&input.agent.version,
	);
	let valid_executor = access
		.bundle()
		.subjects
		.get(&executor)
		.is_some_and(|subject| subject.kind == SubjectKind::Agent);
	let invalid_kind = access
		.bundle()
		.subjects
		.get(&executor)
		.is_some_and(|subject| subject.kind != SubjectKind::Agent);
	let generation = access.generation(node, input).await?;
	access.push_subject(executor);
	if invalid_kind {
		access.invalid_executor(&resource).await?;
		return Err(Error::RemoteExecutionRefused(
			aidash_domain::federation::execution::Refusal::InvalidExecutor,
		));
	}
	access
		.require(&resource, "federation.execute")
		.await
		.map_err(|error| match error {
			Error::Forbidden => {
				Error::RemoteExecutionRefused(aidash_domain::federation::execution::Refusal::Policy)
			}
			other => other,
		})?;
	// Only this Node's Catalog Approval creates the executor subject.
	if !valid_executor {
		return Err(Error::RemoteExecutionRefused(
			aidash_domain::federation::execution::Refusal::CatalogApproval,
		));
	}
	let mut definitions = BTreeMap::new();
	let entry = definition(
		access,
		&input.agent,
		"agent",
		"agent.execute",
		&[],
		&mut definitions,
	)
	.await?;
	if pinned.is_none() && !access.active_installation(&entry).await? {
		return Err(Error::Forbidden);
	}
	access.pinned_installation(&entry).await?;
	if !input.requirements.matches(&entry) {
		return Err(Error::Invalid(
			"executor does not satisfy task requirements".into(),
		));
	}
	let snapshot = if let Some(pinned) = pinned {
		pinned.binding_snapshot.validate()?;
		pinned.binding_snapshot.clone()
	} else {
		access.bindings(&entry).await?
	};
	if pinned.is_none() {
		// A new inspection starts a Run here: the executing Node requires the
		// Agent's compaction Decider to hold its Catalog Approval (#540).
		for binding in crate::authorization::execution::compaction::bound(&snapshot) {
			let current = access
				.entry(&binding.identity.local(), aidash_domain::decision::INVOKE)
				.await;
			crate::authorization::execution::compaction::require(binding, current)?;
		}
	}
	if !snapshot.remote
		|| snapshot.agent.registry_deployment != access.node_id()
		|| snapshot.agent.local() != input.agent
	{
		return Err(Error::Forbidden);
	}
	// A delegated agent's own dependencies are restricted to that agent.
	let delegated = snapshot.delegated_users()?;
	for pinned in &snapshot.definitions {
		if pinned.identity.registry_deployment != access.node_id() {
			return Err(Error::Forbidden);
		}
		if aidash_domain::registry::bindings::defaults::is_compaction_decider(&pinned.definition) {
			access
				.require(&access.entry_resource(&pinned.definition), "registry.read")
				.await?;
			// Retain the exact immutable definition; compaction's own dispatch
			// checks current Catalog Approval before sending any state.
			definitions.insert(
				(pinned.identity.id.clone(), pinned.identity.version.clone()),
				Definition {
					entry: pinned.identity.local(),
					kind: pinned.definition.kind.clone(),
					digest: pinned.digest.clone(),
					metadata: pinned.definition.clone(),
				},
			);
			continue;
		}
		let action = match pinned.definition.kind.as_str() {
			"agent" => "agent.execute",
			"model" => "model.infer",
			"cluster" => "cluster.execute",
			"skill" => "skill.use",
			"tool"
				if snapshot.bindings.iter().any(|binding| {
					binding.identity == pinned.identity && binding.excluded_reason.is_none()
				}) =>
			{
				"tool.invoke"
			}
			_ => "registry.read",
		};
		let users = delegated
			.get(&pinned.identity)
			.map(|users| users.iter().map(|user| user.local()).collect::<Vec<_>>())
			.unwrap_or_default();
		let current = definition(
			access,
			&pinned.identity.local(),
			&pinned.definition.kind,
			action,
			&users,
			&mut definitions,
		)
		.await?;
		access.pinned_installation(&current).await?;
		if digest(&serde_json::to_value(current)?) != pinned.digest {
			return Err(Error::Forbidden);
		}
	}
	if let Some(decider) = &input.decider {
		definition(
			access,
			decider,
			"decider",
			"decision.invoke",
			&[],
			&mut definitions,
		)
		.await?;
	}
	let source_subject = access.context()["source_subject"].clone();
	Ok(Inspection {
		binding_snapshot: snapshot,
		generation,
		lineage: access.lineage().await?,
		node_id: access.receiver_address(),
		authority_digest: digest(
			&json!({"source_node":node,"source_subject":source_subject,"tenant":access.identity().tenant,"credential_id":access.identity().credential_id,"subjects":access.subjects()}),
		),
		agent: entry,
		definitions: definitions.into_values().collect(),
		semantic_memory: aidash_domain::semantic::remote::VERSION,
		decider: input.decider.clone(),
	})
}

pub async fn inspect(
	access: &mut dyn PeerInspectionScope,
	node: &str,
	input: &InspectInput,
) -> Result<Inspection> {
	inspect_in(access, node, input, None).await
}
pub async fn inspect_retained(
	access: &mut dyn PeerInspectionScope,
	node: &str,
	input: &InspectInput,
	pinned: &Inspection,
) -> Result<Inspection> {
	inspect_in(access, node, input, Some(pinned)).await
}

#[cfg(test)]
mod tests;
