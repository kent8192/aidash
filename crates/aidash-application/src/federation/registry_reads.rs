//! A retained remote definition requires exact metadata, local `registry.read` and, unless the
//! Node is the reader's own Tenant's, live peer proof. A Catalog Approval is not consulted.
//! An Agent of the reader's own Tenant is that Tenant's definition, so policy names it as the
//! local one; only another Tenant's Agent carries a Node-qualified id (#856).
use crate::{
	Error, Result,
	ports::federation::registry_reads::{
		RegistryReadScope, RegistryTransport, RegistryVerificationScope, RemoteReads, Verification,
	},
};
use aidash_domain::{
	federation::{
		dependencies::{LIMIT, Reference as Dependency},
		registry_reads::Reference,
	},
	policy::Resource,
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
	Ok(reads(scope, transport, run).await? == RemoteReads::Visible)
}
/// What `run`'s reads of Agents found on other Nodes are to this reader, with the first cause.
pub async fn reads(
	scope: &mut dyn RegistryReadScope,
	transport: &dyn RegistryTransport,
	run: Uuid,
) -> Result<RemoteReads> {
	if let Some(reads) = scope.cached(run) {
		return Ok(reads);
	}
	let reads = check(scope, transport, run).await?;
	scope.remember(run, reads.clone());
	Ok(reads)
}
async fn check(
	scope: &mut dyn RegistryReadScope,
	transport: &dyn RegistryTransport,
	run: Uuid,
) -> Result<RemoteReads> {
	let mut nodes: BTreeMap<String, Vec<Reference>> = BTreeMap::new();
	for (node, id, version, hash, metadata, same_tenant) in scope.dependencies(run).await? {
		let entry: Entry = serde_json::from_value(metadata.clone())?;
		if entry.id != id
			|| entry.version != version
			|| entry.kind != "agent"
			|| digest(&metadata) != hash
		{
			return Ok(RemoteReads::Unconfirmed);
		}
		let resource =
			remote_agent_resource(scope.catalog_resource(&entry), &node, &version, same_tenant);
		if same_tenant {
			// Policy decides within a Tenant: deleting the Node, or its not answering,
			// hides nothing, so there is no route, frontier or transport to consult.
			if !scope.decide(&resource, "registry.read").await? {
				return Ok(RemoteReads::Definition(EntityRef { id, version }));
			}
			let node = scope.resource("node", &node, json!({"remote_node":node}));
			if !scope.decide(&node, "federation.discover").await? {
				return Ok(RemoteReads::Unconfirmed);
			}
			continue;
		}
		if !scope.decide(&resource, "registry.read").await? {
			return Ok(RemoteReads::Unconfirmed);
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
		return Ok(if pending.len() <= LIMIT {
			RemoteReads::Visible
		} else {
			RemoteReads::Unconfirmed
		});
	}
	let mut silent = None;
	'nodes: for (node, references) in nodes {
		let resource = scope.resource("node", &node, json!({"remote_node":node}));
		if !scope.decide(&resource, "federation.discover").await? {
			return Ok(RemoteReads::Unconfirmed);
		}
		// After the decision: a mark may come from another identity of the
		// same authority, and this reader's own right comes first.
		if scope.unavailable(&node) {
			scope.skipped(&node);
			if let Some(node) = scope.take_row_silence() {
				silent.get_or_insert(node);
			}
			continue;
		}
		let Some(route) = scope.route(&node).await? else {
			return Ok(RemoteReads::Unconfirmed);
		};
		for batch in references.chunks(128) {
			match transport.verify(&route, batch).await {
				Verification::Verified => scope.answered(&node),
				Verification::Rejected => {
					scope.answered(&node);
					return Ok(RemoteReads::Unconfirmed);
				}
				Verification::NotAsked => return Ok(RemoteReads::Unconfirmed),
				Verification::Unavailable => {
					scope.mark_unavailable(&node);
					if let Some(node) = scope.take_row_silence() {
						silent.get_or_insert(node);
					}
					continue 'nodes;
				}
			}
		}
	}
	if let Some(node) = silent {
		scope.set_row_silence(Some(node));
		Ok(RemoteReads::Unconfirmed)
	} else {
		Ok(RemoteReads::Visible)
	}
}
/// The policy resource of an Agent found on another Node: the Tenant's own definition keeps its
/// id, and another Tenant's is named by its Node. Either carries the trusted `remote_node`.
pub fn remote_agent_resource(
	mut resource: Resource,
	node: &str,
	version: &str,
	same_tenant: bool,
) -> Resource {
	if !same_tenant {
		resource.id = aidash_domain::agent_subject(node, &resource.id, version);
	}
	resource.attributes["remote_node"] = json!(node);
	resource
}
pub async fn verify(
	scope: &mut dyn RegistryVerificationScope,
	references: &[Reference],
) -> Result<bool> {
	aidash_domain::federation::registry_reads::validate(references)?;
	let node = scope.node_resource().await?;
	scope.require(&node, "federation.discover").await?;
	for reference in references {
		let entry = match scope.entry(&reference.entry).await {
			Ok(entry) => entry,
			Err(Error::Forbidden | Error::NotFound(_)) => return Ok(false),
			Err(error) => return Err(error),
		};
		if entry.kind != "agent" || digest(&serde_json::to_value(&entry)?) != reference.digest {
			return Ok(false);
		}
	}
	Ok(true)
}
#[cfg(test)]
mod tests;
