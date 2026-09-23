use crate::{
	Error, Result,
	config::{secret, validate_endpoint},
	domain::{ArtifactInput, NewTask, Run},
	federation::Home,
	provider::ToolSpec,
	registry::{EntityRef, Entry, Search},
	store::Store,
};
use async_trait::async_trait;
use rmcp::{
	ServiceExt,
	model::CallToolRequestParams,
	transport::{
		StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
	},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone)]
pub struct ToolContext {
	pub home: Home,
	pub store: Store,
	pub run: Run,
}
#[async_trait]
pub trait Tool: Send + Sync {
	fn specification(&self) -> ToolSpec;
	fn replay_safe(&self) -> bool;
	async fn invoke(&self, context: &ToolContext, input: Value, key: &str) -> Result<Value>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolConfig {
	Native {
		operation: String,
		#[serde(default)]
		allowed_hosts: Vec<String>,
	},
	Http {
		endpoint: String,
		credential_env: Option<String>,
		replay: String,
	},
	Mcp {
		endpoint: String,
		credential_env: Option<String>,
		tool_name: String,
		replay: String,
		idempotency_argument: Option<String>,
	},
	Agent {
		node_id: String,
		agent: EntityRef,
	},
}

pub fn validate_config(value: &Value) -> Result<()> {
	validate_config_in(value, true)
}
pub(crate) fn validate_config_in(value: &Value, local: bool) -> Result<()> {
	let cfg: ToolConfig =
		serde_json::from_value(value.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
	match &cfg {
		ToolConfig::Native {
			operation,
			allowed_hosts,
		} => {
			if !matches!(operation.as_str(), "echo" | "http_get")
				|| (operation == "http_get" && allowed_hosts.is_empty())
			{
				return Err(Error::Invalid(
					"native tools support echo or http_get with allowed_hosts".into(),
				));
			}
		}
		ToolConfig::Http {
			endpoint,
			credential_env,
			replay,
		}
		| ToolConfig::Mcp {
			endpoint,
			credential_env,
			replay,
			..
		} => {
			validate_endpoint(endpoint)?;
			if let Some(name) = credential_env {
				crate::config::validate_secret_reference(name)?;
				if local {
					secret(name)?;
				}
			}
			if !matches!(replay.as_str(), "read_only" | "idempotent" | "unsafe") {
				return Err(Error::Invalid(
					"replay must be read_only, idempotent or unsafe".into(),
				));
			}
			if let ToolConfig::Mcp {
				replay,
				idempotency_argument,
				..
			} = &cfg && replay == "idempotent"
				&& idempotency_argument
					.as_ref()
					.is_none_or(|s| s.trim().is_empty())
			{
				return Err(Error::Invalid(
					"idempotent MCP tools require an idempotency_argument supported by the server"
						.into(),
				));
			}
		}
		ToolConfig::Agent { node_id, agent } => {
			crate::config::validate_node_id(node_id)?;
			if agent.id.is_empty()
				|| agent.id.len() > 100
				|| !agent
					.id
					.as_bytes()
					.first()
					.is_some_and(u8::is_ascii_alphanumeric)
				|| !agent
					.id
					.bytes()
					.all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
				|| semver::Version::parse(&agent.version).is_err()
			{
				return Err(Error::Invalid(
					"agent tool requires a valid executor ID and semantic version".into(),
				));
			}
		}
	}
	Ok(())
}

mod mcp;

pub struct PluginTool {
	pub entry: Entry,
	pub alias: String,
	pub config: ToolConfig,
	pub client: reqwest::Client,
}
pub(crate) fn plugin_specification(entry: &Entry, alias: &str) -> ToolSpec {
	ToolSpec {
		name: alias.into(),
		description: entry
			.description
			.values()
			.cloned()
			.collect::<Vec<_>>()
			.join(" / "),
		parameters: entry.schema.clone(),
	}
}
#[async_trait]
impl Tool for PluginTool {
	fn specification(&self) -> ToolSpec {
		plugin_specification(&self.entry, &self.alias)
	}

	fn replay_safe(&self) -> bool {
		match &self.config {
			ToolConfig::Http { replay, .. } | ToolConfig::Mcp { replay, .. } => replay != "unsafe",
			_ => true,
		}
	}
	async fn invoke(&self, ctx: &ToolContext, input: Value, key: &str) -> Result<Value> {
		jsonschema::validator_for(&self.entry.schema)
			.map_err(|e| Error::Invalid(e.to_string()))?
			.validate(&input)
			.map_err(|e| Error::Invalid(e.to_string()))?;
		match &self.config {
			ToolConfig::Native {
				operation,
				allowed_hosts,
			} => match operation.as_str() {
				"echo" => Ok(input),
				"http_get" => {
					let url = reqwest::Url::parse(required(&input, "url")?)
						.map_err(|_| Error::Invalid("invalid URL".into()))?;
					if !matches!(url.scheme(), "https" | "http")
						|| !allowed_hosts
							.iter()
							.any(|h| Some(h.as_str()) == url.host_str())
						|| !url.username().is_empty()
						|| url.password().is_some()
					{
						return Err(Error::Invalid(
							"URL is outside the tool's configured hosts".into(),
						));
					}
					let response = self.client.get(url).send().await?.error_for_status()?;
					let mut body = response;
					let mut bytes = Vec::new();
					while let Some(chunk) = body.chunk().await? {
						if bytes.len() + chunk.len() > 256_000 {
							return Err(Error::Invalid("tool response exceeds 256 KB".into()));
						}
						bytes.extend_from_slice(&chunk);
					}
					Ok(json!({"text":String::from_utf8_lossy(&bytes)}))
				}
				_ => Err(Error::Invalid("unknown native tool".into())),
			},
			ToolConfig::Http {
				endpoint,
				credential_env,
				..
			} => {
				let mut req = self
					.client
					.post(endpoint)
					.header("idempotency-key", key)
					.json(&input);
				if let Some(name) = credential_env {
					req = req.bearer_auth(secret(name)?);
				}
				let response = req.send().await?;
				if !response.status().is_success() {
					return Err(Error::External(format!(
						"HTTP tool returned {}",
						response.status()
					)));
				}
				crate::response::json(response, 256_000).await
			}
			ToolConfig::Mcp {
				endpoint,
				credential_env,
				tool_name,
				idempotency_argument,
				..
			} => {
				let mut transport_config =
					StreamableHttpClientTransportConfig::with_uri(endpoint.clone());
				if let Some(name) = credential_env {
					transport_config.auth_header = Some(secret(name)?);
				}
				let transport = StreamableHttpClientTransport::with_client(
					mcp::BoundedClient(self.client.clone()),
					transport_config,
				);
				let service = ()
					.serve(transport)
					.await
					.map_err(|e| Error::External(format!("MCP initialization failed: {e}")))?;
				let mut args = input
					.as_object()
					.cloned()
					.ok_or_else(|| Error::Invalid("tool arguments must be an object".into()))?;
				if let Some(argument) = idempotency_argument {
					args.insert(argument.clone(), json!(key));
				}
				let result = service
					.call_tool(CallToolRequestParams::new(tool_name.clone()).with_arguments(args))
					.await
					.map_err(|e| Error::External(format!("MCP call failed: {e}")));
				let _ = service.cancel().await;
				let result = result?;
				if result.is_error == Some(true) {
					return Ok(json!({"is_error":true,"content":result.content}));
				}
				Ok(serde_json::to_value(result)?)
			}
			ToolConfig::Agent { node_id, agent } => {
				let mut input: NewTask =
					serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
				input.parent_id.get_or_insert(ctx.run.task_id);
				let task = ctx.home.create_task(&format!("{key}:task"), &input).await?;
				Ok(
					json!({"task":task,"delegation":ctx.home.delegate(task.id,node_id,agent).await?}),
				)
			}
		}
	}
}

pub struct Builtin {
	pub name: &'static str,
	pub description: &'static str,
	pub schema: Value,
}
#[async_trait]
impl Tool for Builtin {
	fn specification(&self) -> ToolSpec {
		ToolSpec {
			name: self.name.into(),
			description: self.description.into(),
			parameters: self.schema.clone(),
		}
	}
	fn replay_safe(&self) -> bool {
		true
	}
	async fn invoke(&self, ctx: &ToolContext, input: Value, key: &str) -> Result<Value> {
		jsonschema::validator_for(&self.schema)
			.map_err(|e| Error::Invalid(e.to_string()))?
			.validate(&input)
			.map_err(|e| Error::Invalid(e.to_string()))?;
		match self.name {
			"skill_read" => {
				let reference: EntityRef = serde_json::from_value(input["skill"].clone())
					.map_err(|error| Error::Invalid(error.to_string()))?;
				let path = required(&input, "path")?;
				let registry =
					crate::registry::Registry::new(ctx.store.pool.clone(), &ctx.store.node_id);
				let agent = registry
					.get(&ctx.run.agent_id, &ctx.run.agent_version)
					.await?;
				let config: crate::registry::AgentConfig = serde_json::from_value(agent.config)?;
				if !config.skills.contains(&reference) {
					return Err(Error::Forbidden);
				}
				let skill = registry.get(&reference.id, &reference.version).await?;
				let file = crate::registry::skill_files(&skill)?
					.into_iter()
					.find(|file| file.path == path)
					.ok_or_else(|| Error::NotFound(path.into()))?;
				let offset = input["offset"].as_u64().unwrap_or(0) as usize;
				let max_chars = input["max_chars"].as_u64().unwrap_or(8000).min(16000) as usize;
				let mut start = offset.min(file.content.len());
				while !file.content.is_char_boundary(start) {
					start -= 1;
				}
				let mut end = (start + max_chars).min(file.content.len());
				while !file.content.is_char_boundary(end) {
					end -= 1;
				}
				Ok(
					json!({"path":path,"text":&file.content[start..end],"encoding":file.encoding.as_deref().unwrap_or("utf8"),"total_chars":file.content.len(),"next_offset":if end < file.content.len() { Some(end) } else { None }}),
				)
			}
			"agent_discover" => Ok(json!(
				ctx.home
					.discover(&serde_json::from_value::<Search>(input)?)
					.await?
			)),
			"task_create" => {
				let mut task: NewTask =
					serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
				if task.parent_id.is_none() {
					task.parent_id = Some(ctx.run.task_id);
				}
				Ok(json!(ctx.home.create_task(key, &task).await?))
			}
			"task_assign" => {
				let id = required(&input, "task_id")?
					.parse()
					.map_err(|_| Error::Invalid("invalid task id".into()))?;
				Ok(json!(
					ctx.home
						.assign(
							id,
							required(&input, "policy_id")?,
							required(&input, "reason")?
						)
						.await?
				))
			}
			"task_delegate" => {
				let id = required(&input, "task_id")?
					.parse()
					.map_err(|_| Error::Invalid("invalid task id".into()))?;
				let agent: EntityRef = serde_json::from_value(input["agent"].clone())
					.map_err(|e| Error::Invalid(e.to_string()))?;
				Ok(json!(
					ctx.home
						.delegate(id, required(&input, "node_id")?, &agent)
						.await?
				))
			}
			"artifact_publish" => Ok(json!(
				ctx.home
					.artifact(key, &serde_json::from_value::<ArtifactInput>(input)?)
					.await?
			)),
			"workspace_message" => {
				ctx.home.message(key, required(&input, "content")?).await?;
				Ok(json!({"sent":true}))
			}
			"workspace_observe" => {
				ctx.home
					.observation(
						input["offset"].as_u64().unwrap_or(0) as usize,
						input["limit"]
							.as_u64()
							.unwrap_or(crate::context::observation::DEFAULT_LIMIT as u64)
							as usize,
					)
					.await
			}
			"workspace_read" => {
				let kind = required(&input, "kind")?;
				let id = required(&input, "id")?;
				ctx.home
					.read_record_chunk(
						kind,
						id,
						input["offset"].as_u64().unwrap_or(0) as usize,
						input["max_chars"].as_u64().unwrap_or(8000) as usize,
					)
					.await
			}
			"workspace_wait" => {
				Ok(json!({"wait_seconds":input["seconds"].as_u64().unwrap_or(2).clamp(1,60)}))
			}
			"memory_write" => {
				if let Some(authority) = &ctx.home.authority {
					authority.remember(&ctx.store, &ctx.run, &input).await?;
				} else if ctx.home.local() {
					let mut lease = crate::semantic::service::Lease::begin(
						&ctx.store,
						&crate::authorization::identity::Actor::Operator,
					)
					.await?;
					let result = crate::semantic::service::remember_in(
						&ctx.store, &mut lease, &ctx.run, &input,
					)
					.await;
					lease.finish(result).await?;
				} else {
					ctx.store.remember(&ctx.run, &input).await?;
				}
				Ok(json!({"saved":true}))
			}
			"human_request" => {
				let request = ctx
					.store
					.human_request(
						&ctx.run,
						required(&input, "kind")?,
						required(&input, "prompt")?,
						key,
					)
					.await?;
				Ok(json!({"human_request_id":request.id}))
			}
			_ => Err(Error::Invalid("unknown tool".into())),
		}
	}
}
pub fn required<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
	v[key]
		.as_str()
		.ok_or_else(|| Error::Invalid(format!("{key} must be a string")))
}

pub fn builtins() -> BTreeMap<String, Arc<dyn Tool>> {
	let string = json!({"type":"string"});
	let entity_ref = json!({"type":"object","required":["id","version"],"properties":{"id":string,"version":string},"additionalProperties":false});
	let entries = vec![
		Builtin {
			name: "skill_read",
			description: "Read a file bundled with one of this agent's registered Skills. Use the exact Skill id/version and relative path listed in the Skill instructions; continue from next_offset when present. Binary files are returned as base64 text with an encoding field.",
			schema: json!({"type":"object","required":["skill","path"],"properties":{"skill":entity_ref,"path":string,"offset":{"type":"integer","minimum":0},"max_chars":{"type":"integer","minimum":1,"maximum":16000}},"additionalProperties":false}),
		},
		Builtin {
			name: "agent_discover",
			description: "Find local and federated agents by capability, skill, tag, language or model. Choose an exact node_id, entity id and version from these results.",
			schema: json!({"type":"object","properties":{"capability":string,"language":string,"skill":string,"tag":string,"model":string,"query":string},"additionalProperties":false}),
		},
		Builtin {
			name: "task_create",
			description: "Decompose a goal or task. Create a subtask in this workspace; returns its ID. Delegate it next.",
			schema: json!({"type":"object","required":["title","description"],"properties":{"title":string,"description":string,"requirements":{"type":"object"},"dependencies":{"type":"array","items":string},"parent_id":string},"additionalProperties":false}),
		},
		Builtin {
			name: "task_assign",
			description: "Assign a workspace task to an approved existing agent, or request a generated specialist using an explicitly named generation policy. Generation may wait for human approval. Requires a tenant-scoped execution identity.",
			schema: json!({"type":"object","required":["task_id","policy_id","reason"],"properties":{"task_id":string,"policy_id":string,"reason":string},"additionalProperties":false}),
		},
		Builtin {
			name: "task_delegate",
			description: "Offer an existing workspace task to the explicitly selected local or remote agent.",
			schema: json!({"type":"object","required":["task_id","node_id","agent"],"properties":{"task_id":string,"node_id":string,"agent":entity_ref},"additionalProperties":false}),
		},
		Builtin {
			name: "artifact_publish",
			description: "Publish an intermediate artifact to the shared workspace.",
			schema: json!({"type":"object","required":["kind","name","content"],"properties":{"kind":{"enum":["text","json","file_reference","code","structured_result"]},"name":string,"content":{}},"additionalProperties":false}),
		},
		Builtin {
			name: "workspace_message",
			description: "Send a message to the human and other agents sharing this workspace.",
			schema: json!({"type":"object","required":["content"],"properties":{"content":string},"additionalProperties":false}),
		},
		Builtin {
			name: "workspace_observe",
			description: "Read a bounded summary of goal, tasks, artifact references, messages and recent event metadata. Use workspace_read for full records. Collections have separate totals and next_offset; events/messages are newest first. Refresh pagination if the workspace changes.",
			schema: json!({"type":"object","properties":{"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":50}},"additionalProperties":false}),
		},
		Builtin {
			name: "workspace_read",
			description: "Read one accessible workspace record by kind and exact ID. Returns a JSON text chunk, total_chars and next_offset; concatenate chunks in offset order to recover the full record. The returned chunk is capped to fit the active request budget. If budget_limited is true, continue from next_offset on a later turn; if deferred is true, stop reading until that later turn.",
			schema: json!({"type":"object","required":["kind","id"],"properties":{"kind":{"enum":["workspace","task","artifact","message","event"]},"id":string,"offset":{"type":"integer","minimum":0},"max_chars":{"type":"integer","minimum":0,"maximum":16000}},"additionalProperties":false}),
		},
		Builtin {
			name: "workspace_wait",
			description: "Wait without inference until peers have made progress. Recheck workspace tasks on the next turn.",
			schema: json!({"type":"object","required":["seconds"],"properties":{"seconds":{"type":"integer","minimum":1,"maximum":60}},"additionalProperties":false}),
		},
		Builtin {
			name: "memory_write",
			description: "Replace this agent's persistent memory for the current workspace with this JSON object.",
			schema: json!({"type":"object"}),
		},
		Builtin {
			name: "human_request",
			description: "Ask the human for input and wait for a response. Approval requests do not grant permission until answered.",
			schema: json!({"type":"object","required":["kind","prompt"],"properties":{"kind":{"enum":["QUESTION","APPROVAL_REQUIRED","CONFIRMATION","INFORMATION_REQUEST"]},"prompt":string},"additionalProperties":false}),
		},
	];
	entries
		.into_iter()
		.map(|t| (t.name.to_owned(), Arc::new(t) as Arc<dyn Tool>))
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn idempotent_mcp_requires_a_nonblank_idempotency_argument() {
		for argument in ["", " \t\n", "\u{2003}\u{00a0}"] {
			let config = json!({
				"transport":"mcp",
				"endpoint":"http://localhost:9999/mcp",
				"credential_env":null,
				"tool_name":"create",
				"replay":"idempotent",
				"idempotency_argument":argument
			});
			assert!(validate_config_in(&config, false).is_err(), "{argument:?}");
		}
		let valid = json!({
			"transport":"mcp",
			"endpoint":"http://localhost:9999/mcp",
			"credential_env":null,
			"tool_name":"create",
			"replay":"idempotent",
			"idempotency_argument":"request_id"
		});
		validate_config_in(&valid, false).unwrap();
	}
}
