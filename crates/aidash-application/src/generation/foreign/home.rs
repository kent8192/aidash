//! Durable Home intents and prepared subjects share the current authority policy.
use super::home_authority;
use crate::{
	Error, Result,
	ports::generation::foreign::home::{HomeGenerationRepository, HomeGenerationScope},
};
use aidash_domain::{
	TaskStatus,
	generation::intent::{Input, Intent, Prepared},
	identity::Principal,
	policy::{Subject, SubjectKind},
	qualified_agent,
};
use serde_json::json;
use uuid::Uuid;

pub async fn lease(
	repository: &dyn HomeGenerationRepository,
	source: &str,
	id: Uuid,
	exclusive: bool,
) -> Result<(Box<dyn HomeGenerationScope>, Intent)> {
	let record = repository.load(id).await?;
	let intent: Intent = serde_json::from_value(record.binding.clone())?;
	if record.cancelled
		|| intent.target_node != source
		|| intent.expires_at <= repository.now()
		|| intent.home_node != repository.node_id()
	{
		return Err(Error::Forbidden);
	}
	let mut scope = repository.begin_saved(&record, exclusive).await?;
	let result = async {
		let current = scope.current(id).await?;
		if current.cancelled
			|| current.binding != record.binding
			|| current.subject_chain != record.subject_chain
			|| current.credential_id != record.credential_id
		{
			return Err(Error::Forbidden);
		}
		scope.replace_subjects(record.subject_chain);
		let task = scope.task(intent.task.id).await?;
		if task.revision != intent.task.revision || task.status != TaskStatus::Open {
			return Err(Error::Forbidden);
		}
		home_authority(scope.as_mut(), &task, source, &intent.policy_id).await?;
		if scope.lineage(repository.node_id()).await? != intent.lineage {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	.await;
	if let Err(error) = result {
		return Err(scope.abort(error).await);
	}
	Ok((scope, intent))
}

pub async fn request(
	repository: &dyn HomeGenerationRepository,
	task_id: Uuid,
	input: Input,
) -> Result<Prepared> {
	let Principal::Subject { tenant, subject } = repository.principal() else {
		return Err(Error::Forbidden);
	};
	input.validate(repository.node_id())?;
	let mut scope = repository.begin().await?;
	let result = async {
		scope.inherit_task_origin(task_id).await?;
		let task = scope.task(task_id).await?;
		if task.status != TaskStatus::Open {
			return Err(Error::Conflict("task is already assigned".into()));
		}
		home_authority(scope.as_mut(), &task, &input.node_id, &input.policy_id).await?;
		let intent = Intent {
			id: input.id,
			home_node: repository.node_id().into(),
			source_tenant: tenant.clone(),
			source_subject: subject.clone(),
			task,
			target_node: input.node_id.clone(),
			policy_id: input.policy_id.clone(),
			policy_revision: input.policy_revision,
			lineage: scope.lineage(repository.node_id()).await?,
			reason: input.reason.clone(),
			ttl_seconds: input.ttl_seconds,
			expires_at: repository.now() + chrono::Duration::seconds(input.ttl_seconds),
		};
		scope.insert(&intent).await?;
		let saved = scope.saved(input.id).await?;
		let old: Intent = serde_json::from_value(saved.binding)?;
		let mut expected = intent;
		expected.expires_at = old.expires_at;
		let authority = scope.authority();
		if saved.cancelled
			|| saved.tenant != *tenant
			|| saved.credential_id != authority.credential_id
			|| saved.subject_chain != authority.subjects
			|| json!(expected) != json!(old)
		{
			return Err(Error::Conflict(
				"generation intent already binds different authority or policy".into(),
			));
		}
		Ok(())
	}
	.await;
	if let Err(error) = result {
		return Err(scope.abort(error).await);
	}
	scope.finish().await?;
	let prepared = repository.prepare(&input.node_id, input.id).await?;
	if prepared.intent_id != input.id || prepared.node_id != input.node_id {
		return Err(Error::Forbidden);
	}
	if prepared.prepared {
		let (mut scope, intent) = lease(repository, &input.node_id, input.id, true).await?;
		let result = async {
			let subject =
				qualified_agent(&input.node_id, &prepared.agent.id, &prepared.agent.version);
			let value = Subject {
				kind: SubjectKind::Agent,
				roles: Default::default(),
				groups: Default::default(),
				attributes: json!({"generation_intent":intent.id,"remote_node":input.node_id}),
				enabled: true,
				delegated_by: scope.authority().subjects.last().cloned(),
			};
			if let Some(old) = scope.snapshot().bundle.subjects.get(&subject) {
				if json!(old) != json!(value) {
					return Err(Error::Conflict(
						"prepared executor subject already differs".into(),
					));
				}
			} else {
				scope.snapshot_mut().bundle.subjects.insert(subject, value);
				scope.snapshot().bundle.validate()?;
				scope.snapshot_mut().revision = scope
					.snapshot()
					.revision
					.checked_add(1)
					.ok_or_else(|| Error::Invalid("authorization revision exhausted".into()))?;
				scope.save_snapshot().await?;
			}
			Ok(())
		}
		.await;
		if let Err(error) = result {
			return Err(scope.abort(error).await);
		}
		scope.finish().await?;
	}
	Ok(prepared)
}

pub async fn describe(
	repository: &dyn HomeGenerationRepository,
	source: &str,
	id: Uuid,
) -> Result<Intent> {
	let (scope, intent) = lease(repository, source, id, false).await?;
	scope.finish().await?;
	Ok(intent)
}

#[cfg(test)]
mod tests;

/// Cancellation requires management authority without disclosing stored execution context.
pub async fn cancel(
	repository: &dyn HomeGenerationRepository,
	maintenance: &dyn crate::ports::generation::foreign::maintenance::ForeignGenerationMaintenance,
	task: Uuid,
	id: Uuid,
) -> Result<bool> {
	let Principal::Subject { tenant, .. } = repository.principal() else {
		return Err(Error::Forbidden);
	};
	let record = repository.load(id).await?;
	let intent: Intent = serde_json::from_value(record.binding)?;
	if intent.task.id != task
		|| record.tenant != *tenant
		|| Some(record.credential_id) != repository.credential_id()
	{
		return Err(Error::Forbidden);
	}
	let mut scope = repository.begin().await?;
	let result = async {
		scope
			.require(
				&scope.resource(
					"task",
					&task.to_string(),
					json!({"workspace_id":intent.task.workspace_id}),
				),
				"task.delegate",
			)
			.await?;
		scope.set_cancelled(id).await
	}
	.await;
	if let Err(error) = result {
		return Err(scope.abort(error).await);
	}
	scope.finish().await?;
	if let Err(error) =
		super::maintenance::deliver_cancel(maintenance, id, &intent.target_node).await
	{
		maintenance.warn_cancel(id, &error, false);
	}
	Ok(true)
}
