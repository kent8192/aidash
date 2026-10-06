//! Publication authority and worker live checks are shared application decisions.
use crate::{
	Error, Result,
	ports::generation::publication::{GenerationLive, GenerationPublication},
};
use aidash_domain::{
	generation::{policy::Spec, requests::Request},
	policy::{Subject, SubjectKind},
	qualified_agent,
	registry::{EntityRef, Entry},
};
use uuid::Uuid;
pub async fn publish(
	scope: &mut dyn GenerationPublication,
	job: &Request,
	spec: &Spec,
) -> Result<()> {
	let entry: Entry = serde_json::from_value(job.definition.clone())?;
	let subject = qualified_agent(scope.node_id(), &entry.id, &entry.version);
	let mut snapshot = scope.snapshot().clone();
	if snapshot.bundle.subjects.contains_key(&subject) {
		return Err(Error::Conflict("generated subject already exists".into()));
	}
	snapshot.bundle.subjects.insert(
		subject,
		Subject {
			kind: SubjectKind::Agent,
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
	scope.approve(job, &entry).await?;
	scope.catalog_history(job, &entry).await
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
				&& (job.task_id != task || !job.home_node.is_empty()))
		{
			return Err(Error::Forbidden);
		}
	}
	Ok(())
}
#[cfg(test)]
mod tests;
