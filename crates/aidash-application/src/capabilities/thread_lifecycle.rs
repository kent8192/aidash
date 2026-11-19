//! Deleting a Workspace preserves each owned Area's explicit retention and journal.
use crate::{
	Error, Result,
	capabilities::cleanup,
	ports::capabilities::{cleanup::Creation, thread_lifecycle::ThreadScope},
};
use aidash_domain::capabilities::{
	cleanup::{Choice, Cleanup},
	sessions::Area,
	thread_lifecycle::{DeleteThread, choices, require_idle},
};
use serde_json::{Value, json};
use uuid::Uuid;
pub async fn delete(
	scope: &mut dyn ThreadScope,
	workspace: Uuid,
	input: DeleteThread,
) -> Result<Value> {
	scope.serialize().await?;
	let resource = scope.workspace(workspace).await?;
	scope.require(&resource, "workspace.read").await?;
	scope.require(&resource, "thread.delete").await?;
	let digest =
		aidash_domain::registry::rules::digest(&json!(["thread_delete", workspace, input]));
	if let Some(cached) = scope.cached(input.idempotency_key, &digest).await? {
		return Ok(cached);
	}
	scope.lock_for_deletion(workspace).await?;
	scope.visible_thread(workspace).await?;
	let mut choices = choices(input.files)?;
	let mut results = vec![];
	let mut offset = 0_u64;
	loop {
		// Area rows remain in this query after cleanup, so ordered offset pages
		// keep a stable membership while bounding each database fetch.
		let areas: Vec<Area> = scope.owned_areas(workspace, offset).await?;
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
	scope.delete_attachments(workspace).await?;
	scope.stop_schedules(workspace).await?;
	// The tombstone hides the Workspace everywhere (#1008, ADR 0140).
	scope
		.create(Creation {
			id: workspace,
			area: None,
			kind: "thread_tombstone",
			state: "deleted",
			data: json!({"workspace_id":workspace,"file_choices":results}),
			expires: None,
		})
		.await?;
	let result = json!({"workspace_id":workspace,"state":"deleted","file_operations":results});
	scope.cache(input.idempotency_key, &digest, &result).await?;
	scope
		.event(
			workspace,
			"workspace.deleted",
			json!({"workspace_id":workspace}),
		)
		.await?;
	Ok(result)
}
