//! Scoped effects preserve authorization order, durable replay and the final lease check.
use super::{Admission, Command, Prepared, prepare};
use crate::{
	Error, Result,
	ports::authorization::commands::effects::{
		Applied, Effect, RemoteCommandEffects, WriteContext,
	},
};
use aidash_domain::{
	ArtifactInput, NewTask, TaskStatus,
	identity::commands::{
		self, Metadata,
		effects::{AuditError, audit_event, child_eligible, field},
	},
	registry::{EntityRef, Entry},
};
use serde_json::{Value, json};
use uuid::Uuid;
pub async fn execute(
	scope: &mut dyn RemoteCommandEffects,
	input: Command<'_>,
	agent: &Entry,
) -> Result<Value> {
	let command = Command {
		grant_id: input.grant_id,
		admission_id: input.admission_id,
		operation: input.operation,
		data: input.data,
		node: input.node,
		agent: input.agent,
		version: input.version,
	};
	let Prepared {
		task,
		owner,
		metadata,
		key,
		task_resource,
	} = match prepare(scope, command).await? {
		Admission::Replay(result) => return Ok(result),
		Admission::Ready(prepared) => *prepared,
	};
	let Metadata {
		mutation,
		digest,
		request_key,
	} = metadata;
	let data = input.data;
	let context = || WriteContext {
		task: &task,
		owner: &owner,
		key: &key,
		admission: input.admission_id,
	};
	let result = match input.operation {
		"task" => json!(task),
		"snapshot" => scope.snapshot(task.workspace_id).await?,
		"workspace_record" | "workspace_record_chunk" => {
			let id: Uuid = serde_json::from_value(data["id"].clone())?;
			let kind = field(data, "kind")?;
			let record = scope.record(task.workspace_id, kind, id).await?;
			if input.operation == "workspace_record_chunk" {
				let offset = serde_json::from_value(data["offset"].clone())?;
				let max_chars: usize = serde_json::from_value(data["max_chars"].clone())?;
				if max_chars > 16000 {
					return Err(Error::Invalid(
						"workspace chunk exceeds 16000 characters".into(),
					));
				}
				aidash_domain::context::observation::chunk_record(
					record,
					kind,
					&id.to_string(),
					offset,
					max_chars,
				)?
			} else {
				record
			}
		}
		"workspace_children" => {
			let parent: Uuid = serde_json::from_value(data["parent_id"].clone())?;
			if parent != task.id {
				return Err(Error::Forbidden);
			}
			scope.children(task.workspace_id, parent).await?
		}
		"claim" => {
			scope.require(&task_resource, "task.execute").await?;
			if data["entry"] != json!(agent) {
				return Err(Error::Forbidden);
			}
			scope
				.apply(
					context(),
					Effect::Claim {
						revision: serde_json::from_value(data["revision"].clone())?,
						agent,
					},
				)
				.await?
				.value
		}
		"transition" | "run_message_terminal_transition" => {
			scope.require(&task_resource, "task.execute").await?;
			let revision = serde_json::from_value(data["revision"].clone())?;
			let next: TaskStatus = serde_json::from_value(data["status"].clone())?;
			let through = if input.operation == "run_message_terminal_transition" {
				let seq: i64 = serde_json::from_value(data["through_seq"].clone())?;
				if seq < 0 {
					return Err(Error::Invalid("invalid terminal sequence".into()));
				}
				Some(seq)
			} else {
				None
			};
			scope
				.apply(
					context(),
					Effect::Transition {
						revision,
						next,
						through,
					},
				)
				.await?
				.value
		}
		"run_message_complete" | "artifact" => {
			let resource = scope.artifact_creation_resource(task.id, &owner).await?;
			scope.require(&resource, "artifact.create").await?;
			let artifact: ArtifactInput = serde_json::from_value(data["artifact"].clone())?;
			let complete_through = if input.operation == "run_message_complete" {
				scope.require(&task_resource, "task.complete").await?;
				let seq: i64 = serde_json::from_value(data["through_seq"].clone())?;
				if seq < 0 {
					return Err(Error::Invalid("invalid input sequence".into()));
				}
				Some(seq)
			} else {
				None
			};
			let result = scope
				.apply(
					context(),
					Effect::Artifact {
						artifact: &artifact,
						complete_through,
					},
				)
				.await?;
			track(
				scope,
				&result,
				input.grant_id,
				task.workspace_id,
				"artifact",
			)
			.await?;
			result.value
		}
		"create_task" => {
			let resource = scope.workspace(task.workspace_id).await?;
			scope.require(&resource, "task.create").await?;
			let new: NewTask = serde_json::from_value(data["task"].clone())?;
			if new.parent_id != Some(task.id) {
				return Err(Error::Forbidden);
			}
			for id in &new.dependencies {
				let dependency = scope.task(*id).await?;
				if dependency.workspace_id != task.workspace_id {
					return Err(Error::Forbidden);
				}
			}
			let result = scope
				.apply(context(), Effect::CreateTask { task: &new })
				.await?;
			track(scope, &result, input.grant_id, task.workspace_id, "task").await?;
			result.value
		}
		"delegate" => {
			let id: Uuid = serde_json::from_value(data["task_id"].clone())?;
			let child = scope.task(id).await?;
			if !child_eligible(&task, &child, &owner) {
				return Err(Error::Forbidden);
			}
			let created = scope.created_by_grant(input.grant_id, child.id).await?;
			if !created || field(data, "node_id")? != scope.local_node() {
				return Err(Error::Forbidden);
			}
			let agent: EntityRef = serde_json::from_value(data["agent"].clone())?;
			scope
				.apply(
					context(),
					Effect::Delegate {
						child: child.id,
						agent: &agent,
					},
				)
				.await?
				.value
		}
		"message" | "run_message_output" => {
			let resource = scope.workspace(task.workspace_id).await?;
			scope.require(&resource, "message.create").await?;
			let content = field(data, "content")?;
			let through = if input.operation == "run_message_output" {
				Some(serde_json::from_value(data["included_input_seq"].clone())?)
			} else {
				None
			};
			let result = scope
				.apply(context(), Effect::Message { content, through })
				.await?;
			track(scope, &result, input.grant_id, task.workspace_id, "message").await?;
			json!({"sent":true})
		}
		"run_message_delivery_capability" => json!({"protocol":2}),
		"run_message_reserve" | "run_message_commit" | "run_message_delivery" => {
			let run = scope.resource(
				"run",
				&input.admission_id.to_string(),
				task_resource.attributes.clone(),
			);
			scope.require(&run, "run.message").await?;
			let resource = scope.workspace(task.workspace_id).await?;
			scope.require(&resource, "message.create").await?;
			let key = field(data, "key")?;
			commands::require_input_key(key, input.admission_id)?;
			let content = field(data, "content")?;
			aidash_domain::nonempty(content, "run message")?;
			let effect = match input.operation {
				"run_message_reserve" => Effect::ReserveInput {
					node: input.node,
					key,
					content,
				},
				"run_message_commit" => Effect::CommitInput {
					key,
					content,
					seq: serde_json::from_value(data["input_seq"].clone())?,
				},
				_ => Effect::DeliverInput {
					node: input.node,
					key,
					content,
				},
			};
			scope.apply(context(), effect).await?.value
		}
		"run_message_release" | "run_message_ack" => {
			let keys: Vec<String> = serde_json::from_value(data["keys"].clone())?;
			if keys.len() > 1024 {
				return Err(Error::Invalid("too many input keys".into()));
			}
			for key in &keys {
				commands::require_input_key(key, input.admission_id)?;
			}
			if input.operation == "run_message_release" {
				scope
					.apply(
						context(),
						Effect::ReleaseInputs {
							node: input.node,
							keys: &keys,
						},
					)
					.await?;
			}
			json!({"acknowledged":true})
		}
		"event" => {
			let (kind, detail) =
				audit_event(data, input.admission_id).map_err(|error| match error {
					AuditError::Unbound => Error::Forbidden,
					AuditError::Invalid(error) => error.into(),
				})?;
			scope.apply(context(), Effect::Event {kind, data:json!({"task_id":task.id,"grant_id":input.grant_id,"remote_run_id":input.admission_id,"detail":detail})}).await?;
			json!({"recorded":true})
		}
		"run_message_history" => {
			let offset: usize = serde_json::from_value(data["offset"].clone())?;
			let rows = scope
				.history(task.workspace_id, task.id, input.node, offset)
				.await?;
			let mut visible = Vec::new();
			for message in rows {
				visible.push(
					scope
						.record(task.workspace_id, "message", message.id)
						.await?,
				);
			}
			json!(visible)
		}
		_ => return Err(Error::Invalid("unsupported scoped home operation".into())),
	};
	if mutation {
		scope
			.persist_replay(input.grant_id, task.id, &request_key, &digest, &result)
			.await?;
	}
	if !scope.live(input.grant_id).await? {
		return Err(Error::Forbidden);
	}
	Ok(result)
}
async fn track(
	scope: &mut dyn RemoteCommandEffects,
	result: &Applied,
	grant: Uuid,
	workspace: Uuid,
	kind: &str,
) -> Result<()> {
	let id = result
		.output_id
		.ok_or_else(|| Error::External("remote output identifier missing".into()))?;
	scope.record_output(grant, workspace, kind, id).await
}
#[cfg(test)]
mod tests;
