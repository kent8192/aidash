//! Complete inherited authority before evaluating ordered mutations.
use crate::{Error, Result, ports::transactions::TransactionAuthorityScope};
use aidash_domain::{
	RunMetadata,
	transactions::{
		Manifest, Mutation,
		authority::{Origin, Preflight, Target, chain_compatible},
		share::Share,
	},
};
use serde_json::json;
/// The metadata-only preflight for the participant holding `share`.
pub fn request(share: &Share, origin: &Origin) -> Result<Preflight> {
	Ok(Preflight {
		id: share.id,
		coordinator: share.coordinator.clone(),
		digest: share.digest()?,
		origin: origin.clone(),
		recipients: share.recipients(),
		targets: targets(&share.part.mutations),
	})
}

/// The preflight the coordinator sends `node`, from the manifest sealed with
/// its `seed`.
pub fn sealed_request(
	manifest: &Manifest,
	seed: &str,
	origin: &Origin,
	node: &str,
) -> Result<Preflight> {
	manifest.local(node).map_err(|_| Error::Forbidden)?;
	request(&manifest.share(seed, node)?, origin)
}

fn targets(mutations: &[Mutation]) -> Vec<Target> {
	mutations
		.iter()
		.map(|mutation| match mutation {
			Mutation::ThreadTitle { thread_id, .. } => Target {
				kind: "thread".into(),
				id: *thread_id,
				task_id: None,
			},
			Mutation::CompleteTask { task_id, .. } => Target {
				kind: "task".into(),
				id: *task_id,
				task_id: None,
			},
			Mutation::FinishRun {
				run_id, task_id, ..
			} => Target {
				kind: "run".into(),
				id: *run_id,
				task_id: Some(*task_id),
			},
		})
		.collect()
}
pub async fn inherit_run(
	scope: &mut dyn TransactionAuthorityScope,
	run: &RunMetadata,
	input: &Preflight,
) -> Result<()> {
	let previous = scope.subjects().to_vec();
	if scope.source_node() != Some(run.home_node.as_str()) {
		scope.inherit_local_run(run).await?;
	} else {
		let admission = scope
			.source_admission(run, &input.coordinator)
			.await?
			.ok_or(Error::Forbidden)?;
		if !admission.matches(&scope.identity(), &input.origin, run) {
			return Err(Error::Forbidden);
		}
		scope.set_subjects(admission.subject_chain);
	}
	if !chain_compatible(&previous, scope.subjects()) {
		return Err(Error::Forbidden);
	}
	Ok(())
}
pub async fn checks(
	scope: &mut dyn TransactionAuthorityScope,
	input: &Preflight,
	node: &str,
	action: &str,
) -> Result<()> {
	let transaction = scope.resource(
		"transaction",
		input.id,
		json!({"coordinator":input.coordinator,"participants":input.recipients}),
	);
	scope.require(&transaction, action).await?;
	// Establish every stored origin before evaluating any mutation.
	for target in &input.targets {
		scope.require_target_node(node, target).await?;
		match target.kind.as_str() {
			"task" => scope.inherit_task(target.id).await?,
			"run" => {
				let run = scope.run(target.id).await?;
				if Some(run.task_id) != target.task_id {
					return Err(Error::Forbidden);
				}
				inherit_run(scope, &run, input).await?;
			}
			"thread" => {}
			_ => return Err(Error::Invalid("unsupported transaction target".into())),
		}
	}
	scope.require(&transaction, action).await?;
	for target in &input.targets {
		match target.kind.as_str() {
			"thread" => {
				let resource = scope.thread(target.id).await?;
				scope.require(&resource, "thread.read").await?;
				if action != "transaction.read" {
					scope.require(&resource, "thread.update").await?;
				}
			}
			"task" => {
				let task = scope.task(target.id).await?;
				let resource = scope.task_resource(&task).await?;
				if action != "transaction.read" {
					scope.require(&resource, "task.complete").await?;
					let artifact = scope
						.artifact_creation_resource(task.id, task.owner.as_deref().unwrap_or(""))
						.await?;
					scope.require(&artifact, "artifact.create").await?;
				}
			}
			"run" => {
				let run = scope.run(target.id).await?;
				let resource = scope.resource(
					"run",
					run.id,
					json!({"task_id":run.task_id,"thread_id":run.thread_id,"home_node":run.home_node}),
				);
				scope.require(&resource, "run.read").await?;
				if action != "transaction.read" {
					scope.require(&resource, "run.finish").await?;
				}
			}
			_ => return Err(Error::Forbidden),
		}
	}
	Ok(())
}
#[cfg(test)]
mod tests;

/// The submitting subject's authority over every target of `manifest`, its
/// own Node's targets through the full checks and every other Node's through
/// its qualified resources.
pub async fn source_checks(
	scope: &mut dyn TransactionAuthorityScope,
	manifest: &Manifest,
	seed: &str,
	origin: &Origin,
	action: &str,
) -> Result<()> {
	checks(
		scope,
		&sealed_request(manifest, seed, origin, &manifest.coordinator)?,
		&manifest.coordinator,
		action,
	)
	.await?;
	for participant in &manifest.participants {
		if participant.node_id == manifest.coordinator {
			continue;
		}
		for target in targets(&participant.mutations) {
			let resource = scope.qualified_resource(&target.kind, &participant.node_id, target.id);
			scope.require(&resource, action).await?;
		}
	}
	Ok(())
}

pub mod control;
