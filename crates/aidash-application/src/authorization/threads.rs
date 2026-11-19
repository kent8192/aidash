//! Ownership, disclosure and interaction share the caller's exact authority snapshot.
use crate::{
	Error, Result,
	ports::authorization::threads::{
		RunInteractionScope, ThreadAuthorityScope, ThreadOwner, ThreadResourceScope,
	},
};
use aidash_domain::{
	Run,
	policy::{Evaluation, Resource},
};
use serde_json::json;
use uuid::Uuid;
/// An inherited lease reaches its own Thread and, for reading only, the
/// Subnode Threads its Run created (named by the lease context).
pub fn thread_reachable(context: &serde_json::Value, id: Uuid) -> bool {
	let id = json!(id.to_string());
	context["thread_id"] == id || subnode_read_only(context, &id)
}
/// Whether `id` is reachable only through the creator's read exception.
pub fn subnode_read_thread(context: &serde_json::Value, id: Uuid) -> bool {
	let id = json!(id.to_string());
	context["thread_id"] != id && subnode_read_only(context, &id)
}
fn subnode_read_only(context: &serde_json::Value, id: &serde_json::Value) -> bool {
	context["subnode_read_threads"]
		.as_array()
		.is_some_and(|threads| threads.contains(id))
}
/// The trusted attributes of the `thread` resource: `owner`, `thread_id` and,
/// for a Duty's own Thread, `duty_id` (#1008, ADR 0140). Every place that
/// evaluates the `thread` resource builds them here, so a policy condition on
/// any of them answers the same on a read, in a list and for a Node
/// administrator. `duty_id` is absent, not null, on other Threads, so a
/// condition that `/duty_id` exists names exactly the Duty Threads.
pub fn attributes(id: Uuid, owner: &ThreadOwner) -> serde_json::Value {
	let mut attributes = json!({"owner":owner.owner,"thread_id":id});
	if let Some(duty) = owner.duty_id {
		attributes["duty_id"] = json!(duty);
	}
	attributes
}
pub async fn resource(scope: &mut dyn ThreadResourceScope, id: Uuid) -> Result<Resource> {
	if scope.inherited() && !thread_reachable(scope.context(), id) && !scope.recheck_reaches(id) {
		return Err(Error::Forbidden);
	}
	let lock = !scope.inherited();
	let owner = scope.owner(id, lock).await?.ok_or(Error::Forbidden)?;
	let mut resource = scope.resource("thread", &id.to_string(), attributes(id, &owner));
	// The lease context fills in attributes a resource lacks. A Run working in a
	// Duty's Thread must not lend that Duty to another Thread it reads (a
	// Subnode's), so a Thread without a Duty never carries one.
	if owner.duty_id.is_none()
		&& let Some(attributes) = resource.attributes.as_object_mut()
	{
		attributes.remove("duty_id");
	}
	Ok(resource)
}
pub async fn decision(
	scope: &mut dyn ThreadAuthorityScope,
	id: Uuid,
	action: &str,
	owner: Option<&ThreadOwner>,
) -> Result<bool> {
	let (tenant, subject) = {
		let (tenant, subject) = scope.identity();
		(tenant.to_owned(), subject.to_owned())
	};
	let input = Evaluation {
		subject: subject.clone(),
		action: action.into(),
		resource: Resource {
			tenant,
			kind: "thread".into(),
			id: id.to_string(),
			attributes: owner
				.map(|owner| attributes(id, owner))
				.unwrap_or_else(|| json!({})),
		},
		environment: scope.environment().clone(),
	};
	let mut decision = scope.snapshot().bundle.evaluate(&input);
	decision.revision = scope.snapshot().revision;
	if owner.is_none() {
		decision.allowed = false;
		decision.reason = "resource_unavailable".into();
	}
	let mut allowed = decision.allowed;
	let mut decisions = vec![(input, decision)];
	// A Node administrator reads every Thread of their Node (#1008, ADR 0140),
	// unless a policy denies them this Thread by name. It is a person's view:
	// a Run's inherited authority stays confined to its own Thread.
	if !allowed
		&& !scope.inherited()
		&& owner.is_some()
		&& matches!(action, "thread.read" | "thread.events")
		&& decisions[0].1.reason != "explicit_deny"
	{
		let node = scope.thread_node(id).await?;
		let input = Evaluation {
			subject,
			action: aidash_domain::identity::tenant_administration::NODE_ADMINISTER.into(),
			resource: scope.resource("node", &node, json!({})),
			environment: scope.environment().clone(),
		};
		let mut administers = scope.snapshot().bundle.evaluate(&input);
		administers.revision = scope.snapshot().revision;
		allowed = administers.allowed;
		decisions.push((input, administers));
	}
	scope.record(&decisions).await?;
	Ok(allowed)
}
pub async fn allowed(scope: &mut dyn ThreadAuthorityScope, id: Uuid, action: &str) -> Result<bool> {
	let owner = scope.locked_owner(id).await?;
	decision(scope, id, action, owner.as_ref()).await
}
pub async fn require(scope: &mut dyn ThreadAuthorityScope, id: Uuid, action: &str) -> Result<()> {
	if allowed(scope, id, action).await? {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}
pub async fn visible(scope: &mut dyn ThreadAuthorityScope, action: &str) -> Result<Vec<Uuid>> {
	let mut result = vec![];
	for (id, owner) in scope.all_owners().await? {
		if decision(scope, id, action, Some(&owner)).await? {
			result.push(id);
		}
	}
	Ok(result)
}
pub async fn event_threads(
	scope: &mut dyn ThreadAuthorityScope,
	selected: Option<Uuid>,
) -> Result<Vec<Uuid>> {
	let rows = scope.event_owners(selected).await?;
	if rows.is_empty()
		&& let Some(id) = selected
	{
		decision(scope, id, "thread.read", None).await?;
	}
	let mut visible = vec![];
	for (id, owner) in rows {
		if decision(scope, id, "thread.read", Some(&owner)).await?
			&& decision(scope, id, "thread.events", Some(&owner)).await?
		{
			visible.push(id);
		}
	}
	Ok(visible)
}
pub async fn run_for_interaction(scope: &mut dyn RunInteractionScope, id: Uuid) -> Result<Run> {
	let run = scope.run(id).await?.ok_or(Error::Forbidden)?;
	interaction_of(scope, run).await
}
/// [`run_for_interaction`] for work done on the Run's behalf. A Run this Node executes for
/// another Home has no Thread here, so its visibility is its admission's: expiry, Peer
/// Mapping, the chain's read policies and then what the Run has read, as a Live preview's
/// recheck asks it (#904, #989). Every other Run goes through its Thread as before.
pub async fn run_for_action(scope: &mut dyn RunInteractionScope, id: Uuid) -> Result<Run> {
	let run = scope.run(id).await?.ok_or(Error::Forbidden)?;
	if scope.admitted_here(&(&run).into()).await? {
		if !scope.run_visible(&(&run).into()).await? {
			return Err(Error::Forbidden);
		}
		return Ok(run);
	}
	interaction_of(scope, run).await
}
async fn interaction_of(scope: &mut dyn RunInteractionScope, run: Run) -> Result<Run> {
	let thread = resource(scope, run.thread_id).await?;
	scope.set_context(thread.attributes.clone());
	scope.require(&thread, "thread.read").await?;
	if !scope.run_visible(&(&run).into()).await? {
		return Err(Error::Forbidden);
	}
	Ok(run)
}
#[cfg(test)]
mod tests;
