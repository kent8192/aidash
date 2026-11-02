//! Run and event disclosure preserves complete provenance and live human membership.
use crate::{
	Result,
	ports::authorization::visibility::{
		EventVisibilityScope, LocalRunVisibilityScope, RunVerdicts, RunVisibilityScope,
	},
};
use aidash_domain::{Event, RunMetadata};
use serde_json::{Value, json};
use uuid::Uuid;

pub fn memory_attributes(run: &RunMetadata, mut attributes: Value) -> Value {
	attributes["created_by"] = json!(aidash_domain::agent_subject(
		&run.home_node,
		&run.agent_id,
		&run.agent_version
	));
	attributes["version"] = json!(run.agent_version);
	attributes
}

pub async fn local_base_visible(
	scope: &mut dyn LocalRunVisibilityScope,
	run: &RunMetadata,
) -> Result<bool> {
	if !scope.core_area_visible(run.id).await? {
		return Ok(false);
	}
	if let Some(allowed) = scope.cached(run) {
		return if allowed {
			scope.human_reads(run.workspace_id, run.id).await
		} else {
			Ok(false)
		};
	}
	let workspace = scope.workspace(run.workspace_id).await?;
	let resource = scope.resource("run", &run.id.to_string(), workspace.attributes.clone());
	let memory = scope.memory_resource(run, &workspace).await?;
	let task_visible = match scope.task(run).await? {
		Some(task) => scope.task_visible(&task).await?,
		None => false,
	};
	let allowed = task_visible
		&& scope.decide(&resource, "run.read").await?
		&& scope.decide(&memory, "memory.read").await?;
	scope.remember(run, allowed);
	Ok(allowed && scope.human_reads(run.workspace_id, run.id).await?)
}

pub async fn local_run_visible(
	scope: &mut dyn LocalRunVisibilityScope,
	run: &RunMetadata,
) -> Result<bool> {
	Ok(local_base_visible(scope, run).await? && scope.run_reads(run.id).await?)
}

/// A source Run with no native memory reads has no private memory dependency.
/// Keep the ordinary Run disclosure gate and its cached verdict unchanged.
pub async fn local_dependency_without_memory_visible(
	scope: &mut dyn LocalRunVisibilityScope,
	run: &RunMetadata,
) -> Result<bool> {
	if !scope.core_area_visible(run.id).await? {
		return Ok(false);
	}
	let workspace = scope.workspace(run.workspace_id).await?;
	let resource = scope.resource("run", &run.id.to_string(), workspace.attributes);
	let task_visible = match scope.task(run).await? {
		Some(task) => scope.task_visible(&task).await?,
		None => false,
	};
	Ok(task_visible
		&& scope.decide(&resource, "run.read").await?
		&& scope.human_reads(run.workspace_id, run.id).await?)
}

pub async fn base_visible(scope: &mut dyn RunVisibilityScope, run: &RunMetadata) -> Result<bool> {
	if !run.is_local(scope.node_id()) {
		if scope.scoped_admission(run.id).await? {
			return Ok(scope.core_area_visible(run.id).await?
				&& scope.foreign_base_visible(run).await?
				&& scope.human_reads(run.workspace_id, run.id).await?);
		}
		// Missing scoped admission is not a legacy grant; only an exact local record qualifies.
		if !scope.legacy_execution(run).await? {
			return Ok(false);
		}
	}
	local_base_visible(scope, run).await
}

/// Recheck the base policies without collapsing the owning Task's producer
/// journals into a boolean. The ordinary cached verdict may already be false
/// because one of those journals is invalidated.
pub async fn classified_base_visible<S>(
	scope: &mut S,
	run: &RunMetadata,
	pending: &mut Vec<Uuid>,
) -> Result<bool>
where
	S: RunVisibilityScope
		+ crate::ports::authorization::visibility::provenance::ReadProvenanceScope,
{
	if !run.is_local(scope.node_id()) {
		if scope.scoped_admission(run.id).await? {
			return Ok(scope.core_area_visible(run.id).await?
				&& scope.foreign_base_visible(run).await?
				&& scope.human_reads(run.workspace_id, run.id).await?);
		}
		if !scope.legacy_execution(run).await? {
			return Ok(false);
		}
	}
	if !scope.core_area_visible(run.id).await? {
		return Ok(false);
	}
	let workspace = LocalRunVisibilityScope::workspace(scope, run.workspace_id).await?;
	let resource = LocalRunVisibilityScope::resource(
		scope,
		"run",
		&run.id.to_string(),
		workspace.attributes.clone(),
	);
	let memory = scope.memory_resource(run, &workspace).await?;
	let Some(task) = scope.task(run).await? else {
		return Ok(false);
	};
	Ok(
		provenance::classified_task_visible(scope, &task, pending).await?
			&& LocalRunVisibilityScope::decide(scope, &resource, "run.read").await?
			&& LocalRunVisibilityScope::decide(scope, &memory, "memory.read").await?
			&& scope.human_reads(run.workspace_id, run.id).await?,
	)
}

pub async fn run_visible(scope: &mut dyn RunVisibilityScope, run: &RunMetadata) -> Result<bool> {
	Ok(base_visible(scope, run).await? && scope.run_reads(run.id).await?)
}

pub async fn event_visible(scope: &mut dyn EventVisibilityScope, event: &Event) -> Result<bool> {
	event_visible_in(scope, event, &mut RunVerdicts::default()).await
}

/// Event disclosure within one scan; `runs` holds the scan's Run verdicts.
pub async fn event_visible_in(
	scope: &mut dyn EventVisibilityScope,
	event: &Event,
	runs: &mut RunVerdicts,
) -> Result<bool> {
	if event.kind.starts_with("marketplace.") {
		return scope.marketplace_visible(event).await;
	}
	if let Some(visible) = scope.resource_visible(event).await? {
		return Ok(visible);
	}
	if event.kind.starts_with("generation.") {
		let Some(id) = event.data["id"]
			.as_str()
			.and_then(|s| s.parse::<Uuid>().ok())
		else {
			return Ok(false);
		};
		return match scope.generation(id, event.workspace_id).await? {
			Some(job) => scope.generation_visible(&job).await,
			None => Ok(false),
		};
	}
	if event.kind.starts_with("conversation.") {
		let Some(id) = event.data["id"]
			.as_str()
			.and_then(|s| s.parse::<Uuid>().ok())
		else {
			return Ok(false);
		};
		let Some(conversation) = scope.conversation(id, event.workspace_id).await? else {
			return Ok(false);
		};
		let resource = scope.conversation_resource(&conversation).await?;
		return scope.decide(&resource, "conversation.read").await;
	}
	if event.kind.starts_with("human.") {
		let Some(id) = event.data["id"]
			.as_str()
			.and_then(|s| s.parse::<Uuid>().ok())
		else {
			return Ok(false);
		};
		let Some(request) = scope.human(id, event.workspace_id).await? else {
			return Ok(false);
		};
		if !scope.human_visible(&request).await? {
			return Ok(false);
		}
	}
	let candidate = event.data.get("run_id").or_else(|| {
		(event.kind == "run.created")
			.then(|| event.data.get("id"))
			.flatten()
	});
	if candidate.is_none() {
		return Ok(matches!(
			event.kind.as_str(),
			"workspace.created" | "workspace.updated"
		));
	}
	let Some(id) = candidate
		.and_then(Value::as_str)
		.and_then(|id| id.parse::<Uuid>().ok())
	else {
		return Ok(false);
	};
	if let Some(visible) = runs.get(id, event.workspace_id) {
		return Ok(visible);
	}
	let visible = match scope.run(id, event.workspace_id).await? {
		Some(run) => scope.run_visible(&run).await?,
		None => false,
	};
	runs.remember(id, event.workspace_id, visible);
	Ok(visible)
}

#[cfg(test)]
mod tests;

pub mod resources;

pub mod provenance;
