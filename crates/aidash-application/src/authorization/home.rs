//! Home manages remote work with current authority and releases source locks before peer RPCs.
use crate::{
	Error, Result,
	ports::authorization::home::{HomeRepository, HomeScope},
};
use aidash_domain::{
	NewTask, Task, TaskStatus,
	federation::{
		Delegation,
		execution::{
			PrepareInput,
			admission::{Activation, Admission, Message, MessageReceipt, RemoteExecutionControl},
			home::{FollowUpInput, Grant, Status},
		},
	},
	nonempty, qualified_agent,
	registry::EntityRef,
	semantic::{
		Failure,
		remote::{Binding, Request, status::Provenance},
	},
};
use futures_util::{StreamExt, stream};
use serde_json::json;
use uuid::Uuid;
async fn finish<S: HomeScope, T>(scope: S, result: Result<T>) -> Result<T> {
	match result {
		Ok(value) => {
			scope.finish(Ok(())).await?;
			Ok(value)
		}
		Err(error) => {
			scope.finish(Err(error)).await?;
			Err(Error::External(
				"source scope accepted a rejected effect".into(),
			))
		}
	}
}
fn subject<R: HomeRepository>(
	repository: &R,
) -> Result<aidash_domain::identity::execution::ExecutionPrincipal> {
	repository.identity().ok_or(Error::Forbidden)
}
async fn managed_task<S: HomeScope>(scope: &mut S, id: Uuid, read: bool) -> Result<Task> {
	if read {
		return scope.task_read(id).await;
	}
	let task = scope.control_task(id).await?.ok_or(Error::Forbidden)?;
	let resource = scope.task_resource(&task).await?;
	scope.require(&resource, "task.delegate").await?;
	Ok(task)
}
async fn requester<R: HomeRepository>(
	repository: &R,
	task: Uuid,
	grant: Uuid,
	read: bool,
) -> Result<Grant> {
	subject(repository)?;
	let mut scope = repository.begin().await?;
	let result = async {
		let task_resource = managed_task(&mut scope, task, read).await?;
		let resource = scope.task_resource(&task_resource).await?;
		scope.require(&resource, "task.delegate").await?;
		scope
			.requester_grant(task, grant)
			.await?
			.ok_or(Error::Forbidden)
	}
	.await;
	finish(scope, result).await
}
pub async fn message<R: HomeRepository>(
	repository: &R,
	task: Uuid,
	id: Uuid,
	input: Message,
) -> Result<MessageReceipt> {
	nonempty(&input.content, "run message")?;
	subject(repository)?;
	let grant = requester(repository, task, id, true).await?;
	let mut scope = repository.begin().await?;
	let result = async {
		let task = scope.task_read(task).await?;
		let resource = scope.workspace(task.workspace_id).await?;
		scope.require(&resource, "message.create").await?;
		let binding = scope.binding(id).await?.ok_or(Error::Forbidden)?;
		let run = scope.resource("run", binding.admission_id, resource.attributes);
		scope.require(&run, "run.message").await?;
		Ok(binding.admission_id)
	}
	.await;
	let admission = finish(scope, result).await?;
	let receipt: MessageReceipt = repository
		.request(
			&grant.node_id,
			&format!("/scoped/execution/admissions/{admission}/messages"),
			&json!({"grant_id":id,"message":input}),
		)
		.await?;
	if receipt.id != input.id || receipt.run_id != admission || !receipt.accepted {
		return Err(Error::Forbidden);
	}
	Ok(receipt)
}
pub async fn control<R: HomeRepository>(
	repository: &R,
	task: Uuid,
	id: Uuid,
	action: RemoteExecutionControl,
) -> Result<Activation> {
	let identity = subject(repository)?;
	let grant = requester(
		repository,
		task,
		id,
		matches!(action, RemoteExecutionControl::Resume),
	)
	.await?;
	let mut scope = repository.begin().await?;
	let result = async {
		let task = managed_task(
			&mut scope,
			task,
			matches!(action, RemoteExecutionControl::Resume),
		)
		.await?;
		let resource = scope.task_resource(&task).await?;
		let binding = scope.binding(id).await?.ok_or(Error::Forbidden)?;
		let run = scope.resource("run", binding.admission_id, resource.attributes);
		scope.require(&run, "run.control").await?;
		Ok(binding.admission_id)
	}
	.await;
	let admission = finish(scope, result).await?;
	if matches!(action, RemoteExecutionControl::Resume)
		&& !serde_json::from_value::<Binding>(grant.semantic.clone())?.disabled()
	{
		let (mut authority, description) = repository.description(&grant.node_id, id).await?;
		let result = authority
			.resume_semantic(
				id,
				admission,
				description.task.workspace_id,
				&identity.subject,
			)
			.await;
		finish(authority, result).await?;
	}
	let result = repository
		.request(
			&grant.node_id,
			&format!("/scoped/execution/admissions/{admission}/control"),
			&json!({"grant_id":id,"action":action}),
		)
		.await?;
	if matches!(action, RemoteExecutionControl::Cancel) {
		// Receiver stop precedes Home ledger closure. No source row locks span that RPC.
		let mut scope = repository.begin().await?;
		let result = async {
			let current = managed_task(&mut scope, task, false).await?;
			let resource = scope.task_resource(&current).await?;
			let run = scope.resource("run", admission, resource.attributes);
			scope.require(&run, "run.control").await?;
			// Revocation shares the lease row with every Home command; completed effects stay fenced.
			scope.revoke(id).await?;
			if !matches!(
				current.status,
				TaskStatus::Cancelled | TaskStatus::Completed | TaskStatus::Failed
			) {
				let keys = scope.delivered_inputs(task, admission).await?;
				let owner = qualified_agent(
					&grant.node_id,
					&grant.prepared()?.agent.id,
					&grant.prepared()?.agent.version,
				);
				let cancelled = scope
					.cancel_task(task, current.revision, &owner, admission, &keys)
					.await?;
				scope.binding_revision(id, cancelled.revision).await?;
			}
			Ok(())
		}
		.await;
		finish(scope, result).await?;
	}
	Ok(result)
}
fn semantic_failure(error: &Error) -> Failure {
	match error {
		Error::RemoteSemantic(reason) => *reason,
		Error::Forbidden | Error::Unauthorized => Failure::Authority,
		Error::Invalid(_) | Error::NotFound(_) => Failure::Configuration,
		_ => Failure::Unavailable,
	}
}
pub async fn list<R: HomeRepository>(repository: &R, task: Uuid) -> Result<Vec<Status>> {
	subject(repository)?;
	let mut scope = repository.begin().await?;
	let result = async {
		managed_task(&mut scope, task, false).await?;
		scope.grants(task).await
	}
	.await;
	let grants = finish(scope, result).await?;
	let results: Vec<Result<Status>> = stream::iter(grants.into_iter().map(|grant| async move {
		// Home-owned continuations have their own bounded read, completed before
		// peer RPC so a slow receiver cannot hide an already durable request.
		let requests = tokio::time::timeout(std::time::Duration::from_secs(2), async {
			let mut scope = repository.begin().await?;
			let result = async {
				scope
					.requester_grant(task, grant.id)
					.await?
					.ok_or(Error::Forbidden)?;
				match scope.binding(grant.id).await? {
					Some(bound) => scope.human_requests(grant.id, bound.admission_id).await,
					None => Ok(vec![]),
				}
			}
			.await;
			finish(scope, result).await
		})
		.await;
		// RPC and authority recheck share the original total two-second per-grant budget.
		let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
		let mut status: Result<Option<Activation>> = tokio::time::timeout_at(
			deadline,
			repository.request(
				&grant.node_id,
				"/scoped/execution/status",
				&json!({"grant_id":grant.id}),
			),
		)
		.await
		.unwrap_or_else(|_| Err(Error::External("remote status unavailable".into())));
		let binding: Binding = serde_json::from_value(grant.semantic.clone())?;
		let mut reason = status
			.as_ref()
			.ok()
			.and_then(|s| s.as_ref())
			.and_then(|s| s.semantic_reason);
		if !binding.disabled() {
			if let Err(error) = &status {
				reason = Some(semantic_failure(error));
			} else {
				let allowed = tokio::time::timeout_at(deadline, async {
					let mut reader = repository.begin().await?;
					let result = async {
						reader.remote_semantic_sources(grant.id).await?;
						reader.grant_output_visible(grant.id).await
					}
					.await;
					finish(reader, result).await
				})
				.await;
				match allowed {
					Ok(Ok(true)) => {}
					Ok(Err(Error::RemoteSemantic(failure))) => reason = Some(failure),
					Ok(_) => reason = Some(Failure::Authority),
					Err(_) => {
						reason = Some(Failure::Unavailable);
						status = Err(Error::External("remote status unavailable".into()));
					}
				}
			}
		}
		let human_requests = match requests {
			Ok(result) => result?,
			Err(_) => {
				status = Err(Error::External("remote status unavailable".into()));
				if !binding.disabled() {
					reason = Some(Failure::Unavailable);
				}
				vec![]
			}
		};
		let semantic = repository
			.semantic_status(grant.id, &binding, reason)
			.await?;
		Ok(Status {
			human_requests,
			semantic,
			grant: grant.prepared()?,
			unavailable: status.is_err(),
			execution: status.ok().flatten(),
		})
	}))
	.buffered(4)
	.collect()
	.await;
	results.into_iter().collect()
}
pub async fn activate<R: HomeRepository>(
	repository: &R,
	task: Uuid,
	id: Uuid,
) -> Result<Activation> {
	let grant = requester(repository, task, id, true).await?;
	// Admission performs fresh source and receiver checks after the first scope is completed.
	let admission: Admission = repository
		.request(
			&grant.node_id,
			"/scoped/execution/admissions",
			&json!({"grant_id":id}),
		)
		.await?;
	if admission.grant_id != id
		|| admission.source_node != repository.node_id()
		|| admission.task_id != task
	{
		return Err(Error::Forbidden);
	}
	let (mut scope, description) = repository.description(&grant.node_id, id).await?;
	let result = async {
		scope
			.insert_binding(id, task, &admission, &description)
			.await?;
		let current = scope
			.binding(id)
			.await?
			.ok_or_else(|| Error::Conflict("task already has another scoped execution".into()))?;
		if !current.matches(admission.id, task, &json!(description.task)) {
			return Err(Error::Conflict("remote execution identity changed".into()));
		}
		Ok(())
	}
	.await;
	finish(scope, result).await?;
	let activation: Activation = repository
		.request(
			&grant.node_id,
			&format!("/scoped/execution/admissions/{}/activate", admission.id),
			&json!({"grant_id":id}),
		)
		.await?;
	if activation.run_id != admission.id
		|| activation.admission_id != admission.id
		|| activation.grant_id != id
	{
		return Err(Error::Forbidden);
	}
	Ok(activation)
}
pub async fn activation_binding<R: HomeRepository>(
	repository: &R,
	node: &str,
	grant: Uuid,
) -> Result<Uuid> {
	let (mut scope, _) = repository.description(node, grant).await?;
	let result = async {
		Ok(scope
			.binding(grant)
			.await?
			.ok_or(Error::Forbidden)?
			.admission_id)
	}
	.await;
	finish(scope, result).await
}
pub async fn delegate<R: HomeRepository>(
	repository: &R,
	task: Uuid,
	node: &str,
	agent: &EntityRef,
) -> Result<Delegation> {
	subject(repository)?;
	let mut scope = repository.begin().await?;
	let result = async {
		let task = scope.task_read(task).await?;
		let resource = scope.task_resource(&task).await?;
		scope.require(&resource, "task.delegate").await?;
		let grants = scope.delegation_grants(&task, node).await?;
		Ok(grants
			.into_iter()
			.find(|g| {
				g.inspection["agent"]["id"] == agent.id
					&& g.inspection["agent"]["version"] == agent.version
			})
			.map(|g| g.id))
	}
	.await;
	let prior = finish(scope, result).await?;
	let id = prior.unwrap_or_else(Uuid::new_v4);
	if prior.is_none() {
		repository
			.prepare(
				task,
				PrepareInput {
					id,
					node_id: node.into(),
					agent: agent.clone(),
					ttl_seconds: 3600,
					semantic: Request::Disabled {},
				},
			)
			.await?;
	}
	activate(repository, task, id).await?;
	Ok(Delegation {
		task_id: task,
		node_id: node.into(),
		agent_id: agent.id.clone(),
		agent_version: agent.version.clone(),
		delivered: true,
	})
}
pub async fn follow_up<R: HomeRepository>(
	repository: &R,
	task: Uuid,
	id: Uuid,
	input: FollowUpInput,
) -> Result<Task> {
	nonempty(&input.title, "follow-up title")?;
	nonempty(&input.description, "follow-up description")?;
	let grant = requester(repository, task, id, false).await?;
	let identity = subject(repository)?;
	let mut scope = repository.begin().await?;
	let result = async {
		let prior = managed_task(&mut scope, task, false).await?;
		let workspace = scope.workspace(prior.workspace_id).await?;
		scope.require(&workspace, "task.create").await?;
		let binding = scope.binding(id).await?.ok_or(Error::Forbidden)?;
		let run = scope.resource("run", binding.admission_id, workspace.attributes);
		scope.require(&run, "run.control").await?;
		let key = format!("remote-follow-up:{}:{}", grant.id, input.id);
		// Independent intent avoids importing invalid producer context through parent or dependency links.
		let input = NewTask {
			title: input.title,
			description: input.description,
			requirements: serde_json::to_value(input.requirements)?,
			dependencies: vec![],
			parent_id: None,
		};
		scope
			.create_task(prior.workspace_id, &input, &identity.subject, &key)
			.await
	}
	.await;
	finish(scope, result).await
}
pub async fn provenance<R: HomeRepository>(
	repository: &R,
	task: Uuid,
	id: Uuid,
) -> Result<Option<Provenance>> {
	requester(repository, task, id, true).await?;
	subject(repository)?;
	let mut scope = repository.begin().await?;
	let result = async {
		if !scope.grant_output_visible(id).await? {
			return Err(Error::Forbidden);
		}
		let receipt = scope.receipt(id).await?;
		scope.provenance(receipt).await
	}
	.await;
	finish(scope, result).await
}
#[cfg(test)]
mod tests;

pub async fn answer_human<R: HomeRepository>(
	repository: &R,
	task: Uuid,
	grant: Uuid,
	id: Uuid,
	response: serde_json::Value,
) -> Result<aidash_domain::HumanRequest> {
	subject(repository)?;
	let mut scope = repository.begin().await?;
	let result = async {
		managed_task(&mut scope, task, true).await?;
		let current = scope
			.requester_grant(task, grant)
			.await?
			.ok_or(Error::Forbidden)?;
		if current.revoked || current.expires_at <= chrono::Utc::now() {
			return Err(Error::Forbidden);
		}
		let bound = scope.binding(grant).await?.ok_or(Error::Forbidden)?;
		scope
			.answer_human(grant, bound.admission_id, id, response)
			.await
	}
	.await;
	finish(scope, result).await
}
