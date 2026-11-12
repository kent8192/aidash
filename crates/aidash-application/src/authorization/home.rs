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
			PrepareInput, Refusal,
			admission::{
				Activation, Admission, Declined, Message, MessageReceipt, RemoteExecutionControl,
				RemoteExecutionControlState,
			},
			home::{FollowUpInput, Grant, Status},
		},
	},
	nonempty,
	registry::EntityRef,
	semantic::{
		Failure,
		recovery::Recovery,
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
			grant.workspace_id,
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
/// A person on the Home declines the call a remote Run is paused on (#763). The requester needs
/// what `control` needs for a resume; anyone else needs `run.decline_call` and may read the Task.
pub async fn decline_call<R: HomeRepository>(
	repository: &R,
	task: Uuid,
	id: Uuid,
	call: &str,
	rest_of_run: bool,
) -> Result<Activation> {
	if call.is_empty() || call.len() > 256 {
		return Err(Error::Invalid("call key is empty or too long".into()));
	}
	let identity = subject(repository)?;
	let mut scope = repository.begin().await?;
	let found = scope.task_grant(task, id).await;
	let grant = finish(scope, found).await?.ok_or(Error::Forbidden)?;
	let own = grant.tenant == identity.tenant && grant.root_subject == identity.subject;
	if own {
		requester(repository, task, id, true).await?;
	}
	let mut scope = repository.begin().await?;
	let result = async {
		let task = if own {
			managed_task(&mut scope, task, true).await?
		} else {
			scope.task_read(task).await?
		};
		let resource = scope.task_resource(&task).await?;
		let binding = scope.binding(id).await?.ok_or(Error::Forbidden)?;
		let run = scope.resource("run", binding.admission_id, resource.attributes);
		scope
			.require(
				&run,
				if own {
					"run.control"
				} else {
					"run.decline_call"
				},
			)
			.await?;
		Ok((binding.admission_id, task.workspace_id))
	}
	.await;
	let (admission, workspace) = finish(scope, result).await?;
	if !serde_json::from_value::<Binding>(grant.semantic.clone())?.disabled() {
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
	let mut input = json!({"grant_id":id,"call":call});
	if rest_of_run {
		input["rest_of_run"] = json!(true);
	}
	let declined: Declined = repository
		.request(
			grant.workspace_id,
			&grant.node_id,
			&format!("/scoped/execution/admissions/{admission}/declined-calls"),
			&input,
		)
		.await?;
	let activation = declined.activation;
	if activation.grant_id != id
		|| activation.admission_id != admission
		|| activation.run_id != admission
	{
		return Err(Error::Forbidden);
	}
	// The receiver's event does not name the person; this one does.
	let mut scope = repository.begin().await?;
	let result = scope
		.call_declined(
			workspace,
			json!({
				"task_id": task,
				"grant_id": id,
				"run_id": admission,
				"call": call,
				"tool": declined.tool,
				"kind": declined.kind,
				"by": identity.subject,
				"rest_of_run": rest_of_run,
			}),
		)
		.await;
	finish(scope, result).await?;
	Ok(activation)
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
			grant.workspace_id,
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
				let owner = aidash_domain::agent_subject(
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
		// The error comes from the peer's status reply, whose 404 used to read as `Forbidden`.
		Error::Forbidden | Error::Unauthorized | Error::NotFound(_) => Failure::Authority,
		Error::Invalid(_) => Failure::Configuration,
		_ => Failure::Unavailable,
	}
}
pub async fn list<R: HomeRepository>(repository: &R, task: Uuid) -> Result<Vec<Status>> {
	subject(repository)?;
	let mut scope = repository.begin().await?;
	let result = async {
		let grants = scope.grants(task).await?;
		if !grants.is_empty() {
			managed_task(&mut scope, task, false).await?;
		}
		Ok(grants)
	}
	.await;
	let (grants, requester_denied) = match finish(scope, result).await {
		Ok(grants) => (grants, false),
		Err(Error::Forbidden) => (vec![], true),
		Err(error) => return Err(error),
	};
	let has_requester_grants = !grants.is_empty();
	let mut task_read_denied = false;
	let holder_result = async {
		let mut scope = repository.begin().await?;
		let result = async {
			if let Err(error) = scope.task_read(task).await {
				task_read_denied = matches!(&error, Error::Forbidden);
				return Err(error);
			}
			scope.holder_grants(task).await
		}
		.await;
		finish(scope, result).await
	}
	.await;
	let holder_grants = match holder_result {
		Ok(grants) => grants,
		Err(error) if has_requester_grants => {
			tracing::warn!(%error, "holder grant lookup failed");
			vec![]
		}
		Err(Error::Forbidden) if task_read_denied => vec![],
		Err(error) => return Err(error),
	};
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
				grant.workspace_id,
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
		let mut semantic = repository
			.semantic_status(grant.id, &binding, reason)
			.await?;
		// A cancelled or finished remote Run keeps its cause visible but, like the
		// local management view, offers no action that would resume it. A paused
		// remote Run claims no retry, so like a paused local Run it has none
		// scheduled.
		match status.as_ref().ok().and_then(Option::as_ref) {
			Some(activation) if !activation.recoverable() => semantic.recovery = None,
			Some(activation)
				if matches!(activation.control, RemoteExecutionControlState::Paused) =>
			{
				semantic.recovery = semantic
					.recovery
					.map(|recovery| Recovery::new(recovery.cause, false));
			}
			_ => {}
		}
		let mut execution = status.as_ref().ok().cloned().flatten();
		// The paused call is shown only to a caller who may decline it; `list` already
		// narrowed these grants to the requester, whose check is `run.control`.
		if execution
			.as_ref()
			.is_some_and(|activation| activation.declinable_call.is_some())
		{
			let allowed = async {
				let mut scope = repository.begin().await?;
				let result = async {
					let task = scope.task_read(task).await?;
					let resource = scope.task_resource(&task).await?;
					let binding = scope.binding(grant.id).await?.ok_or(Error::Forbidden)?;
					let run = scope.resource("run", binding.admission_id, resource.attributes);
					scope.may(&run, "run.control").await
				}
				.await;
				finish(scope, result).await
			}
			.await
			.unwrap_or(false);
			if !allowed && let Some(activation) = execution.as_mut() {
				activation.declinable_call = None;
			}
		}
		Ok(Status {
			requester: true,
			human_requests,
			semantic,
			grant: grant.prepared()?,
			unavailable: status.is_err(),
			execution,
		})
	}))
	.buffered(4)
	.collect()
	.await;
	let mut result: Vec<Status> = results.into_iter().collect::<Result<_>>()?;
	let holders: Vec<Result<Option<Status>>> =
		stream::iter(holder_grants.into_iter().map(|grant| async move {
			if !holder_may_decline(repository, task, grant.id).await? {
				return Ok(None);
			}
			let status: Result<Option<Activation>> = tokio::time::timeout(
				std::time::Duration::from_secs(2),
				repository.request(
					grant.workspace_id,
					&grant.node_id,
					"/scoped/execution/status",
					&json!({"grant_id":grant.id}),
				),
			)
			.await
			.unwrap_or_else(|_| Err(Error::External("remote status unavailable".into())));
			let Ok(Some(activation)) = status else {
				return Ok(None);
			};
			if activation.control != RemoteExecutionControlState::Paused
				|| !activation.recoverable()
			{
				return Ok(None);
			}
			if activation.declinable_call.is_none() {
				return Ok(None);
			}
			// Authority or the grant may have changed while the peer RPC was in flight.
			if !holder_may_decline(repository, task, grant.id).await? {
				return Ok(None);
			}
			Ok(Some(Status {
				requester: false,
				grant: grant.prepared()?,
				execution: Some(activation),
				unavailable: false,
				human_requests: vec![],
				semantic: aidash_domain::semantic::remote::status::Status {
					state: "disabled".into(),
					body_cleanup: None,
					reason: None,
					operation_id: None,
					retry_count: 0,
					retry_at: None,
					result_count: None,
					truncated: false,
					retrieved_at: None,
					recovery: None,
				},
			}))
		}))
		.buffered(4)
		.collect()
		.await;
	match holders.into_iter().collect::<Result<Vec<_>>>() {
		Ok(holders) => result.extend(holders.into_iter().flatten()),
		Err(error) if has_requester_grants => tracing::warn!(%error, "holder grant recheck failed"),
		Err(error) => return Err(error),
	}
	if (requester_denied || task_read_denied) && result.is_empty() {
		return Err(Error::Forbidden);
	}
	Ok(result)
}

async fn holder_may_decline<R: HomeRepository>(
	repository: &R,
	task: Uuid,
	grant: Uuid,
) -> Result<bool> {
	// A non-requester follows the same Task and Run checks as the decline route.
	let mut scope = repository.begin().await?;
	let result = async {
		let Some(found) = scope.task_grant(task, grant).await? else {
			return Ok(false);
		};
		if found.revoked || found.expires_at <= chrono::Utc::now() {
			return Ok(false);
		}
		let identity = scope.identity();
		if found.tenant == identity.tenant && found.root_subject == identity.subject {
			return Ok(false);
		}
		let task = scope.task_read(task).await?;
		let resource = scope.task_resource(&task).await?;
		let Some(binding) = scope.binding(grant).await? else {
			return Ok(false);
		};
		let run = scope.resource("run", binding.admission_id, resource.attributes);
		scope.may(&run, "run.decline_call").await
	}
	.await;
	match finish(scope, result).await {
		Err(Error::Forbidden) => Ok(false),
		result => result,
	}
}
/// A check that fails after the target Node has acted. The `task_delegate` tool tells the model
/// the delegation may have partly happened, where pausing would resume into a second admission;
/// the management endpoint keeps `Forbidden`.
fn after_effect(by_tool: bool, what: &str) -> Error {
	if by_tool {
		Error::Refused(format!(
			"{what}, so the delegation may have partly happened. Do not repeat this call; check the task first"
		))
	} else {
		Error::Forbidden
	}
}
/// What the model hears when this Node, not the target, refuses a step after the target admitted.
const UNRECORDED: &str =
	"the target Node admitted the task, but this Node could not record the admission";
/// The error of a step that follows the target Node's admission, as the model should hear it.
/// The management endpoint gets every error unchanged. For the tool, a refusal or a failed
/// request becomes [`after_effect`], since the effect has happened and pausing would resume into
/// a second admission. An ask stays an Authority Pause, as ADR 0109 decided for another Node.
/// Everything else keeps its meaning: a `Conflict` is an internal fault that still fails the Run,
/// and a pending transaction or a database error has its own retry path.
fn refused_after_effect(by_tool: bool, what: &str, error: Error) -> Error {
	if !by_tool {
		return error;
	}
	match error {
		Error::RemoteExecutionRefused(Refusal::PolicyAsk | Refusal::NodeUseAsk) => error,
		Error::RemoteExecutionRefused(reason) => {
			after_effect(by_tool, &format!("{what}: {reason}"))
		}
		Error::Forbidden | Error::Unauthorized | Error::NotFound(_) | Error::External(_) => {
			after_effect(by_tool, what)
		}
		error => error,
	}
}
pub async fn activate<R: HomeRepository>(
	repository: &R,
	task: Uuid,
	id: Uuid,
) -> Result<Activation> {
	activate_as(repository, task, id, false).await
}
async fn activate_as<R: HomeRepository>(
	repository: &R,
	task: Uuid,
	id: Uuid,
	by_tool: bool,
) -> Result<Activation> {
	let grant = requester(repository, task, id, true).await?;
	// Admission performs fresh source and receiver checks after the first scope is completed.
	let admission: Admission = repository
		.request(
			grant.workspace_id,
			&grant.node_id,
			"/scoped/execution/admissions",
			&json!({"grant_id":id}),
		)
		.await?;
	// From here on the target Node has acted, so the tool hears a refusal of any later step as a
	// partial effect.
	let source = repository
		.node_address(grant.workspace_id)
		.await
		.map_err(|error| refused_after_effect(by_tool, UNRECORDED, error))?;
	if admission.grant_id != id || admission.source_node != source || admission.task_id != task {
		return Err(after_effect(
			by_tool,
			"the target Node admitted the task, but its admission did not match this request",
		));
	}
	let (mut scope, description) = repository
		.description(&grant.node_id, id)
		.await
		.map_err(|error| refused_after_effect(by_tool, UNRECORDED, error))?;
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
	finish(scope, result)
		.await
		.map_err(|error| refused_after_effect(by_tool, UNRECORDED, error))?;
	let activation: Activation = repository
		.request(
			grant.workspace_id,
			&grant.node_id,
			&format!("/scoped/execution/admissions/{}/activate", admission.id),
			&json!({"grant_id":id}),
		)
		.await
		.map_err(|error| {
			refused_after_effect(
				by_tool,
				"the target Node admitted the task, but did not activate it",
				error,
			)
		})?;
	if activation.run_id != admission.id
		|| activation.admission_id != admission.id
		|| activation.grant_id != id
	{
		return Err(after_effect(
			by_tool,
			"the target Node activated the task, but its activation did not match this request",
		));
	}
	// Only a matching response says the Run started. An error or an uncertain
	// outcome above leaves the binding unmarked: if the Run did start, its
	// claim moves the Task out of OPEN (#772).
	//
	// The receiver has already started the Run, so a failure to record it must
	// not report the activation as failed: that claim moves the Task out of OPEN
	// anyway, and only a binding without the mark could be read as unstarted.
	let marked = async {
		let mut scope = repository.begin().await?;
		let result = scope.mark_activated(id).await;
		finish(scope, result).await
	}
	.await;
	if let Err(error) = marked {
		tracing::warn!(task = %task, grant = %id, %error, "activation was not recorded on the binding");
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
/// Delegate `task` to an Agent on another Node. `by_tool` says whether a Run's tool asks, whose
/// refusals reach the model, or a person through the API, who gets the plain error.
pub async fn delegate<R: HomeRepository>(
	repository: &R,
	task: Uuid,
	node: &str,
	agent: &EntityRef,
	by_tool: bool,
) -> Result<Delegation> {
	subject(repository)?;
	let mut scope = repository.begin().await?;
	let result = async {
		// For the model, a missing task and one the Run may not see read the same.
		let task = scope.task_read(task).await.map_err(|error| match error {
			Error::Forbidden if by_tool => super::worker_tasks::task_not_visible(task),
			error => error,
		})?;
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
					semantic: Request::Disabled { summarizer: None },
				},
			)
			.await?;
	}
	activate_as(repository, task, id, by_tool).await?;
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
