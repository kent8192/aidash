//! Operator-owned, tenant-specific connection profiles for confined real-tool tests.
use super::*;
use crate::apps::registry::workbench::models::AgentTestProfile;
use crate::{config, tool::ToolConfig};
use reinhardt::injectable;

use std::collections::HashSet;

pub(super) fn validate_real_rule(rule: &RealToolRule, tool: &crate::registry::Entry) -> Result<()> {
	config::validate_endpoint(&rule.endpoint)?;
	let url = reqwest::Url::parse(&rule.endpoint)
		.map_err(|_| Error::Invalid("invalid test endpoint".into()))?;
	if url.scheme() != "https"
		&& !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
	{
		return Err(Error::Invalid(
			"test endpoints require HTTPS except on loopback".into(),
		));
	}
	if let Some(name) = &rule.credential_env {
		config::secret(name)?;
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
			let production = reqwest::Url::parse(&endpoint)
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

pub use crate::apps::registry::workbench::serializers::profile::{
	ProfileInput, ProfileQuery, ProfileSummary, RealToolRule, TestProfile,
};

#[derive(Clone)]
pub struct TestProfiles {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_profile(#[inject] runtime: Federation) -> TestProfiles {
	TestProfiles { runtime }
}

impl TestProfiles {
	pub(crate) async fn list(
		&self,
		actor: Actor,
		query: ProfileQuery,
	) -> Result<Vec<ProfileSummary>> {
		let f = self.runtime.clone();
		let mut draft_tools = None;
		let tenant = match actor {
			Actor::Operator => query
				.tenant
				.ok_or_else(|| Error::Invalid("tenant is required".into()))?,
			Actor::Subject(subject) => {
				if query
					.tenant
					.as_deref()
					.is_some_and(|value| value != subject.tenant)
				{
					return Err(Error::Forbidden);
				}
				let draft_id = query
					.draft_id
					.ok_or_else(|| Error::Invalid("draft_id is required".into()))?;
				let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
				let draft = AgentDraft::read(&mut tx, draft_id, false).await?;
				authorize(
					&mut tx,
					&Actor::Subject(subject.clone()),
					&draft,
					"agent_draft.test",
					true,
				)
				.await?;
				let config: AgentConfig = serde_json::from_value(draft.entry["config"].clone())?;
				draft_tools = Some(config.tools);
				Box::new(tx).commit().await?;
				subject.tenant
			}
		};
		let lease = f.store.orm_connection()?;
		let rows = lease
			.handle()
			.atomic(async |tx| AgentTestProfile::page(tx, &tenant).await)
			.await?;
		Ok(rows
			.into_iter()
			.filter(|row: &TestProfile| {
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
			.map(|row: TestProfile| ProfileSummary {
				id: row.id,
				revision: row.revision,
				enabled: row.enabled,
			})
			.collect())
	}
	pub(crate) async fn put(
		&self,
		actor: Actor,
		(tenant, id): (String, String),
		input: ProfileInput,
	) -> Result<TestProfile> {
		let f = self.runtime.clone();
		if !matches!(actor, Actor::Operator) {
			return Err(Error::Forbidden);
		}
		if tenant.trim().is_empty()
			|| id.trim().is_empty()
			|| id.len() > 100
			|| input.rules.len() > 32
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
					"real-tool rules need unique Tools and bounded action/resource allowlists"
						.into(),
				));
			}
			let tool = f.registry.get(&rule.tool.id, &rule.tool.version).await?;
			validate_real_rule(rule, &tool)?;
		}
		let lease = f.store.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| AgentTestProfile::save(tx, &tenant, &id, &input).await)
			.await
	}
}
