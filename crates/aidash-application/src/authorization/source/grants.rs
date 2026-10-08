//! Preparation, description and revocation share the original Source transaction boundaries.
use crate::{
	Error, Result,
	ports::authorization::{
		home::HomeScope,
		source::{
			SourceAuthorityScope, SourcePeerScope,
			grants::{GrantRepository, GrantScope},
		},
	},
};
use aidash_domain::{
	TaskStatus,
	configuration::validate_node_id,
	federation::execution::{
		Description, Inspection, PrepareInput, Prepared, admission::InspectInput,
		home::PreparationAuthority,
	},
	policy::SubjectKind,
	qualified_agent,
	registry::{EntityRef, Search},
	semantic::{Failure, remote::Binding},
};
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
async fn inspect<R: GrantRepository>(
	repository: &R,
	scope: &mut R::Scope,
	node: &str,
	input: InspectInput,
) -> Result<Inspection> {
	let resource = scope.source_resource("node", node, json!({"remote_node":node}));
	scope
		.source_require(&resource, "federation.execute")
		.await?;
	require_peer(scope, node, repository.protocol_version()).await?;
	let inspection:Inspection=repository.source_request(node,"/scoped/execution/inspect",&json!({"tenant":input.tenant,"subject":input.subject,"task_id":input.task_id,"agent":input.agent,"requirements":input.requirements,"compactor":input.compactor})).await?;
	crate::federation::admission::validate_inspection(
		&repository.validation(),
		&inspection,
		node,
		&input.agent,
		&input.requirements,
	)?;
	if inspection.compactor != input.compactor {
		return Err(Error::Forbidden);
	}
	Ok(inspection)
}
fn inspection_input<S: HomeScope>(
	scope: &S,
	task: Uuid,
	agent: &EntityRef,
	requirements: Search,
	compactor: Option<&EntityRef>,
) -> InspectInput {
	let identity = scope.identity();
	InspectInput {
		task_id: Some(task),
		tenant: identity.tenant,
		subject: identity.subject,
		agent: agent.clone(),
		requirements,
		compactor: compactor.cloned(),
	}
}
/// Inspect under the same task, subject chain and peer authority as preparation,
/// without creating a grant or disclosing Home memory.
pub async fn agent_inspection<R: GrantRepository>(
	repository: &R,
	task_id: Uuid,
	input: aidash_domain::federation::execution::AgentInspectionInput,
) -> Result<aidash_domain::federation::execution::AgentMemoryRequirements> {
	repository.source_identity().ok_or(Error::Forbidden)?;
	validate_node_id(&input.node_id)?;
	if input.node_id == repository.source_node_id() {
		return Err(Error::Invalid(
			"inspection requires a remote destination".into(),
		));
	}
	let mut scope = repository.source_begin().await?;
	let result = async {
		scope.inherit_task_origin(task_id).await?;
		let task = scope.task_read(task_id).await?;
		if task.status != TaskStatus::Open {
			return Err(Error::Conflict("task is already assigned".into()));
		}
		if scope.source_subjects().len() >= 32 {
			return Err(Error::Invalid(
				"execution delegation depth exceeds 32".into(),
			));
		}
		let executor = qualified_agent(&input.node_id, &input.agent.id, &input.agent.version);
		if scope
			.source_bundle()
			.subjects
			.get(&executor)
			.is_none_or(|subject| subject.kind != SubjectKind::Agent)
		{
			return Err(Error::Forbidden);
		}
		scope.append_subject(executor);
		let workspace = scope.workspace(task.workspace_id).await?;
		scope.source_context(workspace.attributes.clone());
		let resource = scope.task_resource(&task).await?;
		scope.require(&resource, "task.delegate").await?;
		scope.require(&resource, "task.execute").await?;
		let requirements: Search = serde_json::from_value(task.requirements.clone())?;
		let request = inspection_input(&scope, task.id, &input.agent, requirements, None);
		let inspection = inspect(repository, &mut scope, &input.node_id, request).await?;
		crate::generation::foreign::check_preparation(&task, inspection.generation.as_ref())?;
		super::authorize(&mut scope, &task, &input.node_id, &inspection).await?;
		super::semantic::requirements(&inspection)
	}
	.await;
	finish(scope, result).await
}

pub async fn prepare<R: GrantRepository>(
	repository: &R,
	task_id: Uuid,
	input: PrepareInput,
) -> Result<Prepared> {
	let identity = repository.source_identity().ok_or(Error::Forbidden)?;
	if input.id.is_nil()
		|| input.node_id == repository.source_node_id()
		|| !(1..=3600).contains(&input.ttl_seconds)
	{
		return Err(Error::Invalid(
			"invalid remote grant destination or lifetime".into(),
		));
	}
	validate_node_id(&input.node_id)?;
	let mut scope = repository.source_begin().await?;
	let result=async {
  scope.inherit_task_origin(task_id).await?;
  let task=scope.task_read(task_id).await?;
  if task.status!=TaskStatus::Open {return Err(Error::Conflict("task is already assigned".into()));}
  if scope.source_subjects().len()>=32 {return Err(Error::Invalid("execution delegation depth exceeds 32".into()));}
  let executor=qualified_agent(&input.node_id,&input.agent.id,&input.agent.version);
  if scope.source_bundle().subjects.get(&executor).is_none_or(|subject|subject.kind!=SubjectKind::Agent) {return Err(Error::Forbidden);}
  scope.append_subject(executor);
  let workspace=scope.workspace(task.workspace_id).await?;
  scope.source_context(workspace.attributes.clone());
  let resource=scope.task_resource(&task).await?;
  scope.require(&resource,"task.delegate").await?;
  scope.require(&resource,"task.execute").await?;
  let requirements:Search=serde_json::from_value(task.requirements.clone())?;
  let request=inspection_input(&scope,task.id,&input.agent,requirements,input.semantic.compactor());
  let inspection=inspect(repository,&mut scope,&input.node_id,request).await?;
  crate::generation::foreign::check_preparation(&task,inspection.generation.as_ref())?;
  let semantic=serde_json::to_value(scope.semantic_binding(&task,&input.node_id,&inspection,&input.semantic).await?)?;
  super::authorize(&mut scope,&task,&input.node_id,&inspection).await?;
  let metadata=serde_json::to_value(&inspection)?;
  // Keep the original task revision through persistence, after authorizing its read.
  let current=scope.locked_task(task_id).await?;
  if current.revision!=task.revision || current.status!=TaskStatus::Open {return Err(Error::Conflict("task changed during grant preparation".into()));}
  let inserted=scope.insert_grant(&input,&task,&metadata).await?;
  if inserted==1 {scope.persist_semantic(input.id,&semantic).await?;}
  let grant=scope.current_grant(input.id).await?;
  let expected=PreparationAuthority {task_id,task:&task,input:&input,identity:&identity,subjects:scope.source_subjects(),inspection:&metadata,semantic:&semantic};
  if !grant.matches_authority(&expected) || !scope.live(grant.id).await? {return Err(Error::Conflict("grant id already binds different or expired authority".into()));}
  if inserted==1 {scope.event(task.workspace_id,"task.remote_grant_prepared",json!({"grant_id":grant.id,"task_id":task_id,"node_id":grant.node_id,"expires_at":grant.expires_at})).await?;}
  Ok(grant.prepared()?)
 }.await;
	finish(scope, result).await
}
pub async fn revoke<R: GrantRepository>(
	repository: &R,
	task_id: Uuid,
	id: Uuid,
) -> Result<Prepared> {
	repository.source_identity().ok_or(Error::Forbidden)?;
	let mut scope = repository.source_begin().await?;
	let result = async {
		let task = scope.task_read(task_id).await?;
		let resource = scope.task_resource(&task).await?;
		scope.require(&resource, "task.delegate").await?;
		let mut grant = scope
			.revocation_grant(task_id, id)
			.await?
			.ok_or(Error::Forbidden)?;
		if !grant.revoked {
			scope.revoke_locked(id).await?;
			scope
				.event(
					task.workspace_id,
					"task.remote_grant_revoked",
					json!({"grant_id":id,"task_id":task_id}),
				)
				.await?;
			grant.revoked = true;
		}
		Ok(grant.prepared()?)
	}
	.await;
	finish(scope, result).await
}
/// Read-only description retries only the race between authorized task read and shared row lock.
pub async fn description<R: GrantRepository>(
	repository: &R,
	node: &str,
	id: Uuid,
) -> Result<(R::Scope, Description)> {
	for attempt in 0..3 {
		let mut revision_race = false;
		match description_mode(repository, node, id, false, &mut revision_race).await {
			Err(Error::Forbidden) if revision_race && attempt < 2 => {
				tokio::time::sleep(std::time::Duration::from_millis(10)).await
			}
			result => return result,
		}
	}
	unreachable!("the last verification attempt returns")
}
pub async fn description_mode<R: GrantRepository>(
	repository: &R,
	node: &str,
	id: Uuid,
	command: bool,
	revision_race: &mut bool,
) -> Result<(R::Scope, Description)> {
	let grant = repository
		.source_grant(id, node)
		.await?
		.ok_or(Error::Forbidden)?;
	let mut scope = repository.begin_grant(&grant).await.map_err(|error| {
		if matches!(error, Error::Unauthorized) {
			Error::Forbidden
		} else {
			error
		}
	})?;
	let result = async {
		// Serialize journals before task row locks so simultaneous shared leases cannot upgrade together.
		if command {
			scope.command_lock(id).await?;
		}
		let current = scope.current_grant(grant.id).await?;
		if !scope.live(current.id).await? {
			return Err(Error::Forbidden);
		}
		scope.replace_subjects(current.subject_chain.clone());
		let task = scope.task_read(current.task_id).await?;
		let locked = scope.locked_task(task.id).await?;
		if locked.revision != task.revision {
			*revision_race = true;
			return Err(Error::Forbidden);
		}
		let binding = scope.binding(current.id).await?;
		let admitted = current
			.admitted_task(&task, binding.as_ref())?
			.ok_or(Error::Forbidden)?;
		let inspection: Inspection = serde_json::from_value(current.inspection)?;
		super::authorize(&mut scope, &task, node, &inspection).await?;
		scope.remote_semantic_sources(current.id).await?;
		if !scope.grant_reads_visible(current.id).await? {
			return Err(Error::Forbidden);
		}
		let agent = EntityRef {
			id: inspection.agent.id.clone(),
			version: inspection.agent.version.clone(),
		};
		let requirements = serde_json::from_value(task.requirements.clone())?;
		let semantic: Binding = serde_json::from_value(current.semantic)?;
		let request = semantic.request();
		let input = inspection_input(&scope, task.id, &agent, requirements, request.compactor());
		let fresh = inspect(repository, &mut scope, node, input).await?;
		if !fresh.satisfies(&inspection) {
			return Err(Error::Forbidden);
		}
		if !scope.live(current.id).await? {
			return Err(Error::Forbidden);
		}
		if scope
			.semantic_binding(&task, node, &fresh, &request)
			.await? != semantic
		{
			return Err(Error::RemoteSemantic(Failure::Configuration));
		}
		Ok(Description {
			grant_id: current.id,
			source_node: repository.source_node_id().into(),
			target_node: current.node_id,
			source_tenant: current.tenant,
			source_subject: current.root_subject,
			task: admitted,
			inspection,
			expires_at: current.expires_at,
			semantic,
		})
	}
	.await;
	match result {
		Ok(description) => Ok((scope, description)),
		Err(error) => {
			scope.finish(Err(error)).await?;
			Err(Error::External(
				"source scope accepted a rejected description".into(),
			))
		}
	}
}

/// Current reader and producer flows share the same enabled-peer protocol requirement.
pub async fn require_peer<S: SourcePeerScope + ?Sized>(
	scope: &mut S,
	node: &str,
	protocol: &str,
) -> Result<()> {
	if scope
		.peer(node)
		.await?
		.is_none_or(|peer| peer.protocol_version != protocol)
	{
		return Err(Error::Forbidden);
	}
	Ok(())
}
