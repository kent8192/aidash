//! Foreign preparation revalidates Home identity and current receiver policy on every retry.
use crate::{Error, Result, ports::generation::foreign::receiver::ForeignGenerationReceiver};
use aidash_domain::{
	generation::{intent::Prepared, requests::Assignment},
	registry::EntityRef,
};
use serde_json::json;
use uuid::Uuid;

pub async fn prepare(
	repository: &dyn ForeignGenerationReceiver,
	source: &str,
	id: Uuid,
) -> Result<Prepared> {
	let intent = repository.describe(source, id).await?;
	if intent.id != id
		|| intent.home_node != source
		|| intent.target_node != repository.node_id()
		|| intent.expires_at <= repository.now()
	{
		return Err(Error::Forbidden);
	}
	let mut scope = repository
		.begin(source, &intent.source_tenant, &intent.source_subject)
		.await?;
	let result = async {
		scope
			.require(
				&scope.resource("node", repository.node_id(), json!({})),
				"federation.execute",
			)
			.await?;
		scope
			.require(
				&scope.resource(
					"task",
					&format!("{source}/tasks/{}", intent.task.id),
					json!({}),
				),
				"task.read",
			)
			.await?;
		scope
			.require(
				&scope.resource("generation_policy", &intent.policy_id, json!({})),
				"generation.request",
			)
			.await?;
		scope
			.require(
				&scope.resource("generation_policy", &intent.policy_id, json!({})),
				"generation.read",
			)
			.await?;
		let policy = scope.policy(&intent.policy_id).await?;
		if policy.revision != intent.policy_revision || !policy.spec.enabled {
			return Err(Error::Conflict("generation policy revision changed".into()));
		}
		let spec = policy.spec.clone();
		let mut job = if let Some(job) = scope.existing(source, intent.task.id).await? {
			let authority = scope.authority();
			if job.foreign_intent != Some(json!(intent))
				|| job.credential_id != authority.credential_id
				|| job.subject_chain != authority.subjects
				|| job.tenant != authority.tenant
			{
				return Err(Error::Conflict(
					"foreign generation already has a different binding".into(),
				));
			}
			job
		} else {
			if scope.replayed(source, id).await? {
				return Err(Error::Conflict(
					"foreign generation intent already finished".into(),
				));
			}
			let Assignment::Generated { generation } = scope.create(&intent, policy).await? else {
				return Err(Error::Forbidden);
			};
			*generation
		};
		if job.status == "QUEUED" && !job.prepared {
			scope.publish(&job, &spec).await?;
			scope.mark_prepared(job.id).await?;
			job.prepared = true;
		}
		Ok(Prepared {
			intent_id: id,
			node_id: repository.node_id().into(),
			request_id: job.id,
			agent: EntityRef {
				id: job.agent_id,
				version: job.agent_version,
			},
			status: job.status,
			prepared: job.prepared,
			expires_at: job.expires_at,
		})
	}
	.await;
	scope.finish(result).await
}

#[cfg(test)]
mod tests;
