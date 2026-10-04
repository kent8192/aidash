//! Shared dependency authorization and bounded traversal for HTTP and workers.
use crate::{
	Error, Result,
	ports::federation::dependencies::{DependencyScope, DependencyTransport},
};
use aidash_domain::{
	federation::dependencies::{Checked, Input, LIMIT, Reference},
	registry::{EntityRef, rules::digest},
};
use serde_json::json;
use std::{collections::BTreeSet, time::Duration};

/// Apply this before resolving a peer mapping or acquiring its authority lease.
pub fn require_local_target(reference: &Reference, node: &str) -> Result<()> {
	if reference.node() != node {
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub async fn check_local(scope: &mut dyn DependencyScope, reference: &Reference) -> Result<bool> {
	if reference.node() != scope.node() {
		return Ok(false);
	}
	let result = match reference {
		Reference::Admission {
			home_node,
			grant_id,
			admission_id,
			..
		} => {
			let Some(run) = scope.admission(*admission_id, home_node).await? else {
				return Ok(false);
			};
			if scope.bound_grant(*admission_id).await? != Some(*grant_id) {
				return Ok(false);
			}
			scope.run_visible(&run).await
		}
		Reference::Grant {
			execution_node,
			grant_id,
			admission_id,
			..
		} => {
			scope
				.grant_visible(execution_node, *grant_id, *admission_id)
				.await
		}
		Reference::Registry {
			id,
			version,
			digest: expected,
			..
		} => {
			let node = scope.resource("node", scope.node(), json!({}));
			if !scope.decide(&node, "federation.discover").await? {
				return Ok(false);
			}
			let reference = EntityRef {
				id: id.clone(),
				version: version.clone(),
			};
			let entry = scope.catalog_entry(&reference, "registry.read").await?;
			Ok(entry.kind == "agent"
				&& digest(&serde_json::to_value(&entry)?) == *expected
				&& scope
					.decide(&scope.catalog_resource(&entry), "agent.execute")
					.await?)
		}
	};
	match result {
		Err(
			Error::Forbidden | Error::Unauthorized | Error::NotFound(_) | Error::RemoteSemantic(_),
		) => Ok(false),
		other => other,
	}
}

async fn local_frontier(scope: &mut dyn DependencyScope, reference: &Reference) -> Result<Checked> {
	scope.start_frontier();
	let visible = check_local(scope, reference).await?;
	Ok(Checked {
		visible,
		pending: scope.take_frontier(),
	})
}

/// A peer evaluates its own graph and returns qualified edges. Only the
/// initiating reader traverses those edges under an explicit mapping per node.
pub async fn verify_peer(
	scope: &mut dyn DependencyScope,
	reference: &Reference,
) -> Result<Checked> {
	Ok(local_frontier(scope, reference).await?.disclosed())
}

pub async fn verify_all(
	scope: &mut dyn DependencyScope,
	transport: &dyn DependencyTransport,
	protocol: &str,
	mut pending: Vec<Reference>,
) -> Result<bool> {
	let mut visited = BTreeSet::new();
	let deadline = scope.now() + Duration::from_secs(20);
	while let Some(reference) = pending.pop() {
		let remaining = deadline.saturating_duration_since(scope.now());
		if remaining.is_zero() {
			return Ok(false);
		}
		let key = digest(&serde_json::to_value(&reference)?);
		if !visited.insert(key) {
			continue;
		}
		if visited.len() > LIMIT || pending.len() > LIMIT {
			return Ok(false);
		}
		let checked = if reference.node() == scope.node() {
			local_frontier(scope, &reference).await?
		} else {
			let node = reference.node();
			let resource = scope.resource("node", node, json!({"remote_node":node}));
			if !scope.decide(&resource, "federation.discover").await? {
				return Ok(false);
			}
			let Some(peer) = scope.peer(node).await? else {
				return Ok(false);
			};
			if peer.protocol_version != protocol {
				return Ok(false);
			}
			let (tenant, subject) = scope.identity();
			let input = Input {
				tenant: tenant.into(),
				subject: subject.into(),
				reference,
			};
			let Some(checked) = transport.check(&peer, &input, remaining).await else {
				return Ok(false);
			};
			checked
		};
		if !checked.visible || checked.pending.len() > LIMIT {
			return Ok(false);
		}
		pending.extend(checked.pending);
	}
	Ok(true)
}

#[cfg(test)]
mod tests;
