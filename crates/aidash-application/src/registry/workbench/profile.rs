//! Tenant-owned real-tool profiles (#361). Subjects see them only after
//! current draft-test authority and rule matching.
use crate::{
	Error, Result,
	ports::registry::workbench::profile::{ProfileConfiguration, ProfileRepository},
};
use aidash_domain::{
	identity::Principal,
	keyless::Destination,
	registry::{
		Entry,
		workbench::profile::{
			ProfileInput, ProfileQuery, ProfileSummary, RealToolRule, TestProfile,
		},
	},
	tool::ToolConfig,
};
use std::collections::HashSet;
pub fn validate_real_rule(
	configuration: &dyn ProfileConfiguration,
	rule: &RealToolRule,
	tool: &Entry,
) -> Result<()> {
	aidash_domain::configuration::validate_endpoint(&rule.endpoint)?;
	let url = url::Url::parse(&rule.endpoint)
		.map_err(|_| Error::Invalid("invalid test endpoint".into()))?;
	// The Tenant chooses the endpoint the server calls, so it reaches only
	// public addresses unless the Deployment permits its origin.
	match configuration.destination(&rule.endpoint)? {
		Destination::Forbidden => {
			return Err(Error::Invalid(
				"a test endpoint at a non-public address needs its origin in [keyless_endpoints] permitted_private_origins"
					.into(),
			));
		}
		Destination::PublicOnly if url.scheme() != "https" => {
			return Err(Error::Invalid(
				"test endpoints require HTTPS unless the Deployment permits their origin".into(),
			));
		}
		Destination::Permitted | Destination::PublicOnly => {}
	}

	if tool.kind != "tool" {
		return Err(Error::Invalid(
			"test profile references a non-Tool Registry entry".into(),
		));
	}
	let descriptor: aidash_domain::tool::providers::ToolDescriptor =
		serde_json::from_value(tool.config.clone())?;
	let cfg = descriptor
		.transport
		.ok_or_else(|| Error::Invalid("real tests require an integration transport".into()))?;
	let isolated = match cfg {
		ToolConfig::Http { endpoint, .. } => {
			let production = url::Url::parse(&endpoint)
				.map_err(|_| Error::Invalid("invalid production endpoint".into()))?;
			rule.read_only_verified && production != url
		}
		_ => false,
	};
	if !isolated {
		return Err(Error::Invalid(
			"real tests require a read-only HTTP test endpoint, verified by the profile's writer and separate from the production endpoint"
				.into(),
		));
	}
	Ok(())
}
pub async fn list(
	repository: &dyn ProfileRepository,
	query: ProfileQuery,
) -> Result<Vec<ProfileSummary>> {
	let mut draft_tools = None;
	let tenant = match repository.principal() {
		Principal::DeploymentAdministrator => query
			.tenant
			.ok_or_else(|| Error::Invalid("tenant is required".into()))?,
		Principal::Subject { tenant, .. } => {
			if query.tenant.as_deref().is_some_and(|value| value != tenant) {
				return Err(Error::Forbidden);
			}
			let draft_id = query
				.draft_id
				.ok_or_else(|| Error::Invalid("draft_id is required".into()))?;
			let mut scope = repository.begin_draft().await?;
			let draft = scope.draft(draft_id).await?;
			scope.authorize(&draft, "agent_draft.test", true).await?;
			let entry: Entry = serde_json::from_value(draft.entry.clone())?;
			let snapshot = scope.bindings(&entry).await?;
			draft_tools = Some(
				snapshot
					.bindings
					.iter()
					.filter(|b| b.definition.kind == "tool" && b.excluded_reason.is_none())
					.map(|b| b.identity.local())
					.collect::<Vec<_>>(),
			);
			scope.commit().await?;
			tenant
		}
	};
	Ok(repository
		.page(&tenant)
		.await?
		.into_iter()
		.filter(|row| {
			let Some(tools) = &draft_tools else {
				return true;
			};
			if !row.enabled {
				return false;
			}
			serde_json::from_value::<Vec<RealToolRule>>(row.rules.clone()).is_ok_and(|rules| {
				!rules.is_empty() && rules.iter().all(|rule| tools.contains(&rule.tool))
			})
		})
		.map(|row| ProfileSummary {
			id: row.id,
			revision: row.revision,
			enabled: row.enabled,
		})
		.collect())
}
/// Every profile of a Tenant with its rules, for those who write them (#361).
pub async fn administered(
	repository: &dyn ProfileRepository,
	tenant: String,
) -> Result<Vec<TestProfile>> {
	if let Principal::Subject { tenant: own, .. } = repository.principal()
		&& own != tenant
	{
		return Err(Error::Forbidden);
	}
	repository.administered(&tenant).await
}
pub async fn put(
	repository: &dyn ProfileRepository,
	configuration: &dyn ProfileConfiguration,
	tenant: String,
	id: String,
	input: ProfileInput,
) -> Result<TestProfile> {
	// A Subject writes only its own Tenant's profiles, under
	// `workbench.test_profile.update`, which the repository checks with the write.
	if let Principal::Subject { tenant: own, .. } = repository.principal()
		&& own != tenant
	{
		return Err(Error::Forbidden);
	}
	if tenant.trim().is_empty() || id.trim().is_empty() || id.len() > 100 || input.rules.len() > 32
	{
		return Err(Error::Invalid(
			"invalid test profile identity or size".into(),
		));
	}
	// Before any definition is loaded or endpoint classified, so a caller
	// without the action learns nothing about either.
	repository.authorize_update(&tenant, &id).await?;
	let mut seen = HashSet::new();
	for rule in &input.rules {
		if !seen.insert((&rule.tool.id, &rule.tool.version))
			|| rule.allowed_actions.is_empty()
			|| rule.allowed_resources.is_empty()
			|| rule.allowed_actions.len() > 64
			|| rule.allowed_resources.len() > 64
			|| rule
				.allowed_actions
				.iter()
				.chain(&rule.allowed_resources)
				.any(|value| value.trim().is_empty() || value.len() > 200)
		{
			return Err(Error::Invalid(
				"real-tool rules need unique Tools and bounded action/resource allowlists".into(),
			));
		}
		let tool = repository.definition(&tenant, &rule.tool).await?;
		validate_real_rule(configuration, rule, &tool)?;
		// A rule may send a Tenant Secret to its test endpoint, which must be
		// one of that Tenant Secret's recorded hosts (ADR 0051).
		if let Some(name) = &rule.secret {
			aidash_domain::tenant_secrets::validate_name(name)?;
			let hosts = repository.tenant_secret_hosts(&tenant, name).await?;
			crate::tenant_secrets::admit(name, &rule.endpoint, hosts.as_deref())?;
		}
	}
	repository.save(&tenant, &id, &input).await
}
#[cfg(test)]
mod tests;
