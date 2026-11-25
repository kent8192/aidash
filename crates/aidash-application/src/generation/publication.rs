//! Publication authority and worker live checks are shared application decisions.
use crate::{
	Error, Result,
	ports::generation::publication::{GenerationLive, GenerationPublication},
};
use aidash_domain::{
	generation::{policy::Spec, requests::Request},
	policy::{Subject, SubjectKind},
	registry::{EntityRef, Entry},
};
use uuid::Uuid;
pub async fn publish(
	scope: &mut dyn GenerationPublication,
	job: &Request,
	spec: &Spec,
) -> Result<()> {
	let entry: Entry = serde_json::from_value(job.definition.clone())?;
	let subject = aidash_domain::agent_subject(
		&format!("{}/{}", scope.node_id(), job.node),
		&entry.id,
		&entry.version,
	);
	let mut snapshot = scope.snapshot().clone();
	if snapshot.bundle.subjects.contains_key(&subject) {
		return Err(Error::Conflict("generated subject already exists".into()));
	}
	snapshot.bundle.subjects.insert(
		subject,
		Subject {
			kind: SubjectKind::Agent,
			node: Some(job.node.clone()),
			roles: spec.permissions.roles.clone(),
			groups: spec.permissions.groups.clone(),
			attributes: spec.permissions.attributes.clone(),
			enabled: true,
			delegated_by: job.subject_chain.last().cloned(),
		},
	);
	snapshot.bundle.validate()?;
	snapshot.revision = snapshot
		.revision
		.checked_add(1)
		.ok_or_else(|| Error::Invalid("authorization revision exhausted".into()))?;
	scope.replace_snapshot(snapshot);
	scope.save_authority(job).await?;
	scope.register(&entry).await?;
	let actor = if spec.approval_required {
		let approver = scope.approver(job).await?.ok_or(Error::Forbidden)?;
		if !scope.may_approve(&approver, &entry).await? {
			return Err(Error::Forbidden);
		}
		approver
	} else {
		// The pinned policy authorizes publication in advance. The requester
		// did not make an individual Catalog Approval decision.
		"generation-service".to_owned()
	};
	scope.approve(job, &entry).await?;
	scope.catalog_history(job, &entry, &actor).await
}
/// Whether the generated agent of `job` may work on `task` in a chain whose
/// root is `root`: on the Task it was generated for, or, for a local request,
/// on any Task of a chain of the same root person, Duties included (#1012
/// decision 49). Anyone else needs a person's Catalog Approval of their own.
pub fn reusable_by(job: &Request, task: Uuid, root: Option<&str>) -> bool {
	job.is_local() && (job.task_id == task || root == Some(job.root_subject.as_str()))
}

/// Every ancestor is checked using the current policy under the inherited lease.
pub async fn require_live(
	scope: &mut dyn GenerationLive,
	node: &str,
	task: Uuid,
	agent: &EntityRef,
) -> Result<()> {
	let jobs = scope.jobs(node, agent).await?;
	for job in jobs {
		let enabled = scope.policy_enabled(&job).await?;
		if job.tenant != scope.tenant()
			|| job.status != "ACTIVE"
			|| job.expires_at <= scope.now()
			|| !enabled
			|| (job.agent_id == agent.id
				&& job.agent_version == agent.version
				&& !reusable_by(&job, task, scope.root_subject()))
		{
			return Err(Error::Forbidden);
		}
	}
	Ok(())
}
#[cfg(test)]
mod tests;
