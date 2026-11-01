//! Shared connection-wait payload and the bounded policy for one logical call.
use aidash_domain::{Run, agent_plugin::ConnectionCause};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct Need {
	pub resource: String,
	pub scopes: Vec<String>,
	pub reason: String,
	pub tool: String,
}
impl Need {
	pub fn from_cause(cause: &ConnectionCause, tool: &str) -> Option<Self> {
		let resource =
			aidash_domain::oauth_connections::canonical_resource(cause.resource()).ok()?;
		let (reason, scopes) = match cause {
			ConnectionCause::ConnectionRequired { .. } => ("missing", vec![]),
			ConnectionCause::ConnectionScopeInsufficient { scope, .. } => (
				"scope",
				scope
					.as_deref()
					.unwrap_or_default()
					.split_whitespace()
					.map(str::to_owned)
					.collect(),
			),
		};
		Some(Self {
			resource,
			scopes,
			reason: reason.into(),
			tool: tool.into(),
		})
	}
	pub fn prompt(&self) -> String {
		json!({"resource":self.resource,"scopes":self.scopes,"reason":self.reason,"tool":self.tool,"message":format!("Connect to {} to continue this task.",self.resource)}).to_string()
	}
}
#[derive(Debug, Clone, Copy)]
pub struct Policy {
	pub answer_deadline_seconds: Option<i64>,
	pub human_wait_seconds: u64,
}
impl Policy {
	pub fn deadline(self, started: DateTime<Utc>, waited_millis: u64) -> DateTime<Utc> {
		let remaining = self
			.human_wait_seconds
			.saturating_mul(1000)
			.saturating_sub(waited_millis)
			.min(i64::MAX as u64) as i64;
		let answer = self
			.answer_deadline_seconds
			.map(|seconds| seconds.max(0).saturating_mul(1000))
			.unwrap_or(remaining);
		started + Duration::milliseconds(remaining.min(answer))
	}
}
/// Fixed endpoint associations, derived only from the Run's trusted bindings.
fn tool_resources(run: &Run) -> BTreeMap<String, BTreeSet<String>> {
	let mut tool_resources = BTreeMap::<String, BTreeSet<String>>::new();
	// A successful sibling tool at the same fixed endpoint satisfies an older
	// refusal too; the model does not have to call the exact same alias.
	if let Some(snapshot) = &run.context.binding_snapshot {
		for binding in &snapshot.bindings {
			let Some(alias) = &binding.alias else {
				continue;
			};
			let transport =
				serde_json::from_value::<aidash_domain::tool::providers::ToolDescriptor>(
					binding.definition.config.clone(),
				)
				.ok()
				.and_then(|descriptor| descriptor.transport);
			let endpoint = match transport {
				Some(
					aidash_domain::tool::ToolConfig::Mcp { endpoint, .. }
					| aidash_domain::tool::ToolConfig::Http { endpoint, .. },
				) => endpoint,
				_ => continue,
			};
			if let Ok(resource) = aidash_domain::oauth_connections::canonical_resource(&endpoint) {
				tool_resources
					.entry(alias.clone())
					.or_default()
					.insert(resource);
			}
		}
		if let (Ok(agent), Some(pinned)) = (
			crate::plugin_tools::agent_bindings(snapshot),
			&run.context.plugin_tools,
		) {
			for server in &pinned.servers {
				let Some(config) = agent
					.agent_plugins
					.iter()
					.find(|plugin| plugin.name == server.plugin)
					.and_then(|plugin| plugin.servers.get(&server.server))
				else {
					continue;
				};
				let aidash_domain::agent_plugin::McpServer::Remote(config) = config else {
					continue;
				};
				let Ok(resource) =
					aidash_domain::oauth_connections::canonical_resource(config.url.as_str())
				else {
					continue;
				};
				for tool in &server.tools {
					tool_resources
						.entry(tool.alias.clone())
						.or_default()
						.insert(resource.clone());
				}
			}
		}
	}
	tool_resources
}

/// Only a transport-proven refusal can become a recovery candidate.
pub fn record_refusal(run: &mut Run, tool: &str, cause: ConnectionCause, scopes: Vec<String>) {
	run.recovery
		.connection_refusals
		.retain(|old| old.cause.resource() != cause.resource());
	run.recovery
		.connection_refusals
		.push(aidash_domain::run_state::ConnectionRefusal {
			cause,
			tool: tool.into(),
			scopes,
		});
}

/// Clear only after a real success for this fixed endpoint (or this exact tool).
/// The result JSON, including a body shaped like a connection refusal, is irrelevant.
pub fn record_success(run: &mut Run, tool: &str) {
	let resources = tool_resources(run).remove(tool).unwrap_or_default();
	run.recovery
		.connection_refusals
		.retain(|old| old.tool != tool && !resources.contains(old.cause.resource()));
}

pub fn failure_need(run: &Run) -> Option<Need> {
	if let Some(need) = run
		.recovery
		.connection_refusals
		.iter()
		.rev()
		.find_map(|refusal| {
			if run
				.recovery
				.connection_waited
				.contains(refusal.cause.resource())
			{
				return None;
			}
			let mut need = Need::from_cause(&refusal.cause, &refusal.tool)?;
			need.scopes.extend(refusal.scopes.clone());
			need.scopes.sort();
			need.scopes.dedup();
			Some(need)
		}) {
		return Some(need);
	}
	run.context
		.plugin_tools
		.as_ref()?
		.findings
		.iter()
		.find_map(|finding| {
			let cause = finding.connection.as_ref()?;
			if run.recovery.connection_waited.contains(cause.resource()) {
				return None;
			}
			Need::from_cause(cause, &format!("{} MCP server", finding.plugin))
		})
}
