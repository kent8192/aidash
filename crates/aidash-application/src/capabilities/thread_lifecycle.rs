//! Deleting a Thread preserves each owned Area's explicit retention and journal.
use crate::{
	Error, Result,
	capabilities::cleanup,
	ports::capabilities::{
		cleanup::Creation,
		thread_lifecycle::{DeletionAuthority, ThreadScope},
	},
};
use aidash_domain::capabilities::{
	cleanup::{Choice, Cleanup},
	sessions::Area,
	thread_lifecycle::{DeleteThread, choices, require_idle},
};
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
	let mut choices = choices(input.files)?;
	let mut results = vec![];
	let mut offset = 0_u64;
	loop {
		// Area rows remain in this query after cleanup, so ordered offset pages
		// keep a stable membership while bounding each database fetch.
		let areas: Vec<Area> = scope.owned_areas(thread, offset).await?;
		if areas.is_empty() {
			break;
		}
		offset += areas.len() as u64;
		for area in areas {
			let choice = choices
				.remove(&area.id)
				.ok_or_else(|| Error::Conflict("CHOOSE_RETENTION_FOR_EACH_AREA".into()))?;
			let mut area = cleanup::load(scope, area.id, "file.manage").await?;
			require_idle(&area, choice.expected_revision)?;
			let keep = matches!(choice.choice, Choice::Keep);
			let result = cleanup::prepare(
				scope,
				area.id,
				Cleanup {
					idempotency_key: Uuid::new_v4(),
					expected_revision: area.revision,
					choice: choice.choice,
					confirmation_id: choice.confirmation_id,
				},
			)
			.await?;
			if keep && area.state == "active" {
				area.generation += 1;
				area.epoch += 1;
				area.state = "retained".into();
				scope.publish(&mut area).await?;
				scope.persist(&area, false).await?;
				let mut record = scope.load_record(result.operation_id, "cleanup").await?;
				record.data["snapshot"] = area.manifest.clone();
				record.data["retained"] = json!(true);
				record.data["generation"] = json!(area.generation);
				record.data["area_revision"] = json!(area.revision);
				scope.update(&mut record).await?;
			}
			// Cancelling the old generation does not erase its journal or bytes.
			scope.cancel_generation(&area).await?;
			results.push(json!(result));
		}
	}
	if !choices.is_empty() {
		return Err(Error::Conflict("CHOOSE_RETENTION_FOR_EACH_AREA".into()));
	}
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
			data: json!({"thread_id":thread,"file_choices":results}),
			expires: None,
		})
		.await?;
	let result = json!({"thread_id":thread,"state":"deleted","file_operations":results});
	scope.cache(input.idempotency_key, &digest, &result).await?;
	scope
		.event(thread, "thread.deleted", json!({"thread_id":thread}))
		.await?;
	Ok(result)
}

#[cfg(test)]
mod tests;
