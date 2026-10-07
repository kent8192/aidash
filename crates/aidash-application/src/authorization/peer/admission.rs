//! Home callbacks precede local leases; current authority is rechecked at every executable boundary.
use crate::{
	Error, Result,
	ports::authorization::peer::{
		PeerAdmissionRecords, PeerAdmissionRepository, PeerAdmissionScope, PeerInspectionScope,
	},
};
use aidash_domain::{
	RunControl, RunMetadata, TaskStatus,
	federation::execution::{
		Description,
		admission::{
			Activation, Admission, InspectInput, Message, MessageReceipt, RemoteExecutionControl,
			RemoteExecutionControlState, RemoteExecutionPhase,
		},
	},
	registry::{AgentConfig, EntityRef},
	semantic::{Failure, remote::VERSION},
};
use serde_json::json;
use uuid::Uuid;

async fn finish<S: PeerAdmissionScope, T>(scope: S, result: Result<T>) -> Result<T> {
	match result {
		Ok(value) => {
			scope.finish(Ok(())).await?;
			Ok(value)
		}
		Err(error) => {
			scope.finish(Err(error)).await?;
			Err(Error::External(
				"receiver scope accepted a rejected effect".into(),
			))
		}
	}
}
fn semantic_failure(error: &Error) -> Failure {
	match error {
		Error::RemoteSemantic(reason) => *reason,
		Error::Forbidden | Error::Unauthorized => Failure::Authority,
		Error::Invalid(_) | Error::NotFound(_) => Failure::Configuration,
		_ => Failure::Unavailable,
	}
}
pub async fn run_grant<R: PeerAdmissionRecords + ?Sized>(
	repository: &R,
	run: &RunMetadata,
) -> Result<Option<Uuid>> {
	let Some(record) = repository.record(run.id).await? else {
		return Ok(None);
	};
	let description: Description = serde_json::from_value(record.description.clone())?;
	if !record.matches_run(run, &description) {
		return Err(Error::Forbidden);
	};
	Ok(Some(record.grant_id))
}
pub async fn receiver_lease<R: PeerAdmissionRepository>(
	repository: &R,
	source: &str,
	description: Description,
) -> Result<(R::Scope, Description)> {
	description.inspection.binding_snapshot.validate()?;
	if !description.inspection.binding_snapshot.remote {
		return Err(Error::Forbidden);
	}
	let mut scope = repository
		.mapped(
			source,
			&description.source_tenant,
			&description.source_subject,
		)
		.await?;
	let result = async {
		scope.context()["source_workspace_id"] = json!(description.task.workspace_id);
		scope.context()["source_task_id"] = json!(description.task.id);
		scope.context()["source_task_revision"] = json!(description.task.revision);
		let input = InspectInput {
			task_id: Some(description.task.id),
			tenant: description.source_tenant.clone(),
			subject: description.source_subject.clone(),
			agent: EntityRef {
				id: description.inspection.agent.id.clone(),
				version: description.inspection.agent.version.clone(),
			},
			requirements: serde_json::from_value(description.task.requirements.clone())?,
			compactor: description.semantic.request().compactor().cloned(),
		};
		let retained = if let Some(id) = scope.existing(source, description.grant_id).await? {
			scope.activation_run(id).await?.is_some_and(|run| {
				run.context.binding_snapshot.as_deref()
					== Some(&description.inspection.binding_snapshot)
			})
		} else {
			false
		};
		let fresh = if retained {
			super::execution::inspect_retained(&mut scope, source, &input, &description.inspection)
				.await?
		} else {
			super::execution::inspect(&mut scope, source, &input).await?
		};
		if !fresh.satisfies(&description.inspection) {
			return Err(Error::Forbidden);
		};
		let workspace = scope.resource(
			"workspace",
			&format!("{source}/workspaces/{}", description.task.workspace_id),
			json!({}),
		);
		scope.require(&workspace, "workspace.read").await?;
		if !description.semantic.disabled() {
			if fresh.semantic_memory != VERSION {
				return Err(Error::RemoteSemantic(Failure::Configuration));
			};
			scope.require(&workspace, "semantic.use").await?
		};
		let task = scope.resource(
			"task",
			&format!("{source}/tasks/{}", description.task.id),
			json!({"created_by":description.task.created_by,"requirements":description.task.requirements}),
		);
		scope.require(&task, "task.read").await?;
		scope.require(&task, "task.execute").await?;
		if !scope.receiver_live(description.expires_at).await? {
			return Err(Error::Forbidden);
		};
		Ok(())
	}
	.await;
	if let Err(error) = result {
		return finish(scope, Err(error)).await;
	};
	Ok((scope, description))
}
pub async fn lease<R: PeerAdmissionRepository>(
	repository: &R,
	source: &str,
	grant: Uuid,
) -> Result<(R::Scope, Description)> {
	// Home may inspect this receiver; no receiver transaction is held during this RPC.
	let description: Description = serde_json::from_value(
		repository
			.request(
				source,
				"/scoped/execution/grants/describe",
				&json!({"grant_id":grant}),
			)
			.await?,
	)?;
	if description.source_node != source
		|| description.target_node != repository.node_id()
		|| description.grant_id != grant
		|| description.task.status != TaskStatus::Open
	{
		return Err(Error::Forbidden);
	};
	receiver_lease(repository, source, description).await
}
pub async fn admit<R: PeerAdmissionRepository>(
	repository: &R,
	source: &str,
	grant: Uuid,
) -> Result<Admission> {
	let (mut scope, description) = lease(repository, source, grant).await?;
	let result = async {
		scope.lock_admission(source, &description).await?;
		if scope.legacy_conflict(source, &description, grant).await? {
			return Err(Error::Conflict(
				"task already has an incompatible execution".into(),
			));
		};
		let existing = scope.existing(source, grant).await?;
		if existing.is_none()
			&& !scope
				.active_installation(&description.inspection.agent)
				.await?
		{
			return Err(Error::Forbidden);
		};
		scope
			.insert(source, grant, &description, Uuid::new_v4())
			.await?;
		let record = scope.admitted(source, grant).await?.ok_or_else(|| {
			Error::Conflict("source task already has a different admission".into())
		})?;
		if !record.matches(&scope.identity(), scope.subjects(), &description)? {
			return Err(Error::Conflict(
				"admission already binds different authority".into(),
			));
		};
		if !scope.admission_live(description.expires_at).await? {
			return Err(Error::Forbidden);
		};
		scope.bind_foreign(&description, record.id, false).await?;
		Ok(record.view(&description))
	}
	.await;
	finish(scope, result).await
}
pub async fn verify<R: PeerAdmissionRepository>(
	repository: &R,
	source: &str,
	id: Uuid,
) -> Result<bool> {
	let record = repository
		.verify_record(id, source)
		.await?
		.ok_or(Error::Forbidden)?;
	let (scope, description) = lease(repository, source, record.grant_id).await?;
	let result = if record.matches(&scope.identity(), scope.subjects(), &description)? {
		Ok(true)
	} else {
		Err(Error::Forbidden)
	};
	finish(scope, result).await
}
pub async fn worker_lease<R: PeerAdmissionRepository>(
	repository: &R,
	run: &RunMetadata,
) -> Result<Option<(R::Scope, AgentConfig)>> {
	let Some(grant) = run_grant(repository, run).await? else {
		return Ok(None);
	};
	let (mut scope, description) = match lease(repository, &run.home_node, grant).await {
		Ok(value) => value,
		Err(error) => {
			let description: Description =
				serde_json::from_value(repository.description(run.id).await?)?;
			return Err(if description.semantic.disabled() {
				error
			} else {
				Error::RemoteSemantic(semantic_failure(&error))
			});
		}
	};
	let result = async {
		let record = scope.required_record(run.id).await?;
		if !record.matches(&scope.identity(), scope.subjects(), &description)? {
			return Err(Error::Forbidden);
		};
		scope.require_active(&description, run.id).await?;
		let agent = AgentConfig::from_snapshot(&description.inspection.binding_snapshot)?;
		scope.worker(true);
		Ok(agent)
	}
	.await;
	match result {
		Ok(agent) => Ok(Some((scope, agent))),
		Err(error) => finish(scope, Err(error)).await,
	}
}
pub async fn leaf_lease<R: PeerAdmissionRepository>(
	repository: &R,
	source: &str,
	grant: Uuid,
	id: Uuid,
) -> Result<(R::Scope, Description)> {
	let record = repository
		.leaf_record(source, grant, id)
		.await?
		.ok_or(Error::Forbidden)?;
	let description: Description = serde_json::from_value(record.description.clone())?;
	if description.source_node != source
		|| description.target_node != repository.node_id()
		|| description.grant_id != grant
	{
		return Err(Error::Forbidden);
	};
	// A Home callback uses the saved description and never recursively calls Home.
	let (mut scope, description) = receiver_lease(repository, source, description).await?;
	let result = async {
		if !record.matches(&scope.identity(), scope.subjects(), &description)? {
			return Err(Error::Forbidden);
		};
		let run = repository.run(id).await?;
		if run.home_node != source
			|| run.task_id != record.task_id
			|| run.control != RunControl::Active
			|| run.agent_id != description.inspection.agent.id
			|| run.agent_version != description.inspection.agent.version
		{
			return Err(Error::Forbidden);
		};
		scope.require_active(&description, id).await?;
		scope.worker(false);
		Ok(())
	}
	.await;
	match result {
		Ok(()) => Ok((scope, description)),
		Err(error) => finish(scope, Err(error)).await,
	}
}
pub async fn activate<R: PeerAdmissionRepository>(
	repository: &R,
	source: &str,
	id: Uuid,
	grant: Uuid,
) -> Result<Activation> {
	let bound: Uuid = serde_json::from_value(
		repository
			.request(
				source,
				"/scoped/execution/grants/activation",
				&json!({"grant_id":grant}),
			)
			.await?,
	)?;
	if bound != id {
		return Err(Error::Forbidden);
	};
	let (mut scope, description) = lease(repository, source, grant).await?;
	let result = async {
		let record = scope.activation_record(id).await?.ok_or(Error::Forbidden)?;
		if !record.matches(&scope.identity(), scope.subjects(), &description)? {
			return Err(Error::Forbidden);
		};
		scope.bind_foreign(&description, id, true).await?;
		scope.lock_activation(source, &description).await?;
		scope.insert_run(source, id, &description).await?;
		let run = scope
			.activation_run(id)
			.await?
			.ok_or_else(|| Error::Conflict("source task already has another execution".into()))?;
		if run.task_id != description.task.id
			|| run.workspace_id != description.task.workspace_id
			|| run.home_node != source
			|| run.agent_id != description.inspection.agent.id
			|| run.agent_version != description.inspection.agent.version
		{
			return Err(Error::Forbidden);
		};
		Ok(Activation {
			grant_id: grant,
			admission_id: id,
			run_id: id,
			phase: run.phase().into(),
			control: run.control.into(),
			semantic_reason: run.recovery.semantic_reason,
			error: run.error,
		})
	}
	.await;
	let result = finish(scope, result).await?;
	repository.notify();
	Ok(result)
}
pub async fn status<R: PeerAdmissionRepository>(
	repository: &R,
	source: &str,
	grant: Uuid,
) -> Result<Option<Activation>> {
	let Some(record) = repository.status_record(source, grant).await? else {
		return Ok(None);
	};
	let run = repository.status_run(record.id, source).await?;
	Ok(Some(Activation {
		grant_id: grant,
		admission_id: record.id,
		run_id: record.id,
		phase: run
			.as_ref()
			.map_or(RemoteExecutionPhase::Admitted, |r| r.phase().into()),
		control: run
			.as_ref()
			.map_or(RemoteExecutionControlState::Inactive, |r| r.control.into()),
		semantic_reason: run
			.as_ref()
			.and_then(|r| r.recovery.as_ref())
			.and_then(|r| r.semantic_reason),
		error: run.and_then(|r| {
			r.error
				.clone()
				.map(|_| {
					"Remote execution requires attention. Review current authority and retry controls.".into()
				})
		}),
	}))
}
pub async fn control<R: PeerAdmissionRepository>(
	repository: &R,
	source: &str,
	id: Uuid,
	grant: Uuid,
	action: RemoteExecutionControl,
) -> Result<Activation> {
	let run = repository.inspect_run(id).await?;
	if run.home_node != source || run_grant(repository, &run.metadata).await? != Some(grant) {
		return Err(Error::Forbidden);
	};
	if matches!(action, RemoteExecutionControl::Resume) {
		let (scope, _) = worker_lease(repository, &run.metadata)
			.await?
			.ok_or(Error::Forbidden)?;
		finish(scope, Ok(())).await?
	};
	let run = if matches!(action, RemoteExecutionControl::Cancel)
		&& run.control == RunControl::Cancelled
	{
		run
	} else {
		repository.control(id, &action).await?
	};
	repository.notify();
	Ok(Activation {
		grant_id: grant,
		admission_id: id,
		run_id: id,
		phase: run.phase().into(),
		control: run.control.into(),
		semantic_reason: run.recovery.as_ref().and_then(|r| r.semantic_reason),
		error: run
			.error
			.clone()
			.map(|_| "Remote execution requires attention.".into()),
	})
}
pub async fn message<R: PeerAdmissionRepository>(
	repository: &R,
	source: &str,
	id: Uuid,
	grant: Uuid,
	input: Message,
) -> Result<MessageReceipt> {
	let run = repository.run(id).await?;
	if run.home_node != source || run_grant(repository, &run.metadata()).await? != Some(grant) {
		return Err(Error::Forbidden);
	};
	let (mut scope, _) = worker_lease(repository, &run.metadata())
		.await?
		.ok_or(Error::Forbidden)?;
	let preflight = async {
		scope
			.require(
				&scope.resource("run", &id.to_string(), json!({})),
				"run.message",
			)
			.await?;
		scope
			.require(
				&scope.resource(
					"workspace",
					&format!("{source}/workspaces/{}", run.workspace_id),
					json!({}),
				),
				"message.create",
			)
			.await?;
		Ok(())
	}
	.await;
	finish(scope, preflight).await?;
	let home = repository.messages(&run);
	let key = format!("human:{id}:{}", input.id);
	let limit = repository.message_limit(&run).await?;
	if !home.reserve(&key, &input.content).await? {
		return Err(Error::Forbidden);
	};
	// Reservation grants no permission to enqueue; reacquire current receiver authority.
	let (scope, _) = worker_lease(repository, &run.metadata())
		.await?
		.ok_or(Error::Forbidden)?;
	let sender = scope.identity().subject;
	if let Err(error) = scope
		.accept_message(id, &sender, &input.content, &key, limit)
		.await && !repository.message_recorded(id, &key, &input.content).await
	{
		let _ = home.release(&key).await;
		return Err(error);
	};
	home.commit(&key, &input.content).await?;
	repository.deliver(&run).await?;
	repository.notify();
	Ok(MessageReceipt {
		id: input.id,
		run_id: id,
		accepted: true,
	})
}
