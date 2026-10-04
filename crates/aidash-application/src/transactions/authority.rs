//! Complete inherited authority before evaluating ordered mutations and disclosures.
use crate::{Error, Result, ports::transactions::TransactionAuthorityScope};
use aidash_domain::{
	RunMetadata,
	transactions::{
		Manifest, Mutation,
		authority::{Origin, Preflight, Target, chain_compatible},
	},
};
use serde_json::json;
pub fn request(manifest: &Manifest, origin: &Origin, node: &str) -> Result<Preflight> {
	let targets = manifest
		.local(node)
		.map_err(|_| Error::Forbidden)?
		.mutations
		.iter()
		.map(|mutation| {
			Ok(match mutation {
				Mutation::WorkspaceState { workspace_id, .. } => Target {
					kind: "workspace".into(),
					id: *workspace_id,
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
				Mutation::RegistryRegister { .. } => return Err(Error::Forbidden),
			})
		})
		.collect::<Result<Vec<_>>>()?;
	Ok(Preflight {
		id: manifest.id,
		coordinator: manifest.coordinator.clone(),
		digest: manifest.digest()?,
		origin: origin.clone(),
		recipients: manifest
			.participants
			.iter()
			.map(|participant| participant.node_id.clone())
			.collect(),
		targets,
	})
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
		match target.kind.as_str() {
			"task" => scope.inherit_task(target.id).await?,
			"run" => {
				let run = scope.run(target.id).await?;
				if Some(run.task_id) != target.task_id {
					return Err(Error::Forbidden);
				}
				inherit_run(scope, &run, input).await?;
			}
			"workspace" => {}
			_ => return Err(Error::Invalid("unsupported transaction target".into())),
		}
	}
	scope.require(&transaction, action).await?;
	for target in &input.targets {
		let resource = match target.kind.as_str() {
			"workspace" => {
				let resource = scope.workspace(target.id).await?;
				scope.require(&resource, "workspace.read").await?;
				if action != "transaction.read" {
					scope.require(&resource, "workspace.update").await?;
				}
				resource
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
				resource
			}
			"run" => {
				let run = scope.run(target.id).await?;
				let resource = scope.resource(
					"run",
					run.id,
					json!({"task_id":run.task_id,"workspace_id":run.workspace_id,"home_node":run.home_node}),
				);
				scope.require(&resource, "run.read").await?;
				if action != "transaction.read" {
					scope.require(&resource, "run.finish").await?;
				}
				resource
			}
			_ => return Err(Error::Forbidden),
		};
		if action != "transaction.read" {
			for recipient in &input.recipients {
				let mut disclosure = resource.clone();
				disclosure.attributes["recipient_node"] = json!(recipient);
				scope.require(&disclosure, "transaction.disclose").await?;
			}
		}
	}
	Ok(())
}
#[cfg(test)]
mod tests;

pub async fn source_checks(
	scope: &mut dyn TransactionAuthorityScope,
	manifest: &Manifest,
	origin: &Origin,
	action: &str,
) -> Result<()> {
	checks(
		scope,
		&request(manifest, origin, &manifest.coordinator)?,
		action,
	)
	.await?;
	for participant in &manifest.participants {
		if participant.node_id == manifest.coordinator {
			continue;
		}
		for target in request(manifest, origin, &participant.node_id)?.targets {
			let resource = scope.qualified_resource(&target.kind, &participant.node_id, target.id);
			scope.require(&resource, action).await?;
			for recipient in &manifest.participants {
				if action != "transaction.read" {
					let mut disclosure = resource.clone();
					disclosure.attributes["recipient_node"] = json!(recipient.node_id);
					scope.require(&disclosure, "transaction.disclose").await?;
				}
			}
		}
	}
	Ok(())
}

pub mod control;
