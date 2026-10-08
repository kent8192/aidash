//! Origin-owned lineage, exact provider authority and atomic ancestor debits.
use crate::{
	Error, Result,
	authorization::catalog,
	ports::generation::reservation::{GenerationReservationRepository, GenerationUsageAuthority},
};
use aidash_domain::{
	generation::{
		policy::Spec,
		remote::{Ancestor, Purpose, Reserved, Usage, ancestor, lineage_live, usage_live},
		requests::Request,
	},
	registry::{AgentConfig, rules::digest},
	semantic::Failure,
};
use serde_json::json;

/// Derive owners exclusively from the authenticated scope's current subjects.
pub async fn lineage(
	scope: &mut dyn GenerationUsageAuthority,
	node: &str,
) -> Result<Vec<Ancestor>> {
	let jobs = scope.jobs(node).await?;
	for job in &jobs {
		let enabled = scope.policy_enabled(job).await?;
		if !lineage_live(job, enabled, scope.now()) {
			return Err(Error::RemoteSemantic(Failure::Authority));
		}
	}
	Ok(jobs.iter().map(|job| ancestor(node, job)).collect())
}

async fn approved(
	scope: &mut dyn GenerationUsageAuthority,
	node: &str,
	job: &Request,
	usage: &Usage,
) -> Result<()> {
	let enabled = scope.policy_enabled(job).await?;
	if !usage_live(job, enabled, scope.now()) {
		return Err(Error::RemoteSemantic(Failure::Authority));
	}
	let spec: Spec = serde_json::from_value(scope.pinned_policy(job).await?)?;
	if usage.purpose == Purpose::Memory {
		if spec
			.remote
			.as_ref()
			.is_none_or(|approvals| !approvals.memory.contains(&usage.provider))
		{
			return Err(Error::RemoteSemantic(Failure::Allowance));
		}
		if usage.provider.node_id == node {
			let catalog = scope.catalog();
			let entry = catalog::entry(catalog, &usage.provider.entry, "memory.invoke").await?;
			catalog
				.require(&catalog.resource(&entry), "registry.read")
				.await?;
			if digest(&serde_json::to_value(&entry)?) != usage.provider.digest
				|| digest(&entry.config) != usage.provider.configuration_digest
			{
				return Err(Error::RemoteSemantic(Failure::Configuration));
			}
		} else {
			let resource = scope.remote_resource("registry", &format!("{}/registry/{}@{}", usage.provider.node_id, usage.provider.entry.id, usage.provider.entry.version), json!({"remote_node":usage.provider.node_id,"definition_digest":usage.provider.digest,"purpose":"memory"}));
			scope.require(&resource, "registry.read").await?;
			scope.require(&resource, "memory.invoke").await?;
		}
		return Ok(());
	}
	if usage.provider.node_id == node {
		let expected = match usage.purpose {
			Purpose::Embedding => spec.embedding.map(|value| value.provider),
			Purpose::Compaction => spec.compaction.map(|value| value.provider),
			Purpose::Inference => {
				Some(serde_json::from_value::<AgentConfig>(spec.template.config)?.model)
			}
			Purpose::Memory => unreachable!("handled above"),
		}
		.ok_or(Error::RemoteSemantic(Failure::Allowance))?;
		if expected != usage.provider.entry {
			return Err(Error::RemoteSemantic(Failure::Allowance));
		}
		let catalog = scope.catalog();
		let entry = catalog::entry(catalog, &expected, usage.purpose.action()).await?;
		catalog
			.require(&catalog.resource(&entry), "registry.read")
			.await?;
		if digest(&serde_json::to_value(&entry)?) != usage.provider.digest
			|| digest(&entry.config) != usage.provider.configuration_digest
		{
			return Err(Error::RemoteSemantic(Failure::Configuration));
		}
	} else {
		let approvals = spec
			.remote
			.ok_or(Error::RemoteSemantic(Failure::Allowance))?;
		let permitted = match usage.purpose {
			Purpose::Embedding => approvals
				.embedding
				.as_ref()
				.is_some_and(|value| value.provider == usage.provider),
			Purpose::Compaction => approvals
				.compaction
				.as_ref()
				.is_some_and(|value| value.provider == usage.provider),
			Purpose::Inference => approvals.inference.contains(&usage.provider),
			Purpose::Memory => unreachable!("handled above"),
		};
		if !permitted {
			return Err(Error::RemoteSemantic(Failure::Allowance));
		}
		// The exact descriptor grants no local secret or catalog alias resolution.
		let resource = scope.remote_resource("registry", &format!("{}/registry/{}@{}", usage.provider.node_id, usage.provider.entry.id, usage.provider.entry.version), json!({"remote_node":usage.provider.node_id,"definition_digest":usage.provider.digest,"purpose":usage.purpose.name()}));
		scope.require(&resource, "registry.read").await?;
		scope.require(&resource, usage.purpose.action()).await?;
	}
	Ok(())
}

/// Admit all ancestors under current authority before opening the independent
/// debit transaction. Return receipts only after its commit succeeds.
pub async fn reserve(
	scope: &mut dyn GenerationUsageAuthority,
	repository: &dyn GenerationReservationRepository,
	usage: &Usage,
) -> Result<Vec<Reserved>> {
	usage.validate()?;
	let node = repository.node_id();
	let jobs = scope.jobs(node).await?;
	for job in &jobs {
		approved(scope, node, job, usage).await?;
	}
	let digest = usage.digest()?;
	let mut transaction = repository.begin().await?;
	let attempt = transaction.lock_attempt(usage.attempt_id, &digest).await?;
	if super::settlement::attempt_result(attempt, &digest)?.is_some() {
		return Err(Error::Conflict("provider attempt already finalized".into()));
	}
	let mut reservations = vec![];
	for job in jobs {
		transaction.lock_budget(&job).await?;
		if let Some(previous) = transaction.existing(&job, usage).await? {
			if previous.digest != digest || previous.state != "RESERVED" {
				return Err(Error::Conflict(
					"provider attempt already has a different or final reservation".into(),
				));
			}
		} else {
			if transaction.debit_budget(&job, usage).await? != 1 {
				return Err(Error::RemoteSemantic(Failure::Allowance));
			}
			transaction.insert(&job, usage, &digest).await?;
		}
		reservations.push(Reserved {
			owner: ancestor(node, &job),
			attempt_id: usage.attempt_id,
			digest: digest.clone(),
		});
	}
	transaction.commit().await?;
	Ok(reservations)
}
#[cfg(test)]
mod tests;
