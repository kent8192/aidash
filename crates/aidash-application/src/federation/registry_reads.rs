//! A retained remote definition requires exact metadata, both local policies and live peer proof.
use crate::{
	Error, Result,
	ports::federation::registry_reads::{
		RegistryReadScope, RegistryTransport, RegistryVerificationScope, Verification,
	},
};
use aidash_domain::{
	federation::{
		dependencies::{LIMIT, Reference as Dependency},
		registry_reads::Reference,
	},
	qualified_agent,
	registry::{EntityRef, Entry, rules::digest},
};
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;
pub async fn reads_visible(
	scope: &mut dyn RegistryReadScope,
	transport: &dyn RegistryTransport,
	run: Uuid,
) -> Result<bool> {
	if let Some(visible) = scope.cached(run) {
		return Ok(visible);
	}
	let visible = check(scope, transport, run).await?;
	scope.remember(run, visible);
	Ok(visible)
}
async fn check(
	scope: &mut dyn RegistryReadScope,
	transport: &dyn RegistryTransport,
	run: Uuid,
) -> Result<bool> {
	let mut nodes: BTreeMap<String, Vec<Reference>> = BTreeMap::new();
	for (node, id, version, hash, metadata) in scope.dependencies(run).await? {
		let entry: Entry = serde_json::from_value(metadata.clone())?;
		if entry.id != id
			|| entry.version != version
			|| entry.kind != "agent"
			|| digest(&metadata) != hash
		{
			return Ok(false);
		}
		let mut resource = scope.catalog_resource(&entry);
		resource.id = qualified_agent(&node, &id, &version);
		resource.attributes["remote_node"] = json!(node);
		if !scope.decide(&resource, "registry.read").await?
			|| !scope.decide(&resource, "agent.execute").await?
		{
			return Ok(false);
		}
		nodes.entry(node).or_default().push(Reference {
			entry: EntityRef { id, version },
			digest: hash,
		});
	}
	if let Some(pending) = scope.frontier() {
		for (node, references) in nodes {
			pending.extend(
				references
					.into_iter()
					.map(|reference| Dependency::Registry {
						node_id: node.clone(),
						id: reference.entry.id,
						version: reference.entry.version,
						digest: reference.digest,
					}),
			);
		}
		return Ok(pending.len() <= LIMIT);
	}
	for (node, references) in nodes {
		if scope.unavailable(&node) {
			return Ok(false);
		}
		let resource = scope.resource("node", &node, json!({"remote_node":node}));
		if !scope.decide(&resource, "federation.discover").await? {
			return Ok(false);
		}
		let Some(peer) = scope.peer(&node).await? else {
			return Ok(false);
		};
		if peer.protocol_version != scope.protocol() {
			return Ok(false);
		}
		for batch in references.chunks(128) {
			let (tenant, subject) = scope.identity();
			match transport.verify(&peer, tenant, subject, batch).await {
				Verification::Verified => {}
				Verification::Rejected => return Ok(false),
				Verification::Unavailable => {
					scope.mark_unavailable(&node);
					return Ok(false);
				}
			}
		}
	}
	Ok(true)
}
pub async fn verify(
	scope: &mut dyn RegistryVerificationScope,
	references: &[Reference],
) -> Result<bool> {
	aidash_domain::federation::registry_reads::validate(references)?;
	let node = scope.resource("node", scope.node(), json!({}));
	scope.require(&node, "federation.discover").await?;
	for reference in references {
		let entry = match scope.entry(&reference.entry).await {
			Ok(entry) => entry,
			Err(Error::Forbidden | Error::NotFound(_)) => return Ok(false),
			Err(error) => return Err(error),
		};
		if entry.kind != "agent"
			|| digest(&serde_json::to_value(&entry)?) != reference.digest
			|| !scope
				.decide(&scope.catalog_resource(&entry), "agent.execute")
				.await?
		{
			return Ok(false);
		}
	}
	Ok(true)
}
#[cfg(test)]
mod tests;
