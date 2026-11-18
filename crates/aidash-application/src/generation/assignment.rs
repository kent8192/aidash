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
	approval_rules::{
		APPROVAL_ADVANCE, ApprovalKind, MODE_RESOURCE, RefusedBy, Resolution, Standing,
		TenantStance, resolve,
	},
	federation::Delegation,
	generation::{
		intent::Intent,
		policy::Policy,
		requests::{Assignment, resource_attributes},
	},
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
	assign_in_restricted(scope, task_id, policy_id, reason, validation, None).await
}

/// A Subnode may reuse only its current members and may not generate agents.
pub async fn assign_in_restricted(
	scope: &mut dyn GenerationAssignmentScope,
	task_id: Uuid,
	policy_id: &str,
	reason: &str,
	validation: &DefinitionValidation,
	members: Option<&[EntityRef]>,
) -> Result<Assignment> {
	assign_in_drafted(scope, task_id, policy_id, reason, validation, members, None).await
}

/// [`assign_in_restricted`] with what an Agent wrote for a free-form generated
/// agent (#1012, ADR 0143). A draft needs a policy in free-form mode.
pub async fn assign_in_drafted(
	scope: &mut dyn GenerationAssignmentScope,
	task_id: Uuid,
	policy_id: &str,
	reason: &str,
	validation: &DefinitionValidation,
	members: Option<&[EntityRef]>,
	draft: Option<&aidash_domain::generation::policy::Draft>,
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
			if members.is_some_and(|members| {
				!members
					.iter()
					.any(|member| member.id == id && member.version == version)
			}) {
				return Err(Error::Forbidden);
			}
			let node = scope.workspace_node(task.workspace_id).await?;
			let mut expected = scope.subjects().to_vec();
			expected.push(qualified_agent(&node, &id, &version));
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
						node_id: node.to_string(),
						agent_id: id,
						agent_version: version,
					},
				});
			}
		}
		return Err(Error::Conflict("task is already assigned".into()));
	}
	let mut search: Search = serde_json::from_value(task.requirements.clone())?;
	search.kind = Some("agent".into());
	let node = scope.workspace_node(task.workspace_id).await?;
	for entry in scope.catalog(&search).await? {
		if members.is_some_and(|members| {
			!members
				.iter()
				.any(|member| member.id == entry.id && member.version == entry.version)
		}) {
			continue;
		}
		if scope.generated(&entry).await? {
			continue;
		}
		let subject = qualified_agent(&node, &entry.id, &entry.version);
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
	// A Subnode generates only once the generated agent is checked against the
	// Subnode's allowance and joins it as a member; until then its Tasks go to
	// existing members only, so the allowance cannot be escaped.
	if members.is_some() {
		return Err(Error::Refused("no Subnode member matches this task".into()));
	}
	create_in_drafted(scope, &task, policy, reason, None, validation, draft).await
}
/// What the person the chain starts at decided in advance about a generation
/// request that needs an approver (ADR 0076). Only a refusal is acted on: an
/// advance approval never stands in for the person's Catalog Approval of the
/// generated definition (ADR 0143).
async fn advise(
	scope: &mut dyn GenerationCreationScope,
	task: &Task,
	policy: &Policy,
	definition: &Entry,
	id: Uuid,
) -> Result<Resolution> {
	let Some(settings) = scope.approval_settings().await? else {
		return Ok(Resolution::Ask);
	};
	let request = id.to_string();
	let attributes = resource_attributes(&policy.id, scope.subject(), task.id);
	let catalog = format!(
		"{}/{}@{}",
		definition.kind, definition.id, definition.version
	);
	let approver = scope
		.root_allows(
			"generation",
			&request,
			attributes.clone(),
			"generation.approve",
		)
		.await?
		&& scope
			.root_allows(
				"registry",
				&catalog,
				crate::authorization::catalog::attributes(definition),
				"catalog.approve",
			)
			.await?;
	let advance = scope
		.root_allows("generation", &request, attributes, APPROVAL_ADVANCE)
		.await?;
	let mode = match settings.mode.policy_requirement() {
		None => true,
		Some((action, mode)) => {
			scope
				.root_allows(MODE_RESOURCE, mode, json!({}), action)
				.await?
		}
	};
	let ceiling = settings.ceiling(Standing {
		approver,
		advance,
		mode,
	});
	Ok(resolve(
		TenantStance::NeedsApproval,
		Some(&settings),
		ceiling,
		ApprovalKind::Generation,
		&policy.id,
		false,
	))
}
/// A generation request creates no record when a rule or mode refuses it:
/// there is nothing to approve and no quota to reserve.
fn refusal(by: RefusedBy) -> Error {
	tracing::info!(refused_by = %by.data(), "generation request refused in advance");
	Error::Forbidden
}
pub async fn create_in(
	scope: &mut dyn GenerationCreationScope,
	task: &Task,
	policy: Policy,
	reason: &str,
	foreign: Option<&Intent>,
	validation: &DefinitionValidation,
) -> Result<Assignment> {
	create_in_drafted(scope, task, policy, reason, foreign, validation, None).await
}

/// The largest instructions a draft may carry, as for a Skill body.
const MAX_DRAFT_INSTRUCTIONS_BYTES: usize = 65_536;

/// The template with what the draft names: instructions, name, model and the
/// tool and Skill bindings. Every pick must be one the policy's free-form mode
/// allows; whether each is Catalog-approved is checked with the rest of the
/// definition's closure.
async fn drafted(
	scope: &mut dyn GenerationCreationScope,
	policy: &Policy,
	draft: &aidash_domain::generation::policy::Draft,
) -> Result<Entry> {
	let free =
		policy.spec.free_form.as_ref().ok_or_else(|| {
			Error::Invalid("this generation policy does not accept a draft".into())
		})?;
	aidash_domain::nonempty(&draft.instructions, "draft instructions")?;
	if draft.instructions.len() > MAX_DRAFT_INSTRUCTIONS_BYTES || draft.instructions.contains('\0')
	{
		return Err(Error::Invalid(
			"draft instructions are at most 65536 bytes without NUL".into(),
		));
	}
	let mut template = policy.spec.template.clone();
	template.config["instructions"] = json!(draft.instructions);
	if let Some(name) = draft
		.name
		.as_deref()
		.map(str::trim)
		.filter(|name| !name.is_empty())
	{
		template.name = [("en".to_owned(), name.to_owned())].into();
	}
	if let Some(model) = &draft.model {
		if !free.allows(model) {
			return Err(Error::Forbidden);
		}
		template.config["model"] = json!({"id": model.id, "version": model.version});
	}
	if !draft.components.is_empty() {
		let mut bindings = vec![];
		for component in &draft.components {
			if !free.allows(component) {
				return Err(Error::Forbidden);
			}
			let entry = scope.catalog_entry(component, "registry.read").await?;
			if !matches!(entry.kind.as_str(), "tool" | "skill") {
				return Err(Error::Invalid(format!(
					"{}@{} is not a tool or Skill",
					component.id, component.version
				)));
			}
			bindings.push(json!({
				"kind": entry.kind,
				"target": {
					"registry_deployment": scope.node_id(),
					"id": component.id,
					"version": component.version,
				},
				"narrow": {},
			}));
		}
		template.config["bindings"] = json!(bindings);
	}
	Ok(template)
}

pub async fn create_in_drafted(
	scope: &mut dyn GenerationCreationScope,
	task: &Task,
	policy: Policy,
	reason: &str,
	foreign: Option<&Intent>,
	validation: &DefinitionValidation,
	draft: Option<&aidash_domain::generation::policy::Draft>,
) -> Result<Assignment> {
	let search: Search = serde_json::from_value(task.requirements.clone())?;
	if !policy.spec.enabled {
		return Err(Error::Forbidden);
	}
	let _config = super::policy::validate(validation, &policy.spec, scope.bundle())?;
	let template = match draft {
		Some(draft) => drafted(scope, &policy, draft).await?,
		None => policy.spec.template.clone(),
	};

	let snapshot = scope.bindings(&template).await?;
	super::policy::validate_template_snapshot(&snapshot)?;
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
	let allowances = policy.reservation(active, depth)?;
	let id = scope.request_id();
	let mut definition = template;
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
	// A request that arrived through Federation is never covered by anyone's rules.
	let resolution = if policy.spec.approval_required && foreign.is_none() {
		advise(scope, task, &policy, &definition, id).await?
	} else {
		Resolution::Ask
	};
	// A person's own deny rule still refuses, but neither an Approval Mode
	// (Bypass included) nor an allow rule approves a generated definition: its
	// Catalog Approval is a Tenant approval that a person makes, never the
	// chain (#1012, ADR 0143). Only the policy's no-approval mark, set by a
	// person holding `generation.manage`, skips it.
	let given: Option<_> = match resolution {
		Resolution::Refuse(by) => return Err(refusal(by)),
		Resolution::Approve(_) | Resolution::Ask | Resolution::Judge | Resolution::Proceed => None,
	};
	let status = if !policy.spec.approval_required || given.is_some() {
		"QUEUED"
	} else {
		"PENDING_APPROVAL"
	};
	let lifetime_seconds = foreign.map_or(policy.spec.limits.lifetime(), |intent| {
		(intent.expires_at - scope.now())
			.num_seconds()
			.min(policy.spec.limits.lifetime())
	}) as f64;
	let generated = scope
		.insert(&Creation {
			id,
			task,
			policy: &policy,
			definition: &definition,
			status,
			given_by: given,
			reason,
			depth,
			lifetime_seconds,
			foreign_intent: foreign.map(serde_json::to_value).transpose()?,
		})
		.await?;
	scope.allocate(&policy, &allowances).await?;
	scope.budget(id, &policy, &allowances).await?;
	match given {
		// The request is recorded as asked, then as approved by the person whose
		// rule or mode gave the approval.
		Some(by) => {
			scope.history(id, "PENDING_APPROVAL", reason).await?;
			scope
				.history(id, status, &format!("approved in advance through {by}"))
				.await?;
		}
		None => scope.history(id, status, reason).await?,
	}
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
