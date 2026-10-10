//! Tool validation, skill reads, and delegated task execution.
use crate::{
	Error, Result,
	ports::tools::{ToolOperations, ToolTransport},
};
use aidash_domain::{
	ArtifactInput, NewTask, Run,
	provider::ToolSpec,
	registry::{EntityRef, Entry, Search},
	tool::ToolConfig,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemoryMutationInput {
	changes: Vec<aidash_domain::memory::Change>,
}

pub struct ToolContext<'a> {
	pub operations: &'a dyn ToolOperations,
	pub run: &'a Run,
}
pub fn plugin_specification(entry: &Entry, alias: &str) -> ToolSpec {
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

/// The same schema and replay policy apply to HTTP, MCP, and agent tools.
pub struct Plugin {
	pub entry: Entry,
	pub alias: String,
	pub config: ToolConfig,
}
impl Plugin {
	pub fn specification(&self) -> ToolSpec {
		plugin_specification(&self.entry, &self.alias)
	}
	pub fn contract(&self) -> aidash_domain::tool::ToolContract {
		aidash_domain::tool::ToolContract::registry(
			EntityRef {
				id: self.entry.id.clone(),
				version: self.entry.version.clone(),
			},
			&self.config,
		)
	}
	pub fn replay_safe(&self) -> bool {
		self.contract().replay_safe()
	}

	pub async fn invoke(
		&self,
		ctx: &ToolContext<'_>,
		transport: &dyn ToolTransport,
		input: Value,
		key: &str,
	) -> Result<Value> {
		validate_arguments(&self.entry.schema, &input)?;
		match &self.config {
			ToolConfig::Native { operation, .. } if operation == "echo" => Ok(input),
			ToolConfig::Native {
				operation,
				allowed_hosts,
			} if operation == "http_get" => {
				let url = url::Url::parse(required(&input, "url")?)
					.map_err(|_| Error::Invalid("invalid URL".into()))?;
				if !matches!(url.scheme(), "https" | "http")
					|| !allowed_hosts
						.iter()
						.any(|host| Some(host.as_str()) == url.host_str())
					|| !url.username().is_empty()
					|| url.password().is_some()
				{
					return Err(Error::Invalid(
						"URL is outside the tool's configured hosts".into(),
					));
				}
				transport.invoke(&self.config, input, key).await
			}
			ToolConfig::Native { .. } => Err(Error::Invalid("unknown native tool".into())),
			ToolConfig::Http { .. } | ToolConfig::Mcp { .. } => {
				transport.invoke(&self.config, input, key).await
			}
			ToolConfig::Agent { node_id, agent } => {
				let mut input: NewTask =
					serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
				input.parent_id.get_or_insert(ctx.run.task_id);
				let task = ctx
					.operations
					.create_task(&format!("{key}:task"), &input)
					.await?;
				let delegation_key = format!("{key}:delegate");
				Ok(
					json!({"task":task,"delegation":ctx.operations.delegate_with_key(&delegation_key,task.id,node_id,agent).await?}),
				)
			}
		}
	}
}
fn validate_arguments(schema: &Value, input: &Value) -> Result<()> {
	jsonschema::validator_for(schema)
		.map_err(|e| Error::Invalid(e.to_string()))?
		.validate(input)
		.map_err(|e| Error::Invalid(e.to_string()))
}

pub struct Builtin {
	pub name: &'static str,
	contract: aidash_domain::tool::ToolContract,
	pub description: &'static str,
	pub schema: Value,
}
impl Builtin {
	pub fn specification(&self) -> ToolSpec {
		ToolSpec {
			name: self.name.into(),
			description: self.description.into(),
			parameters: self.schema.clone(),
		}
	}
	pub fn contract(&self) -> aidash_domain::tool::ToolContract {
		self.contract.clone()
	}
	pub fn replay_safe(&self) -> bool {
		self.contract().replay_safe()
	}

	pub async fn invoke(&self, ctx: &ToolContext<'_>, input: Value, key: &str) -> Result<Value> {
		jsonschema::validator_for(&self.schema)
			.map_err(|e| Error::Invalid(e.to_string()))?
			.validate(&input)
			.map_err(|e| Error::Invalid(e.to_string()))?;
		match self.name {
			"skill_read" => {
				let reference: EntityRef = serde_json::from_value(input["skill"].clone())
					.map_err(|error| Error::Invalid(error.to_string()))?;
				let path = required(&input, "path")?;
				if !ctx
					.operations
					.registered_skills()
					.await?
					.contains(&reference)
				{
					return Err(Error::Forbidden);
				}
				let file = ctx
					.operations
					.skill_files(&reference)
					.await?
					.into_iter()
					.find(|file| file.path == path)
					.ok_or_else(|| Error::Invalid(format!("Skill file not found: {path}")))?;
				let offset = input["offset"].as_u64().unwrap_or(0) as usize;
				let max_chars = input["max_chars"].as_u64().unwrap_or(8000).min(16000) as usize;
				let total = file.content.chars().count();
				let start = offset.min(total);
				let text: String = file.content.chars().skip(start).take(max_chars).collect();
				let end = start + text.chars().count();
				Ok(
					json!({"path":path,"text":text,"encoding":file.encoding.as_deref().unwrap_or("utf8"),"offset":start,"total_chars":total,"next_offset":if end < total { Some(end) } else { None }}),
				)
			}
			"agent_discover" => Ok(json!(
				ctx.operations
					.discover(&serde_json::from_value::<Search>(input)?)
					.await?
			)),
			"task_create" => {
				let mut task: NewTask =
					serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
				if task.parent_id.is_none() {
					task.parent_id = Some(ctx.run.task_id);
				}
				Ok(json!(ctx.operations.create_task(key, &task).await?))
			}
			"task_assign" => {
				let id = required(&input, "task_id")?
					.parse()
					.map_err(|_| Error::Invalid("invalid task id".into()))?;
				Ok(json!(
					ctx.operations
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
					ctx.operations
						.delegate_with_key(key, id, required(&input, "node_id")?, &agent)
						.await?
				))
			}
			"artifact_publish" => Ok(json!(
				ctx.operations
					.artifact(key, &serde_json::from_value::<ArtifactInput>(input)?)
					.await?
			)),
			"workspace_message" => {
				ctx.operations
					.message(key, required(&input, "content")?)
					.await?;
				Ok(json!({"sent":true}))
			}
			"workspace_observe" => {
				ctx.operations
					.observation(
						input["offset"].as_u64().unwrap_or(0) as usize,
						input["limit"]
							.as_u64()
							.unwrap_or(aidash_domain::context::observation::DEFAULT_LIMIT as u64)
							as usize,
					)
					.await
			}
			"workspace_read" => {
				let kind = required(&input, "kind")?;
				let id = required(&input, "id")?;
				ctx.operations
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
			"memory_mutate" => {
				let input: MemoryMutationInput = serde_json::from_value(input)?;
				if input.changes.iter().any(|change| matches!(change,
					aidash_domain::memory::Change::Add { content, .. } | aidash_domain::memory::Change::Correct { content, .. }
					if content.verification != aidash_domain::memory::Verification::Unverified)) {
					return Err(Error::Invalid("an agent cannot attest memory verification through a model tool".into()));
				}
				Ok(json!(
					ctx.operations.memory_mutate(key, &input.changes).await?
				))
			}
			"memory_recall" | "memory_reflect" => {
				let query = serde_json::from_value(input)?;
				ctx.operations
					.memory_recall(key, &query, self.name == "memory_reflect")
					.await
			}
			"human_request" => {
				let request = ctx
					.operations
					.human_request(required(&input, "kind")?, required(&input, "prompt")?, key)
					.await?;
				Ok(json!({"human_request_id":request.id}))
			}
			"capability_search"
			| "capability_describe"
			| "capability_load"
			| "capability_unload"
			| "skill_asset_read" => exposure::invoke(self.name, ctx, input).await,
			_ => Err(Error::Invalid("unknown tool".into())),
		}
	}
}
pub fn required<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
	v[key]
		.as_str()
		.ok_or_else(|| Error::Invalid(format!("{key} must be a string")))
}
pub fn builtins() -> BTreeMap<String, Builtin> {
	let string = json!({"type":"string"});
	let entity_ref = json!({"type":"object","required":["id","version"],"properties":{"id":string,"version":string},"additionalProperties":false});
	let entries = vec![
		Builtin {
			name: "skill_read",
			contract: aidash_domain::tool::builtin_contract("skill_read")
				.expect("declared builtin"),
			description: "Read a file bundled with one of this agent's registered Skills. Use the exact Skill id/version and relative path listed in the Skill instructions; continue from next_offset when present. Binary files are returned as base64 text with an encoding field. The returned chunk is capped to fit the active request budget; if deferred is true, continue on a later turn.",
			schema: json!({"type":"object","required":["skill","path"],"properties":{"skill":entity_ref,"path":string,"offset":{"type":"integer","minimum":0},"max_chars":{"type":"integer","minimum":0,"maximum":16000}},"additionalProperties":false}),
		},
		Builtin {
			name: "agent_discover",
			contract: aidash_domain::tool::builtin_contract("agent_discover")
				.expect("declared builtin"),
			description: "Find local and federated agents by capability, skill, tag, language or model. Choose an exact node_id, entity id and version from these results.",
			schema: json!({"type":"object","properties":{"capability":string,"language":string,"skill":string,"tag":string,"model":string,"query":string},"additionalProperties":false}),
		},
		Builtin {
			name: "task_create",
			contract: aidash_domain::tool::builtin_contract("task_create")
				.expect("declared builtin"),
			description: "Decompose a goal or task. Create a subtask in this workspace; returns its ID. Delegate it next.",
			schema: json!({"type":"object","required":["title","description"],"properties":{"title":string,"description":string,"requirements":{"type":"object"},"dependencies":{"type":"array","items":string},"parent_id":string},"additionalProperties":false}),
		},
		Builtin {
			name: "task_assign",
			contract: aidash_domain::tool::builtin_contract("task_assign")
				.expect("declared builtin"),
			description: "Assign a workspace task to an approved existing agent, or request a generated specialist using an explicitly named generation policy. Generation may wait for human approval. Requires a tenant-scoped execution identity.",
			schema: json!({"type":"object","required":["task_id","policy_id","reason"],"properties":{"task_id":string,"policy_id":string,"reason":string},"additionalProperties":false}),
		},
		Builtin {
			name: "task_delegate",
			contract: aidash_domain::tool::builtin_contract("task_delegate")
				.expect("declared builtin"),
			description: "Offer an existing workspace task to the explicitly selected local or remote agent.",
			schema: json!({"type":"object","required":["task_id","node_id","agent"],"properties":{"task_id":string,"node_id":string,"agent":entity_ref},"additionalProperties":false}),
		},
		Builtin {
			name: "artifact_publish",
			contract: aidash_domain::tool::builtin_contract("artifact_publish")
				.expect("declared builtin"),
			description: "Publish an intermediate artifact to the shared workspace.",
			schema: json!({"type":"object","required":["kind","name","content"],"properties":{"kind":{"enum":["text","json","file_reference","code","structured_result"]},"name":string,"content":{}},"additionalProperties":false}),
		},
		Builtin {
			name: "workspace_message",
			contract: aidash_domain::tool::builtin_contract("workspace_message")
				.expect("declared builtin"),
			description: "Send a message to the human and other agents sharing this workspace.",
			schema: json!({"type":"object","required":["content"],"properties":{"content":string},"additionalProperties":false}),
		},
		Builtin {
			name: "workspace_observe",
			contract: aidash_domain::tool::builtin_contract("workspace_observe")
				.expect("declared builtin"),
			description: "Read a bounded summary of goal, tasks, artifact references, messages and recent event metadata. Use workspace_read for full records. Collections have separate totals and next_offset; events/messages are newest first. Refresh pagination if the workspace changes.",
			schema: json!({"type":"object","properties":{"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":50}},"additionalProperties":false}),
		},
		Builtin {
			name: "workspace_read",
			contract: aidash_domain::tool::builtin_contract("workspace_read")
				.expect("declared builtin"),
			description: "Read one accessible workspace record by kind and exact ID. Returns a JSON text chunk, total_chars and next_offset; concatenate chunks in offset order to recover the full record. The returned chunk is capped to fit the active request budget. If budget_limited is true, continue from next_offset on a later turn; if deferred is true, stop reading until that later turn.",
			schema: json!({"type":"object","required":["kind","id"],"properties":{"kind":{"enum":["workspace","task","artifact","message","event"]},"id":string,"offset":{"type":"integer","minimum":0},"max_chars":{"type":"integer","minimum":0,"maximum":16000}},"additionalProperties":false}),
		},
		Builtin {
			name: "workspace_wait",
			contract: aidash_domain::tool::builtin_contract("workspace_wait")
				.expect("declared builtin"),
			description: "Wait without inference until peers have made progress. Recheck workspace tasks on the next turn.",
			schema: json!({"type":"object","required":["seconds"],"properties":{"seconds":{"type":"integer","minimum":1,"maximum":60}},"additionalProperties":false}),
		},
		Builtin {
			name: "memory_mutate",
			contract: aidash_domain::tool::builtin_contract("memory_mutate")
				.expect("declared builtin"),
			description: "Explicitly add, correct, or delete individual unverified memory units in the host-bound private bank. Corrections and deletions require the caller-observed expected_revision. Preserve exact evidence. Never replace a bank.",
			schema: json!(schemars::schema_for!(MemoryMutationInput)),
		},
		Builtin {
			name: "memory_recall",
			contract: aidash_domain::tool::builtin_contract("memory_recall")
				.expect("declared builtin"),
			description: "Recall bounded current memory from the Home's bound participant using semantic, keyword, graph and temporal search. Preserve provenance and uncertainty.",
			schema: json!(schemars::schema_for!(aidash_domain::memory::RecallQuery)),
		},
		Builtin {
			name: "memory_reflect",
			contract: aidash_domain::tool::builtin_contract("memory_reflect")
				.expect("declared builtin"),
			description: "Explicitly reflect over currently admitted memory with bounded exact-citation reads. This operation does not admit new learning.",
			schema: json!(schemars::schema_for!(aidash_domain::memory::RecallQuery)),
		},
		Builtin {
			name: "human_request",
			contract: aidash_domain::tool::builtin_contract("human_request")
				.expect("declared builtin"),
			description: "Ask the human for input and wait for a response. Approval requests do not grant permission until answered.",
			schema: json!({"type":"object","required":["kind","prompt"],"properties":{"kind":{"enum":["QUESTION","APPROVAL_REQUIRED","CONFIRMATION","INFORMATION_REQUEST"]},"prompt":string},"additionalProperties":false}),
		},
		Builtin {
			name: "capability_search",
			contract: aidash_domain::tool::builtin_contract("capability_search")
				.expect("declared builtin"),
			description: "Search this agent's Discoverable capabilities (tools and Skills) by keywords; an empty query lists all. Results give each alias, kind, summary, digest and whether it is loaded. Continue with next_cursor when truncated.",
			schema: json!({"type":"object","properties":{"query":string,"cursor":string},"additionalProperties":false}),
		},
		Builtin {
			name: "capability_describe",
			contract: aidash_domain::tool::builtin_contract("capability_describe")
				.expect("declared builtin"),
			description: "Describe one Discoverable capability by alias: its exact identity and digest, the full tool definition or Skill file inventory, and the bytes it uses of its exposure budget.",
			schema: json!({"type":"object","required":["alias"],"properties":{"alias":string},"additionalProperties":false}),
		},
		Builtin {
			name: "capability_load",
			contract: aidash_domain::tool::builtin_contract("capability_load")
				.expect("declared builtin"),
			description: "Load a Discoverable capability by alias and digest. A loaded tool can be called and loaded Skill instructions appear from the next request. Loading never evicts anything; when the budget is exceeded, unload a capability first.",
			schema: json!({"type":"object","required":["alias","digest"],"properties":{"alias":string,"digest":string},"additionalProperties":false}),
		},
		Builtin {
			name: "capability_unload",
			contract: aidash_domain::tool::builtin_contract("capability_unload")
				.expect("declared builtin"),
			description: "Unload a loaded or eager capability by alias to free its exposure budget from the next request. Mandatory capabilities cannot be unloaded.",
			schema: json!({"type":"object","required":["alias"],"properties":{"alias":string},"additionalProperties":false}),
		},
		Builtin {
			name: "skill_asset_read",
			contract: aidash_domain::tool::builtin_contract("skill_asset_read")
				.expect("declared builtin"),
			description: "Read a file packaged with a Skill by its capability alias, digest and relative path. Continue from next_offset when present. Binary files return metadata only.",
			schema: json!({"type":"object","required":["alias","digest","path"],"properties":{"alias":string,"digest":string,"path":string,"offset":{"type":"integer","minimum":0},"max_chars":{"type":"integer","minimum":1}},"additionalProperties":false}),
		},
	];
	entries
		.into_iter()
		.map(|t| (t.name.to_owned(), t))
		.collect()
}

pub mod exposure;
#[cfg(test)]
mod tests;
