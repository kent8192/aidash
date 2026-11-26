//! Delete a Thread and enqueue removal of its Scratch Space directory.
use crate::{
	Error, Result,
	ports::capabilities::{
		cleanup::Creation,
		thread_lifecycle::{DeletionAuthority, ThreadScope},
	},
};
use aidash_domain::capabilities::thread_lifecycle::DeleteThread;
use aidash_domain::policy::Resource;
use serde_json::{Value, json};
use uuid::Uuid;

/// Deleting a Thread removes a whole conversation for everyone in it
/// (#1008): only its owner holding `thread.delete`, or an administrator of its
/// Node, may. A person who may only read it, or who holds `thread.delete`
/// without owning it, is refused.
pub async fn may_delete(
	scope: &mut dyn DeletionAuthority,
	principal: &str,
	resource: &Resource,
	thread: Uuid,
) -> Result<bool> {
	if resource.attributes["owner"].as_str() == Some(principal)
		&& scope.decide(resource, "thread.delete").await?
	{
		return Ok(true);
	}
	let node = scope.node_resource(thread).await?;
	scope
		.decide(
			&node,
			aidash_domain::identity::tenant_administration::NODE_ADMINISTER,
		)
		.await
}

pub async fn delete(
	scope: &mut dyn ThreadScope,
	thread: Uuid,
	input: DeleteThread,
) -> Result<Value> {
	scope.serialize().await?;
	// A replay returns the recorded result before the Thread is read again: the
	// deletion it records refuses every later read of the Thread. The record is
	// the same person's, for this Thread and these choices.
	let digest = aidash_domain::registry::rules::digest(&json!(["thread_delete", thread, input]));
	if let Some(cached) = scope.cached(input.idempotency_key, &digest).await? {
		return Ok(cached);
	}
	let resource = scope.thread(thread).await?;
	scope.require(&resource, "thread.read").await?;
	let principal = scope.principal().to_owned();
	if !may_delete(scope, &principal, &resource, thread).await? {
		return Err(Error::Forbidden);
	}
	scope.lock_for_deletion(thread).await?;
	scope.visible_thread(thread).await?;
	scope.delete_attachments(thread).await?;
	scope.stop_schedules(thread).await?;
	scope.mark_deleted(thread).await?;
	// The Thread is gone for everyone, so is its work: another person's Run
	// left running would find its Thread refused and pause where nobody can
	// see or stop it (#1008).
	scope.stop_work(thread).await?;
	// The tombstone hides the Thread everywhere (#1008, ADR 0140).
	scope
		.create(Creation {
			id: thread,
			area: None,
			kind: "thread_tombstone",
			state: "deleted",
			data: json!({"thread_id":thread}),
			expires: None,
		})
		.await?;
	let result = json!({"thread_id":thread,"state":"deleted"});
	scope.cache(input.idempotency_key, &digest, &result).await?;
	scope
		.event(thread, "thread.deleted", json!({"thread_id":thread}))
		.await?;
	Ok(result)
}

#[cfg(test)]
mod tests;
