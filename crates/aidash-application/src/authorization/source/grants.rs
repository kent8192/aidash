//! Preparation, description and revocation share the original Source transaction boundaries.
use crate::authorization::peer::tool_request::authorization_details;
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
	federation::execution::{
		Description, Inspection, PrepareInput, Prepared, Refusal,
		admission::InspectInput,
		home::{Grant, PreparationAuthority},
	},
	identity::execution::ExecutionPrincipal,
	nodes::NodeAddress,
	policy::SubjectKind,
	registry::{EntityRef, Search},
	semantic::{Failure, remote::Binding},
};
use serde_json::json;
use uuid::Uuid;

mod evidence;
pub use evidence::{DependencyChecks, GrantPeerCache};
use evidence::{Observed, Revision};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

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
async fn inspection_route<S: GrantScope>(scope: &mut S, node: &str) -> Result<()> {
	let resource = scope.source_resource("node", node, json!({"remote_node":node}));
	scope
		.source_require(&resource, "federation.execute")
		.await?;
	if scope.route(node).await?.is_none() {
		scope.record_refusal(&resource, "node_unreachable").await?;
		return Err(Error::RemoteExecutionRefused(
			aidash_domain::federation::execution::Refusal::NodeUnreachable,
		));
	}
	Ok(())
}

async fn fetch_inspection<R: GrantRepository>(
	repository: &R,
	home: &str,
	node: &str,
	identity: &ExecutionPrincipal,
	input: &InspectInput,
) -> Result<Inspection> {
	let inspection = repository
		.source_request(
			home,
			node,
			"/scoped/execution/inspect",
			&serde_json::to_value(input)?,
			identity,
		)
		.await?;
	crate::federation::admission::validate_inspection(
		&repository.validation(),
		&inspection,
		node,
		&input.agent,
		&input.requirements,
	)?;
	Ok(inspection)
}

fn inspection_unavailable(error: &Error) -> bool {
	matches!(error, Error::External(message) if message == "remote execution authority unavailable")
		|| matches!(error, Error::NodeUnreachable { .. })
}

async fn inspect_new_grant<R: GrantRepository>(
	repository: &R,
	scope: &mut R::Scope,
	node: &str,
	mut input: InspectInput,
) -> Result<Inspection> {
	inspection_route(scope, node).await?;
	input.tool_request = Some(
		scope
			.registered_tool_request(node)
			.await?
			.map(|request| request.tools)
			.unwrap_or_default(),
	);
	match fetch_inspection(
		repository,
		&scope.home_address(),
		node,
		&scope.identity(),
		&input,
	)
	.await
	{
		Err(error) if inspection_unavailable(&error) => {
			let resource = scope.source_resource("node", node, json!({"remote_node":node}));
			scope.record_refusal(&resource, "node_unreachable").await?;
			Err(Error::RemoteExecutionRefused(
				aidash_domain::federation::execution::Refusal::NodeUnreachable,
			))
		}
		other => other,
	}
}
fn inspection_input(task: Uuid, agent: &EntityRef, requirements: Search) -> InspectInput {
	InspectInput {
		tool_request: None,
		task_id: Some(task),
		agent: agent.clone(),
		requirements,
	}
}
async fn require_remote_delegation<S: GrantScope>(
	scope: &mut S,
	thread: Uuid,
	node: &str,
) -> Result<()> {
	if scope.is_subnode_thread(thread).await? {
		let resource = scope.source_resource("node", node, json!({"remote_node":node}));
		scope
			.record_refusal(&resource, "subnode_delegation")
			.await?;
		return Err(Error::RemoteExecutionRefused(Refusal::SubnodeDelegation));
	}
	Ok(())
}

/// Inspect under the same task, subject chain and peer authority as preparation,
/// without creating a grant or disclosing Home memory.
pub async fn agent_inspection<R: GrantRepository>(
	repository: &R,
	task_id: Uuid,
	input: aidash_domain::federation::execution::AgentInspectionInput,
) -> Result<aidash_domain::federation::execution::AgentMemoryRequirements> {
	repository.source_identity().ok_or(Error::Forbidden)?;
	NodeAddress::parse(&input.node_id)?;
	let mut scope = repository.source_begin().await?;
	let result = async {
		scope.inherit_task_origin(task_id).await?;
		let task = scope.task_read(task_id).await?;
		require_remote_delegation(&mut scope, task.thread_id, &input.node_id).await?;
		if task.status != TaskStatus::Open {
			return Err(Error::Conflict("task is already assigned".into()));
		}
		let executor =
			aidash_domain::agent_subject(&input.node_id, &input.agent.id, &input.agent.version);
		if scope
			.source_bundle()
			.subjects
			.get(&executor)
			.is_some_and(|subject| subject.kind != SubjectKind::Agent)
		{
			let resource =
				scope.source_resource("node", &input.node_id, json!({"remote_node":input.node_id}));
			scope
				.record_refusal(&resource, "invalid_executor_subject")
				.await?;
			return Err(Error::RemoteExecutionRefused(
				aidash_domain::federation::execution::Refusal::InvalidExecutor,
			));
		}
		let thread = scope.thread(task.thread_id).await?;
		scope.source_context(thread.attributes.clone());
		let resource = scope.task_resource(&task).await?;
		scope.require(&resource, "task.delegate").await?;
		scope.require(&resource, "task.execute").await?;
		let requirements: Search = serde_json::from_value(task.requirements.clone())?;
		let request = inspection_input(task.id, &input.agent, requirements);
		let inspection = inspect_new_grant(repository, &mut scope, &input.node_id, request).await?;
		scope.append_subject(executor);
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
	if input.id.is_nil() || !(1..=3600).contains(&input.ttl_seconds) {
		return Err(Error::Invalid(
			"invalid remote grant destination or lifetime".into(),
		));
	}
	NodeAddress::parse(&input.node_id)?;
	let mut scope = repository.source_begin().await?;
	let result=async {
  scope.inherit_task_origin(task_id).await?;
  let task=scope.task_read(task_id).await?;
  require_remote_delegation(&mut scope, task.thread_id, &input.node_id).await?;
  if task.status!=TaskStatus::Open {return Err(Error::Conflict("task is already assigned".into()));}
  let executor=aidash_domain::agent_subject(&input.node_id,&input.agent.id,&input.agent.version);
  if scope.source_bundle().subjects.get(&executor).is_some_and(|subject|subject.kind!=SubjectKind::Agent) {let resource=scope.source_resource("node",&input.node_id,json!({"remote_node":input.node_id}));scope.record_refusal(&resource,"invalid_executor_subject").await?;return Err(Error::RemoteExecutionRefused(aidash_domain::federation::execution::Refusal::InvalidExecutor));}
  let thread=scope.thread(task.thread_id).await?;
  scope.source_context(thread.attributes.clone());
  let resource=scope.task_resource(&task).await?;
  scope.require(&resource,"task.delegate").await?;
  scope.require(&resource,"task.execute").await?;
  let requirements:Search=serde_json::from_value(task.requirements.clone())?;
  let request=inspection_input(task.id,&input.agent,requirements);
  let inspection=inspect_new_grant(repository,&mut scope,&input.node_id,request).await?;
  scope.append_subject(executor);
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
  if inserted==1 {scope.event(task.thread_id,"task.remote_grant_prepared",json!({"grant_id":grant.id,"task_id":task_id,"node_id":grant.node_id,"expires_at":grant.expires_at})).await?;}
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
					task.thread_id,
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
/// Revalidate local authority under the grant lease after collecting peer evidence.
pub async fn description<R: GrantRepository>(
	repository: &R,
	node: &str,
	id: Uuid,
) -> Result<(R::Scope, Description)> {
	description_mode(repository, node, id, false).await
}

pub async fn description_mode<R: GrantRepository>(
	repository: &R,
	node: &str,
	id: Uuid,
	command: bool,
) -> Result<(R::Scope, Description)> {
	let cache = repository.peer_cache();
	let deadline = Instant::now() + Duration::from_secs(20);
	for attempt in 0..3 {
		let grant = repository
			.source_grant(id, node)
			.await?
			.ok_or(Error::Forbidden)?;
		let mut fresh = None;
		let mut revision_race = false;
		for _ in 0..(aidash_domain::federation::dependencies::LIMIT + 2) {
			match description_pass(
				repository,
				&grant,
				command,
				&cache,
				fresh.as_ref(),
				&mut revision_race,
			)
			.await
			{
				Ok(Pass::Ready(scope, description)) => return Ok((scope, *description)),
				Ok(Pass::Inspect(request)) => {
					let InspectionRequest {
						revision,
						home,
						identity,
						input,
					} = *request;
					let result = match cache.inspection(&revision) {
						Some(inspection) => inspection,
						None => match fetch_inspection(repository, &home, node, &identity, &input)
							.await
						{
							Ok(value) => {
								let inspected = Observed::new(value);
								cache.remember_inspection(revision.clone(), inspected.clone());
								inspected
							}
							// Another request may have filled the cache while this call was in flight.
							Err(error) if inspection_unavailable(&error) => cache
								.inspection(&revision)
								.ok_or_else(|| unreachable_before_command(node))?,
							Err(error) => return Err(error),
						},
					};
					fresh = Some((revision, result));
				}
				Ok(Pass::Dependency(checks, route, input)) => {
					let remaining = deadline.saturating_duration_since(Instant::now());
					if remaining.is_zero() {
						return Err(unreachable_before_command(input.reference.node()));
					}
					let check = repository.check_dependency(&route, &input, remaining).await;
					let target = route.target.to_string();
					if !checks.remember(*route, input, check) {
						return Err(unreachable_before_command(&target));
					}
				}
				Err(Error::Forbidden) if revision_race && attempt < 2 => break,
				Err(error) => return Err(error),
			}
		}
		if !revision_race {
			return Err(Error::Forbidden);
		}
		tokio::time::sleep(Duration::from_millis(10)).await;
	}
	unreachable!("the last revision attempt returns")
}

fn unreachable_before_command(node: &str) -> Error {
	Error::NodeUnreachable {
		node: node.to_owned(),
		after_effect: false,
	}
}

struct InspectionRequest {
	revision: Revision,
	home: String,
	identity: ExecutionPrincipal,
	input: InspectInput,
}

enum Pass<S> {
	Inspect(Box<InspectionRequest>),
	Dependency(
		DependencyChecks,
		Box<aidash_domain::federation::route::Route>,
		aidash_domain::federation::dependencies::Input,
	),
	Ready(S, Box<Description>),
}

async fn description_pass<R: GrantRepository>(
	repository: &R,
	grant: &Grant,
	command: bool,
	cache: &Arc<GrantPeerCache>,
	fresh: Option<&(Revision, Observed<Inspection>)>,
	revision_race: &mut bool,
) -> Result<Pass<R::Scope>> {
	let node = &grant.node_id;
	let id = grant.id;
	let mut scope = repository.begin_grant(grant).await.map_err(|error| {
		if matches!(error, Error::Unauthorized) {
			Error::Forbidden
		} else {
			error
		}
	})?;
	let mut dependencies = None;
	let result = async {
		// Serialize journals before task row locks so simultaneous shared leases cannot upgrade together.
		if command && fresh.is_some() {
			scope.command_lock(id).await?;
		}
		let current = if fresh.is_some() {
			scope.current_grant(id).await?
		} else {
			grant.clone()
		};
		if !scope.live(current.id).await? {
			return Err(Error::Forbidden);
		}
		if *grant != current {
			*revision_race = true;
			return Err(Error::Forbidden);
		}
		let inspection: Inspection = serde_json::from_value(current.inspection.clone())?;
		let chain = current.subject_chain.clone();
		let executor =
			aidash_domain::agent_subject(node, &inspection.agent.id, &inspection.agent.version);
		// The chain ends with the receiver's executor. An executor this Node's
		// policy does not know is evaluated only by the receiver, so every
		// source check below uses the sending chain (ADR 0109).
		let receiver_only = chain.len() >= 2
			&& chain.last() == Some(&executor)
			&& !scope.source_bundle().subjects.contains_key(&executor);
		let sending = if receiver_only {
			chain[..chain.len() - 1].to_vec()
		} else {
			chain
		};
		scope.replace_subjects(sending.clone());
		// Establish the evidence key before evaluating task provenance, which
		// may itself traverse a receiver dependency.
		let projected = scope
			.control_task(current.task_id)
			.await?
			.ok_or(Error::Forbidden)?;
		if projected.thread_id != current.thread_id {
			return Err(Error::Forbidden);
		}
		scope.thread(projected.thread_id).await?;
		let home = scope.home_address();
		let revision = Revision::new(&current, projected.revision, &home);
		let checks = DependencyChecks::new(cache.clone(), revision.clone());
		scope.set_dependency_checks(Some(checks.clone()));
		dependencies = Some(checks);
		let task = scope.task_read(current.task_id).await?;
		if projected.revision != task.revision
			|| (fresh.is_some() && scope.locked_task(task.id).await?.revision != task.revision)
		{
			*revision_race = true;
			return Err(Error::Forbidden);
		}
		if fresh.is_some_and(|(checked, value)| *checked != revision || !value.current()) {
			*revision_race = true;
			return Err(Error::Forbidden);
		}
		let binding = scope.binding(current.id).await?;
		let admitted = current
			.admitted_task(&task, binding.as_ref())?
			.ok_or(Error::Forbidden)?;
		if receiver_only {
			// `authorize` validates the complete chain and strips the executor itself.
			scope.append_subject(executor.clone());
		}
		super::authorize(&mut scope, &task, node, &inspection).await?;
		scope.replace_subjects(sending.clone());
		scope.remote_semantic_sources(current.id).await?;
		if !scope.grant_reads_visible(current.id).await? {
			return Err(Error::Forbidden);
		}
		inspection_route(&mut scope, node).await?;
		let Some((_, fresh)) = fresh else {
			let agent = EntityRef {
				id: inspection.agent.id.clone(),
				version: inspection.agent.version.clone(),
			};
			let requirements = serde_json::from_value(task.requirements.clone())?;
			return Ok(Pass::Inspect(Box::new(InspectionRequest {
				revision,
				home,
				identity: scope.identity(),
				input: inspection_input(task.id, &agent, requirements),
			})));
		};
		if !fresh.current() {
			*revision_race = true;
			return Err(Error::Forbidden);
		}
		let fresh = &fresh.value;
		let semantic: Binding = serde_json::from_value(current.semantic)?;
		let request = semantic.request();
		if !fresh.satisfies(&inspection) {
			return Err(Error::Forbidden);
		}
		if !scope.live(current.id).await? {
			return Err(Error::Forbidden);
		}
		if receiver_only {
			scope.append_subject(executor);
		}
		let rebound = scope.semantic_binding(&task, node, fresh, &request).await;
		scope.replace_subjects(sending);
		if rebound? != semantic {
			return Err(Error::RemoteSemantic(Failure::Configuration));
		}
		// Read now, on every describe, so a list change reaches the receiver at its
		// next describe (ADR 0139).
		let request = match scope.registered_tool_request(&current.node_id).await? {
			// Tools the Home's people allowed for this Run only travel inside the Request, so
			// the receiver reads one list.
			Some(request) => {
				let run_only = scope.run_only_tool_request(current.id).await?;
				Some(request.with_run_tools(&run_only)?)
			}
			None => None,
		};
		let authorization_details = authorization_details(request.as_ref());
		Ok(Pass::Ready(
			(),
			Box::new(Description {
				grant_id: current.id,
				source_node: scope.home_address(),
				target_node: current.node_id,
				source_tenant: current.tenant,
				source_subject: current.root_subject,
				task: admitted,
				inspection,
				expires_at: current.expires_at,
				semantic,
				authorization_details,
			}),
		))
	}
	.await;
	if let Some(checks) = dependencies
		&& let Some((route, input)) = checks.take_missing()
	{
		scope.release_probe().await?;
		return Ok(Pass::Dependency(checks, Box::new(route), input));
	}
	match result {
		Ok(Pass::Ready((), description)) => {
			scope.set_dependency_checks(None);
			Ok(Pass::Ready(scope, description))
		}
		Ok(Pass::Inspect(request)) => {
			scope.release_probe().await?;
			Ok(Pass::Inspect(request))
		}
		Ok(Pass::Dependency(checks, route, input)) => Ok(Pass::Dependency(checks, route, input)),
		Err(error) => {
			scope.finish(Err(error)).await?;
			Err(Error::External(
				"source scope accepted a rejected description".into(),
			))
		}
	}
}

/// Current reader and producer flows share the same enabled-route requirement.
pub async fn require_peer<S: SourcePeerScope + ?Sized>(scope: &mut S, node: &str) -> Result<()> {
	if scope.route(node).await?.is_none() {
		return Err(Error::Forbidden);
	}
	Ok(())
}
