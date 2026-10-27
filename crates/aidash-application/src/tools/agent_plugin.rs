//! A tool of a remote MCP server that a Run pinned from an Agent Plugin copy (ADR 0073). The Run's
//! session port lists and calls it; the pin decides its name, schema and Read-only declaration.
use super::validate_arguments;
use crate::{
	Error, Result,
	plugin_tools::agent_bindings,
	ports::{
		execution::ExecutionTool,
		tools::{PluginServer, PluginSessions},
	},
};
use aidash_domain::{
	Run,
	agent_plugin::{McpServer, PinnedTool, PluginName, PluginTools, RemoteServer, ServerName},
	provider::ToolSpec,
	registry::bindings::{AgentBindings, BindingSnapshot},
	tool::{AgentPluginTool, ToolContract},
};
use async_trait::async_trait;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

/// What a model sees of a pinned tool. Dispatch and the exposure catalog both take it from here,
/// so the catalog measures exactly what the request advertises.
pub fn pinned_specification(tool: &PinnedTool) -> ToolSpec {
	ToolSpec {
		name: tool.alias.clone(),
		description: tool.description.clone(),
		parameters: tool.input_schema.clone(),
	}
}

/// [`pinned_specification`] of every tool a Run pinned, by alias.
pub fn pinned_specifications(pinned: Option<&PluginTools>) -> BTreeMap<String, ToolSpec> {
	pinned
		.into_iter()
		.flat_map(|pinned| &pinned.servers)
		.flat_map(|server| &server.tools)
		.map(|tool| (tool.alias.clone(), pinned_specification(tool)))
		.collect()
}

fn remote_server<'a>(
	agent: &'a AgentBindings,
	plugin: &PluginName,
	digest: &str,
	server: &ServerName,
) -> Option<&'a RemoteServer> {
	let reference = agent
		.agent_plugins
		.iter()
		.find(|reference| &reference.name == plugin && reference.digest == digest)?;
	match reference.servers.get(server)? {
		McpServer::Remote(remote) => Some(remote),
		McpServer::Stdio(_) => None,
	}
}

/// The tools the Run pinned, by alias, ready to call over `sessions`. A server the Agent's plugin
/// copies no longer declare cannot be reached through a pin, so it refuses the Run.
pub fn pinned_tools(
	sessions: &Arc<dyn PluginSessions>,
	run: &Run,
	snapshot: &BindingSnapshot,
) -> Result<Vec<(String, Arc<dyn ExecutionTool>)>> {
	let Some(pinned) = &run.context.plugin_tools else {
		return Ok(Vec::new());
	};
	let agent = agent_bindings(snapshot)?;
	let mut tools: Vec<(String, Arc<dyn ExecutionTool>)> = Vec::new();
	for server in &pinned.servers {
		let config = remote_server(&agent, &server.plugin, &server.digest, &server.server)
			.ok_or_else(|| {
				Error::Conflict("Run pins a plugin server its Agent no longer declares".into())
			})?;
		for tool in &server.tools {
			let identity = AgentPluginTool {
				plugin: server.plugin.clone(),
				digest: server.digest.clone(),
				server: server.server.clone(),
				tool: tool.name.clone(),
			};
			tools.push((
				tool.alias.clone(),
				Arc::new(McpPluginTool {
					sessions: sessions.clone(),
					run: run.id,
					server: PluginServer {
						plugin: server.plugin.clone(),
						digest: server.digest.clone(),
						server: server.server.clone(),
					},
					config: config.clone(),
					contract: ToolContract::agent_plugin(identity, tool.read_only),
					tool: tool.clone(),
				}),
			));
		}
	}
	Ok(tools)
}

struct McpPluginTool {
	sessions: Arc<dyn PluginSessions>,
	run: Uuid,
	server: PluginServer,
	config: RemoteServer,
	contract: ToolContract,
	tool: PinnedTool,
}

impl McpPluginTool {
	fn pinned(&self, run: &Run) -> bool {
		run.context
			.plugin_tools
			.iter()
			.flat_map(|pinned| &pinned.servers)
			.filter(|server| {
				server.plugin == self.server.plugin
					&& server.digest == self.server.digest
					&& server.server == self.server.server
			})
			.flat_map(|server| &server.tools)
			.any(|tool| *tool == self.tool)
	}

	fn declared(&self, run: &Run) -> Result<bool> {
		let snapshot = run
			.context
			.binding_snapshot
			.as_ref()
			.ok_or_else(|| Error::Invalid("Run has no admitted Binding snapshot".into()))?;
		let agent = agent_bindings(snapshot)?;
		Ok(remote_server(
			&agent,
			&self.server.plugin,
			&self.server.digest,
			&self.server.server,
		) == Some(&self.config))
	}
}

#[async_trait]
impl ExecutionTool for McpPluginTool {
	fn mcp_rounds(&self) -> bool {
		true
	}
	async fn mcp_round(
		&self,
		run: &Run,
		input: Value,
		key: &str,
		checkpoint: Option<&aidash_domain::agent_plugin::interaction::CallCheckpoint>,
		inputs: Option<Arc<dyn crate::ports::tools::PluginInputs>>,
	) -> Result<aidash_domain::agent_plugin::interaction::ToolRound> {
		let admitted = self.admit(run, input).await?;
		self.sessions
			.round(
				run.id,
				&self.server,
				&self.config,
				&self.tool.name,
				&self.tool.schema_digest,
				admitted,
				key,
				checkpoint,
				inputs,
				None,
			)
			.await
	}
	fn specification(&self) -> ToolSpec {
		pinned_specification(&self.tool)
	}
	fn contract(&self) -> ToolContract {
		self.contract.clone()
	}
	fn external_target(&self) -> Option<String> {
		Some(self.config.url.as_str().to_owned())
	}
	fn replay_safe(&self) -> bool {
		self.contract.replay_safe()
	}
	async fn invoke(&self, run: &Run, input: Value, key: &str) -> Result<Value> {
		let admitted = self.admit(run, input).await?;
		self.dispatch(run, admitted, key).await
	}
	async fn admit(&self, run: &Run, input: Value) -> Result<Value> {
		if run.id != self.run || !self.pinned(run) || !self.declared(run)? {
			return Err(Error::Conflict(
				"plugin tool dispatch belongs to a different pinned Run".into(),
			));
		}
		validate_arguments(&self.tool.input_schema, &input)?;
		Ok(input)
	}
	async fn dispatch(&self, run: &Run, admitted: Value, key: &str) -> Result<Value> {
		self.sessions
			.call(
				run.id,
				&self.server,
				&self.config,
				&self.tool.name,
				&self.tool.schema_digest,
				admitted,
				key,
				None,
			)
			.await
	}
}

#[cfg(test)]
mod tests;
