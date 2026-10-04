//! Foreign execution uses the same binding fences and current Home permissions.
use crate::{
	Error, Result,
	ports::generation::foreign::{ForeignGenerationBinding, ForeignGenerationGuard},
};
use aidash_domain::{
	Task,
	federation::execution::Description,
	generation::intent::{Intent, guards},
	registry::EntityRef,
};
use serde_json::{Value, json};
use uuid::Uuid;

pub async fn home_authority(
	scope: &mut dyn ForeignGenerationGuard,
	task: &Task,
	target: &str,
	policy_id: &str,
) -> Result<()> {
	let workspace = scope.workspace(task.workspace_id).await?;
	scope.require(&workspace, "workspace.read").await?;
	scope.require(&workspace, "generation.disclose").await?;
	let resource = scope.task_resource(task).await?;
	scope.require(&resource, "task.read").await?;
	scope.require(&resource, "task.delegate").await?;
	scope
		.require(
			&scope.resource("node", target, json!({})),
			"federation.execute",
		)
		.await?;
	scope
		.require(
			&scope.resource(
				"generation_policy",
				&format!("{target}/generation-policies/{policy_id}"),
				json!({"remote_node":target}),
			),
			"generation.request",
		)
		.await
}

pub async fn inspect(
	scope: &mut dyn ForeignGenerationGuard,
	source: &str,
	task: Option<Uuid>,
	agent: &EntityRef,
) -> Result<Option<Value>> {
	let Some(job) = scope.job(agent).await? else {
		return Ok(None);
	};
	if !guards::inspection_matches(&job, &scope.authority(), source, task, scope.now()) {
		return Err(Error::Forbidden);
	}
	Ok(job.foreign_intent)
}

pub async fn bind(
	scope: &mut dyn ForeignGenerationBinding,
	description: &Description,
	admission: Uuid,
	activate: bool,
) -> Result<()> {
	let Some(value) = &description.inspection.generation else {
		return Ok(());
	};
	let intent: Intent = serde_json::from_value(value.clone())?;
	let job = scope
		.prepared(&description.source_node, description.task.id)
		.await?;
	if !guards::binding_matches(&job, value, &intent, description, admission, scope.now()) {
		return Err(Error::Forbidden);
	}
	scope.bind(job.id, description.grant_id, admission).await?;
	if activate && job.status == "QUEUED" {
		scope.activate(&job).await?;
	}
	Ok(())
}

pub async fn check_home(
	scope: &mut dyn ForeignGenerationGuard,
	task: &Task,
	node: &str,
	generation: Option<&Value>,
) -> Result<()> {
	let Some(generation) = generation else {
		return Ok(());
	};
	let intent: Intent = serde_json::from_value(generation.clone())?;
	let record = scope.intent(intent.id).await?.ok_or(Error::Forbidden)?;
	if !guards::home_matches(
		&record,
		generation,
		&intent,
		task,
		node,
		&scope.authority(),
		scope.now(),
	) {
		return Err(Error::Forbidden);
	}
	if scope.lineage(&intent.home_node).await? != intent.lineage {
		return Err(Error::Forbidden);
	}
	home_authority(scope, task, node, &intent.policy_id).await
}

pub async fn require_active(
	scope: &mut dyn ForeignGenerationGuard,
	description: &Description,
	run: Uuid,
) -> Result<()> {
	if description.inspection.generation.is_none() {
		return Ok(());
	}
	if !scope.active(description, run).await? {
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub fn check_preparation(task: &Task, generation: Option<&Value>) -> Result<()> {
	if let Some(value) = generation {
		let intent: Intent = serde_json::from_value(value.clone())?;
		if !intent.preparation_matches(task) {
			return Err(Error::Conflict(
				"generation intent task revision changed before grant preparation".into(),
			));
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests;

pub mod home;

pub mod receiver;

pub mod maintenance;
