//! Operator-owned profiles are exposed to subjects only after current draft-test authority and rule matching.
use crate::{
	Error, Result,
	ports::registry::workbench::profile::{ProfileConfiguration, ProfileRepository},
};
use aidash_domain::{
	identity::Principal,
	registry::{
		AgentConfig, Entry,
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
	if url.scheme() != "https"
		&& !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
	{
		return Err(Error::Invalid(
			"test endpoints require HTTPS except on loopback".into(),
		));
	}
	if let Some(name) = &rule.credential_env {
		configuration.require_secret(name)?;
	}

	if tool.kind != "tool" {
		return Err(Error::Invalid(
			"test profile references a non-Tool Registry entry".into(),
		));
	}
	let cfg: ToolConfig = serde_json::from_value(tool.config.clone())?;
	let isolated = match cfg {
		ToolConfig::Http {
			endpoint,
			replay,
			credential_env,
		} => {
			let production = url::Url::parse(&endpoint)
				.map_err(|_| Error::Invalid("invalid production endpoint".into()))?;
			replay == "read_only"
				&& production != url
				&& match credential_env {
					Some(production) => rule
						.credential_env
						.as_ref()
						.is_some_and(|test| test != &production),
					None => true,
				}
		}
		_ => false,
	};
	if !isolated {
		return Err(Error::Invalid("real tests require an HTTP read-only Tool, a separate test endpoint, and separate test credentials".into()));
	}
	Ok(())
}
pub async fn list(
	repository: &dyn ProfileRepository,
	query: ProfileQuery,
) -> Result<Vec<ProfileSummary>> {
	let mut draft_tools = None;
	let tenant = match repository.principal() {
		Principal::Operator => query
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
			let config: AgentConfig = serde_json::from_value(draft.entry["config"].clone())?;
			draft_tools = Some(config.tools);
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
pub async fn put(
	repository: &dyn ProfileRepository,
	configuration: &dyn ProfileConfiguration,
	tenant: String,
	id: String,
	input: ProfileInput,
) -> Result<TestProfile> {
	if repository.principal() != Principal::Operator {
		return Err(Error::Forbidden);
	}
	if tenant.trim().is_empty() || id.trim().is_empty() || id.len() > 100 || input.rules.len() > 32
	{
		return Err(Error::Invalid(
			"invalid test profile identity or size".into(),
		));
	}
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
		let tool = repository.definition(&rule.tool).await?;
		validate_real_rule(configuration, rule, &tool)?;
	}
	repository.save(&tenant, &id, &input).await
}
#[cfg(test)]
mod tests;
