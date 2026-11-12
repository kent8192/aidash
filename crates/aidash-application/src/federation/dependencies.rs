//! Shared dependency authorization and bounded traversal for HTTP and workers.
use crate::{
	Error, Result,
	ports::federation::dependencies::{Check, DependencyScope, DependencyTransport},
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
			// `registry.read` alone: a Catalog Approval decides use, not who may read (#769).
			let entry = scope.readable_entry(&reference).await?;
			Ok(entry.kind == "agent" && digest(&serde_json::to_value(&entry)?) == *expected)
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

/// Walk the references. `Ok(false)` covers a refusal and a Node that did not
/// answer alike, so list consumers keep hiding the row; only the second marks
/// the Node unavailable, so the reader says so and nothing is remembered as a
/// denial (#751). A Node marked once is not asked again within this authority.
pub async fn verify_all(
	scope: &mut dyn DependencyScope,
	transport: &dyn DependencyTransport,
	mut pending: Vec<Reference>,
) -> Result<bool> {
	let mut visited = BTreeSet::new();
	let deadline = scope.now() + Duration::from_secs(20);
	// The last other Node this walk asked, which a slow answer may have used up
	// the time for.
	let mut last_asked: Option<String> = None;
	let mut silent: Option<String> = None;
	while let Some(reference) = pending.pop() {
		let remaining = deadline.saturating_duration_since(scope.now());
		if remaining.is_zero() {
			// The time ran out before this reference was checked. Name a Node
			// that may well have answered, and only to a reader allowed to
			// discover it; a reference to a Node with no route is a refusal.
			let node = if reference.node() != scope.node() {
				let node = reference.node();
				let resource = scope.resource("node", node, json!({"remote_node":node}));
				if !scope.decide(&resource, "federation.discover").await?
					|| scope.route(node).await?.is_none()
				{
					return Ok(false);
				}
				Some(node.to_owned())
			} else {
				if last_asked.is_none() {
					tracing::warn!("the dependency walk ran out of time with no other Node asked");
				}
				last_asked.clone()
			};
			if let Some(node) = node {
				// The deadline is not proof of silence and must not extend the memory.
				scope.skipped(&node);
				if let Some(node) = scope.take_row_silence() {
					silent.get_or_insert(node);
				}
			}
			scope.set_row_silence(silent);
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
			// After the decision: a mark may come from another identity of the
			// same authority, and this reader's own right comes first.
			if scope.unavailable(node) {
				scope.skipped(node);
				if let Some(node) = scope.take_row_silence() {
					silent.get_or_insert(node);
				}
				continue;
			}
			let Some(route) = scope.route(node).await? else {
				return Ok(false);
			};
			last_asked = Some(node.to_owned());
			let input = Input { reference };
			match transport.check(&route, &input, remaining).await {
				Check::Confirmed(checked) => {
					scope.answered(input.reference.node());
					checked
				}
				Check::Refused => {
					scope.answered(input.reference.node());
					return Ok(false);
				}
				Check::NotAsked => return Ok(false),
				Check::Unavailable => {
					scope.mark_unavailable(input.reference.node());
					if let Some(node) = scope.take_row_silence() {
						silent.get_or_insert(node);
					}
					continue;
				}
			}
		};
		if !checked.visible {
			match scope.take_row_silence() {
				Some(node) => {
					silent.get_or_insert(node);
					continue;
				}
				None => return Ok(false),
			}
		}
		if checked.pending.len() > LIMIT {
			return Ok(false);
		}
		pending.extend(checked.pending);
	}
	let visible = silent.is_none();
	scope.set_row_silence(silent);
	Ok(visible)
}

#[cfg(test)]
mod tests;
