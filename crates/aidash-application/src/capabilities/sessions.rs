//! Admission, disclosure and initialization share one transaction across HTTP and workers.
use crate::{Error, Result, ports::capabilities::sessions::SessionScope};
use aidash_domain::{
	RunMetadata, Task,
	capabilities::{
		operations::available,
		sessions::{Area, SessionStatus, received_scope_matches, reusable_context},
	},
	registry::AgentConfig,
};
use serde_json::{Value, json};
use uuid::Uuid;
pub async fn bind_task(scope: &mut dyn SessionScope, task: Uuid, thread: Uuid) -> Result<()> {
	scope.bind_task(task, thread).await
}
pub async fn load(scope: &mut dyn SessionScope, id: Uuid) -> Result<Area> {
	let area = scope
		.load(id)
		.await?
		.ok_or_else(|| Error::NotFound("working area unavailable".into()))?;
	authorize(scope, &area, "file.read").await?;
	Ok(area)
}
pub async fn authorize(scope: &mut dyn SessionScope, area: &Area, action: &str) -> Result<()> {
	let workspace = scope.workspace(area.workspace_id).await?;
	scope.set_context(workspace.attributes.clone());
	if area.tenant != scope.tenant() || area.owner != scope.principal() {
		return Err(Error::NotFound("working area unavailable".into()));
	}
	let resource = scope.resource(
		"working_area",
		area.id,
		json!({"owner":area.owner,"agent_id":area.agent_id,"thread_id":area.thread_id}),
	);
	scope.require(&workspace, "workspace.read").await?;
	scope
		.require(&resource, action)
		.await
		.map_err(|_| Error::NotFound("working area unavailable".into()))?;
	authorize_sources(scope, area.workspace_id, &area.constraints).await
}
pub async fn authorize_sources(
	scope: &mut dyn SessionScope,
	workspace: Uuid,
	constraints: &Value,
) -> Result<()> {
	for source in constraints
		.as_array()
		.ok_or_else(|| Error::Conflict("invalid area constraints".into()))?
	{
		match source["kind"].as_str() {
			Some("message") => {
				scope
					.message(workspace, serde_json::from_value(source["id"].clone())?)
					.await?
			}
			Some("reference_text") => {
				scope
					.agent(&serde_json::from_value(source["agent"].clone())?)
					.await?
			}
			Some("reference") => {
				scope
					.reference(serde_json::from_value(source["id"].clone())?)
					.await?
			}
			Some("received_scope") => {
				let owner = source["owner"].as_str().ok_or(Error::Forbidden)?;
				let agent = source["agent"].as_str().ok_or(Error::Forbidden)?;
				if !received_scope_matches(scope.subjects(), owner, agent) {
					return Err(Error::Forbidden);
				}
			}
			_ => return Err(Error::Forbidden),
		}
	}
	Ok(())
}
pub async fn for_run(scope: &mut dyn SessionScope, run: &RunMetadata) -> Result<Area> {
	let (id, generation) = scope
		.run_binding(run.id)
		.await?
		.ok_or_else(|| Error::NotFound("working area unavailable".into()))?;
	let area = load(scope, id).await?;
	if area.agent_id != run.agent_id
		|| area.generation != generation
		|| area.workspace_id != run.workspace_id
		|| area.home_node != run.home_node
	{
		return Err(Error::Forbidden);
	}
	Ok(area)
}
pub async fn require_current_run(
	scope: &mut dyn SessionScope,
	area: &Area,
	run: &RunMetadata,
) -> Result<()> {
	if scope.current_run(area).await? == Some(run.id) {
		Ok(())
	} else {
		Err(Error::NotFound("working area unavailable".into()))
	}
}
pub async fn context_authority(scope: &mut dyn SessionScope, run: &RunMetadata) -> Result<()> {
	let area = scope
		.context_area(run.id)
		.await?
		.ok_or_else(|| Error::NotFound("working area unavailable".into()))?;
	authorize(scope, &area, "file.read").await?;
	if !reusable_context(&area.state) {
		return Err(Error::Conflict(
			"AREA_UNAVAILABLE: retained context cannot be reused".into(),
		));
	}
	Ok(())
}
pub async fn prepare_admission(
	scope: &mut dyn SessionScope,
	task: &Task,
	config: &AgentConfig,
	agent_id: &str,
) -> Result<Option<Uuid>> {
	if !config.core_capabilities.enabled() {
		return Ok(None);
	}
	if !scope.admission()? {
		return Err(Error::Conflict("CAPABILITIES_DISABLED: enable the operator execution profile before admitting core work".into()));
	}
	let explicit = scope.explicit_thread(task.id).await?;
	let mut thread = explicit.unwrap_or(task.id);
	let mut ancestor = task.parent_id;
	for _ in 0..64 {
		let Some(id) = ancestor else {
			break;
		};
		let parent = scope.task(id).await?;
		for parent_area in scope.parent_areas(id).await? {
			if parent_area.agent_id == agent_id && parent_area.owner == scope.principal() {
				return Err(Error::Conflict(
					"SESSION_DEPENDENCY_CYCLE: child would wait behind its parent".into(),
				));
			}
			if explicit.is_none() {
				thread = parent_area.thread_id;
			}
		}
		ancestor = parent.parent_id;
	}
	if ancestor.is_some() {
		return Err(Error::Conflict(
			"SESSION_DEPENDENCY_CYCLE: ancestor limit exceeded".into(),
		));
	}
	scope.lock_thread(thread, task.workspace_id).await?;
	Ok(Some(thread))
}
pub async fn admit(
	scope: &mut dyn SessionScope,
	task: &Task,
	run_id: Uuid,
	thread: Option<Uuid>,
	config: &AgentConfig,
	agent_id: &str,
) -> Result<()> {
	let Some(thread) = thread else {
		return Ok(());
	};
	let mut area = scope.ensure_area(task, thread, agent_id).await?;
	authorize(scope, &area, "file.read").await?;
	if !matches!(area.state.as_str(), "active" | "running") {
		return Err(Error::Conflict(
			"AREA_UNAVAILABLE: restore or recreate the area before starting work".into(),
		));
	}
	let queue = status(scope, &area).await?.queue;
	if queue.len() >= 100 {
		return Err(Error::Conflict("QUEUE_LIMIT".into()));
	}
	let initialized = area.state == "active" && queue.is_empty();
	if initialized {
		scope.pin(run_id, &mut area, config).await?;
	}
	scope.enqueue(run_id, &area, initialized).await?;
	scope
		.event(
			task.workspace_id,
			"capability.queued",
			json!({"area_id":area.id,"run_id":run_id,"sequence":area.next_sequence}),
		)
		.await?;
	Ok(())
}
pub async fn initialize(
	scope: &mut dyn SessionScope,
	run: &RunMetadata,
	config: &AgentConfig,
) -> Result<()> {
	let initialized = scope
		.initialized(run.id)
		.await?
		.ok_or_else(|| Error::NotFound("working area unavailable".into()))?;
	if initialized {
		return Ok(());
	}
	let mut area = for_run(scope, run).await?;
	if status(scope, &area).await?.active_run_id != Some(run.id) {
		return Err(Error::Conflict(
			"RUN_QUEUED: wait for the preceding Run".into(),
		));
	}
	available(&area.state)?;
	// Recheck after acquiring the area lock so simultaneous callers pin only once.
	if scope.initialized_locked(run.id).await? {
		return Ok(());
	}
	scope.pin(run.id, &mut area, config).await?;
	scope.mark_initialized(run.id).await
}
pub async fn status(scope: &mut dyn SessionScope, area: &Area) -> Result<SessionStatus> {
	let rows = scope.queue(area).await?;
	if rows.len() > 100 {
		return Err(Error::Conflict("QUEUE_LIMIT".into()));
	}
	let last = scope.last(area).await?;
	Ok(SessionStatus {
		area_id: area.id,
		last_run_id: last.as_ref().map(|r| r.0),
		last_agent_version: last.map(|r| r.1),
		active_run_id: rows.first().map(|r| r.run_id),
		queue: rows,
	})
}
pub async fn cached(
	scope: &mut dyn SessionScope,
	key: Uuid,
	digest: &str,
) -> Result<Option<Value>> {
	match scope.previous(key).await? {
		Some((old, _)) if old != digest => Err(Error::Conflict("IDEMPOTENCY_CONFLICT".into())),
		Some((_, result)) => Ok(Some(result)),
		None => Ok(None),
	}
}
pub async fn cache(
	scope: &mut dyn SessionScope,
	key: Uuid,
	digest: &str,
	result: &Value,
) -> Result<()> {
	scope.cache(key, digest, result).await
}
