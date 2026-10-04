//! Policy admission and atomic history share the current caller authority.
use crate::{
	Error, Result,
	ports::generation::policy::{GenerationPolicies, PolicySession},
	registry::DefinitionValidation,
};
use aidash_domain::{
	generation::policy::{Policy, Spec},
	identity::Principal,
	model::ModelConfig,
	policy::{PolicyBundle, identifier},
	registry::AgentConfig,
};
use serde_json::json;

fn same_tenant(principal: &Principal, tenant: &str) -> Result<()> {
	if let Principal::Subject { tenant: caller, .. } = principal
		&& caller != tenant
	{
		return Err(Error::Forbidden);
	}
	Ok(())
}
fn actor(principal: &Principal) -> &str {
	match principal {
		Principal::Operator => "operator",
		Principal::Subject { subject, .. } => subject,
	}
}

pub async fn set(
	repository: &dyn GenerationPolicies,
	tenant: &str,
	id: &str,
	expected: i64,
	spec: &Spec,
	validation: &DefinitionValidation,
) -> Result<Policy> {
	same_tenant(repository.principal(), tenant)?;
	let actor = actor(repository.principal()).to_owned();
	let mut scope = repository.begin(tenant, true).await?;
	let result = async {
		if matches!(repository.principal(), Principal::Subject { .. })
			&& !scope.decide(id, "generation.manage").await?
		{
			return Err(Error::Forbidden);
		}
		write_in(
			scope.as_mut(),
			tenant,
			id,
			expected,
			spec,
			&actor,
			validation,
		)
		.await
	}
	.await;
	scope.finish_update(result).await
}

pub async fn list(repository: &dyn GenerationPolicies, tenant: &str) -> Result<Vec<Policy>> {
	// Preserve the original ID discovery before establishing the read lease.
	let ids = repository.ids(tenant).await?;
	same_tenant(repository.principal(), tenant)?;
	let mut scope = repository.begin(tenant, false).await?;
	let result = async {
		let mut result = vec![];
		for id in ids {
			if matches!(repository.principal(), Principal::Operator)
				|| scope.decide(&id, "generation.read").await?
			{
				result.push(scope.load(tenant, &id, false).await?);
			}
		}
		Ok(result)
	}
	.await;
	scope.finish_list(result).await
}

pub fn validate(
	validation: &DefinitionValidation,
	spec: &Spec,
	bundle: &PolicyBundle,
) -> Result<AgentConfig> {
	if let Some(remote) = &spec.remote {
		remote.validate()?;
		if (spec.embedding.is_some() && remote.embedding.is_some())
			|| (spec.compaction.is_some() && remote.compaction.is_some())
		{
			return Err(Error::Invalid(
				"select one owning node for each generation provider allowance".into(),
			));
		}
	}
	validation.validate_in(&spec.template, true)?;
	if spec.template.kind != "agent" {
		return Err(Error::Invalid(
			"generation template must define an agent".into(),
		));
	}
	if spec.compaction.as_ref().is_some_and(|c| {
		!(1..=1_000_000).contains(&c.call_budget)
			|| !(1..=c.call_budget).contains(&c.calls_per_agent)
	}) {
		return Err(Error::Invalid("invalid compaction call limits".into()));
	}
	if spec.embedding.as_ref().is_some_and(|c| {
		!(1..=1_000_000).contains(&c.call_budget)
			|| !(1..=c.call_budget).contains(&c.calls_per_agent)
	}) {
		return Err(Error::Invalid("invalid embedding call limits".into()));
	}
	let limits = &spec.limits;
	if !(1..=512).contains(&limits.max_agents)
		|| !(1..=limits.max_agents).contains(&limits.max_concurrent)
		|| !(1..=31).contains(&limits.max_depth)
		|| !(1..=1_000_000_000_000_i64).contains(&limits.token_budget)
		|| !(1..=limits.token_budget).contains(&limits.tokens_per_agent)
		|| !(1..=2_592_000).contains(&limits.lifetime_seconds)
	{
		return Err(Error::Invalid("invalid generation limits".into()));
	}
	if !spec.permissions.attributes.is_object()
		|| spec.permissions.attributes.to_string().len() > 16_384
	{
		return Err(Error::Invalid(
			"generated attributes must be an object of at most 16 KiB".into(),
		));
	}
	for role in &spec.permissions.roles {
		if !bundle.roles.contains_key(role) {
			return Err(Error::Invalid("generated role does not exist".into()));
		}
	}
	for group in &spec.permissions.groups {
		if !bundle.groups.contains_key(group) {
			return Err(Error::Invalid("generated group does not exist".into()));
		}
	}
	let config: AgentConfig = serde_json::from_value(spec.template.config.clone())?;
	if config.knowledge_digest.is_some() {
		return Err(Error::Invalid(
			"private reference documents belong to a registered agent, not a generation template"
				.into(),
		));
	}
	Ok(config)
}

async fn write_in(
	scope: &mut dyn PolicySession,
	tenant: &str,
	id: &str,
	expected: i64,
	spec: &Spec,
	actor: &str,
	validation: &DefinitionValidation,
) -> Result<Policy> {
	identifier(tenant)?;
	identifier(id)?;
	identifier(actor)?;
	if !(0..i64::MAX).contains(&expected) {
		return Err(Error::Invalid("invalid generation policy revision".into()));
	}
	let document = scope.bundle(tenant).await?;
	// Revocation must never prevent an otherwise unchanged policy from being
	// disabled. Re-enabling or editing still validates every live dependency.
	let disabling = if expected > 0 && !spec.enabled {
		let previous = scope.previous(tenant, id, expected).await?;
		previous
			.map(|mut previous| {
				previous["enabled"] = json!(false);
				previous == json!(spec)
			})
			.unwrap_or(false)
	} else {
		false
	};
	if !disabling {
		let cfg = validate(validation, spec, &serde_json::from_value(document)?)?;
		for (reference, kind) in std::iter::once((&cfg.model, "model"))
			.chain(cfg.tools.iter().map(|r| (r, "tool")))
			.chain(cfg.skills.iter().map(|r| (r, "skill")))
			.chain(cfg.cluster.iter().map(|r| (r, "cluster")))
			.chain(spec.compaction.iter().map(|c| (&c.provider, "compactor")))
			.chain(spec.embedding.iter().map(|c| (&c.provider, "embedding")))
		{
			let metadata = scope.approved(tenant, reference).await?;
			let entry: aidash_domain::registry::Entry =
				serde_json::from_value(metadata.ok_or_else(|| {
					Error::Invalid("generation components require tenant catalog approval".into())
				})?)?;
			if entry.kind != kind {
				return Err(Error::Invalid(format!(
					"generation component must be a {kind}"
				)));
			}
			if kind == "model" {
				let model: ModelConfig = serde_json::from_value(entry.config)?;
				let required = model.context_window as i64 + model.output_token_limit() as i64;
				if spec.limits.tokens_per_agent < required {
					return Err(Error::Invalid(
						"agent token allowance is smaller than one model reservation".into(),
					));
				}
			}
		}
	}
	let revision = scope.compare_and_set(tenant, id, expected, spec).await?;
	let revision =
		revision.ok_or_else(|| Error::Conflict("generation policy revision changed".into()))?;
	scope.history(tenant, id, revision, spec, actor).await?;
	scope.load(tenant, id, false).await
}

#[cfg(test)]
mod tests;
