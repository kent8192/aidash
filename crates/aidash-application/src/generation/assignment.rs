//! Agent reuse, authorization narrowing and generation reservation use one scope.
use crate::{
	Error, Result,
	ports::generation::assignment::{
		Creation, GenerationAssignmentScope, GenerationAssignments, GenerationCreationScope,
	},
	registry::DefinitionValidation,
};
use aidash_domain::{
	Task, TaskStatus,
	federation::Delegation,
	generation::{intent::Intent, policy::Policy, requests::Assignment},
	policy::{Resource, SubjectKind},
	qualified_agent,
	registry::{EntityRef, Entry, Search},
};
use serde_json::json;
use uuid::Uuid;

pub async fn assign(
	repository: &dyn GenerationAssignments,
	task_id: Uuid,
	policy_id: &str,
	reason: &str,
	validation: &DefinitionValidation,
) -> Result<Assignment> {
	let mut scope = repository.begin().await?;
	let result = assign_in(scope.as_mut(), task_id, policy_id, reason, validation).await;
	let assignment = scope.finish(result).await?;
	repository.notify();
	Ok(assignment)
}
async fn require(
	scope: &mut dyn GenerationAssignmentScope,
	resource: &Resource,
	action: &str,
) -> Result<()> {
	if scope.decide(resource, action).await? {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}
struct CandidateSubjects<'a> {
	scope: &'a mut dyn GenerationAssignmentScope,
	original: Vec<String>,
}
impl Drop for CandidateSubjects<'_> {
	fn drop(&mut self) {
		self.scope
			.replace_subjects(std::mem::take(&mut self.original));
	}
}
async fn candidate_allowed(
	scope: &mut dyn GenerationAssignmentScope,
	subject: String,
	workspace: &Resource,
	entry: &Entry,
	task: &Resource,
) -> Result<bool> {
	let original = scope.subjects().to_vec();
	let mut narrowed = original.clone();
	narrowed.push(subject);
	scope.replace_subjects(narrowed);
	// The borrowed worker scope must restore its original chain on error and
	// cancellation as well as success. Durable decisions remain in its audit.
	let guard = CandidateSubjects { scope, original };
	let catalog=guard.scope.resource(&entry.kind,&entry.id,json!({"version":entry.version,"capabilities":entry.capabilities,"tags":entry.tags,"languages":entry.languages,"config":entry.config}));
	Ok(guard.scope.decide(workspace, "workspace.read").await?
		&& guard.scope.decide(&catalog, "agent.execute").await?
		&& guard.scope.decide(task, "task.read").await?
		&& guard.scope.decide(task, "task.execute").await?)
}
pub async fn assign_in(
	scope: &mut dyn GenerationAssignmentScope,
	task_id: Uuid,
	policy_id: &str,
	reason: &str,
	validation: &DefinitionValidation,
) -> Result<Assignment> {
	aidash_domain::nonempty(reason, "generation reason")?;
	if reason.len() > 4096 {
		return Err(Error::Invalid(
			"generation reason exceeds 4096 bytes".into(),
		));
	}
	let task = scope.task(task_id).await?;
	let workspace = scope.workspace(task.workspace_id).await?;
	scope.context(workspace.attributes.clone());
	if !scope.inherit_task_origin(task_id).await?
		&& (task.created_by != scope.subject() || scope.subjects().len() > 1)
	{
		return Err(Error::Forbidden);
	}
	require(scope, &workspace, "workspace.read").await?;
	let task_resource = scope.task_resource(&task).await?;
	require(scope, &task_resource, "task.read").await?;
	require(scope, &task_resource, "task.delegate").await?;
	let resource = scope.resource("generation_policy", policy_id, json!({}));
	require(scope, &resource, "generation.request").await?;
	require(scope, &resource, "generation.read").await?;
	// The exclusive policy lock serializes quota reservations and task retries.
	let policy = scope.policy(policy_id).await?;
	if let Some(existing) = scope.existing(task_id).await? {
		if existing.tenant != scope.tenant()
			|| existing.root_subject != scope.subject()
			|| existing.policy_id != policy_id
			|| existing.reason != reason
			|| existing.subject_chain != scope.subjects()
		{
			return Err(Error::Conflict(
				"task already has a different generation request".into(),
			));
		}
		if !scope.visible(&existing).await? {
			return Err(Error::Forbidden);
		}
		return Ok(Assignment::Generated {
			generation: Box::new(existing),
		});
	}
	if task.status != TaskStatus::Open {
		if let Some((id, version, chain)) = scope.existing_run(task_id).await? {
			let mut expected = scope.subjects().to_vec();
			expected.push(qualified_agent(scope.node_id(), &id, &version));
			if chain == expected {
				scope.replace_subjects(chain);
				scope
					.catalog_entry(
						&EntityRef {
							id: id.clone(),
							version: version.clone(),
						},
						"agent.execute",
					)
					.await?;
				return Ok(Assignment::Existing {
					delegation: Delegation {
						task_id,
						node_id: scope.node_id().to_owned(),
						agent_id: id,
						agent_version: version,
						delivered: true,
					},
				});
			}
		}
		return Err(Error::Conflict("task is already assigned".into()));
	}
	let mut search: Search = serde_json::from_value(task.requirements.clone())?;
	search.kind = Some("agent".into());
	for entry in scope.catalog(&search).await? {
		if scope.generated(&entry).await? {
			continue;
		}
		let subject = qualified_agent(scope.node_id(), &entry.id, &entry.version);
		if scope
			.bundle()
			.subjects
			.get(&subject)
			.is_none_or(|s| !s.enabled || s.kind != SubjectKind::Agent)
		{
			continue;
		}
		if candidate_allowed(scope, subject, &workspace, &entry, &task_resource).await? {
			let delegation = scope
				.delegate(
					task_id,
					&EntityRef {
						id: entry.id,
						version: entry.version,
					},
				)
				.await?;
			return Ok(Assignment::Existing { delegation });
		}
	}
	create_in(scope, &task, policy, reason, None, validation).await
}
pub async fn create_in(
	scope: &mut dyn GenerationCreationScope,
	task: &Task,
	policy: Policy,
	reason: &str,
	foreign: Option<&Intent>,
	validation: &DefinitionValidation,
) -> Result<Assignment> {
	let search: Search = serde_json::from_value(task.requirements.clone())?;
	if !policy.spec.enabled {
		return Err(Error::Forbidden);
	}
	let _config = super::policy::validate(validation, &policy.spec, scope.bundle())?;

	let snapshot = scope.bindings(&policy.spec.template).await?;
	for (reference, action) in snapshot
		.definitions
		.iter()
		.filter(|d| d.identity != snapshot.agent)
		.map(|d| {
			(
				d.identity.local(),
				crate::registry::bindings::component_action(&d.definition.kind),
			)
		})
		.chain(
			policy
				.spec
				.compaction
				.iter()
				.map(|c| (c.provider.clone(), "compaction.invoke")),
		)
		.chain(
			policy
				.spec
				.embedding
				.iter()
				.map(|c| (c.provider.clone(), "embedding.invoke")),
		) {
		scope.catalog_entry(&reference, "registry.read").await?;
		scope.catalog_entry(&reference, action).await?;
	}
	let previous_depth = scope.previous_depth().await?;
	let depth = previous_depth.unwrap_or(0).max(foreign.map_or(0, |intent| {
		intent.lineage.iter().map(|a| a.depth).max().unwrap_or(0)
	})) + 1;
	let active = scope.active(&policy.id).await?;
	let allowances = policy.reservation(active, depth, scope.subjects().len())?;
	let id = scope.request_id();
	let mut definition = policy.spec.template.clone();
	definition.id = format!("generated-{}", id.simple());
	definition.binding_normalization = None;
	if !definition.tags.iter().any(|t| t == "generated") {
		definition.tags.push("generated".into());
	}
	definition.normalize_agent(scope.node_id())?;
	validation.validate_in(&definition, true)?;
	if !search.matches(&definition) {
		return Err(Error::Invalid(
			"generation template does not satisfy task requirements".into(),
		));
	}
	let status = if policy.spec.approval_required {
		"PENDING_APPROVAL"
	} else {
		"QUEUED"
	};
	let lifetime_seconds = foreign.map_or(policy.spec.limits.lifetime_seconds, |intent| {
		(intent.expires_at - scope.now())
			.num_seconds()
			.min(policy.spec.limits.lifetime_seconds)
	}) as f64;
	let generated = scope
		.insert(&Creation {
			id,
			task,
			policy: &policy,
			definition: &definition,
			status,
			reason,
			depth,
			lifetime_seconds,
			home_node: foreign.map_or("", |intent| intent.home_node.as_str()),
			foreign_intent: foreign.map(serde_json::to_value).transpose()?,
		})
		.await?;
	scope
		.allocate(
			&policy,
			allowances.compaction_calls,
			allowances.embedding_calls,
		)
		.await?;
	scope
		.budget(
			id,
			&policy,
			allowances.compaction_calls,
			allowances.embedding_calls,
		)
		.await?;
	scope.history(id, status, reason).await?;
	if foreign.is_none() {
		scope
			.event(
				task.workspace_id,
				"generation.requested",
				json!({"id":id,"task_id":task.id,"policy_id":policy.id,"status":status}),
			)
			.await?;
	}
	if !scope.visible(&generated).await? {
		return Err(Error::Forbidden);
	}
	Ok(Assignment::Generated {
		generation: Box::new(generated),
	})
}
#[cfg(test)]
mod tests;
