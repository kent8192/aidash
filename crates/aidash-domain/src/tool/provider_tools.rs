//! OpenRouter-owned tools. Their outputs never establish Research evidence.
use crate::{Error, Result, model::ModelConfig};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const MAX_TOOL_CALLS: u32 = 8;
pub const MAX_RESULTS: u64 = 10;
pub const MAX_TOTAL_RESULTS: u64 = 30;
pub const MAX_ANALYSIS_MODELS: usize = 4;
pub const MAX_COMPLETION_TOKENS: u64 = 8192;
pub const MAX_INNER_TOOL_CALLS: u64 = 4;
pub const MAX_CONTENT_TOKENS: u64 = 16384;
pub const MAX_NETWORK_HOSTS: usize = 32;
/// OpenRouter clamps model-generated shell arguments to these limits.
pub const SHELL_TIMEOUT_MS: u64 = 300_000;
pub const SHELL_COMMANDS: u64 = 100;
pub const SHELL_OUTPUT_LENGTH: u64 = 65_536;

#[derive(Debug, Clone, Copy)]
pub struct Declaration {
	pub id: &'static str,
	pub tool_type: &'static str,
	pub alias: &'static str,
	pub requires_responses: bool,
	pub warning: bool,
}

pub const TOOLS: &[Declaration] = &[
	Declaration {
		id: "provider.openrouter.web_search@1",
		tool_type: "openrouter:web_search",
		alias: "provider_web_search",
		requires_responses: false,
		warning: false,
	},
	Declaration {
		id: "provider.openrouter.web_fetch@1",
		tool_type: "openrouter:web_fetch",
		alias: "provider_web_fetch",
		requires_responses: false,
		warning: false,
	},
	Declaration {
		id: "provider.openrouter.datetime@1",
		tool_type: "openrouter:datetime",
		alias: "provider_datetime",
		requires_responses: false,
		warning: false,
	},
	Declaration {
		id: "provider.openrouter.fusion@1",
		tool_type: "openrouter:fusion",
		alias: "provider_fusion",
		requires_responses: false,
		warning: false,
	},
	Declaration {
		id: "provider.openrouter.advisor@1",
		tool_type: "openrouter:advisor",
		alias: "provider_advisor",
		requires_responses: true,
		warning: true,
	},
	Declaration {
		id: "provider.openrouter.subagent@1",
		tool_type: "openrouter:subagent",
		alias: "provider_subagent",
		requires_responses: true,
		warning: true,
	},
	Declaration {
		id: "provider.openrouter.experimental__search_models@1",
		tool_type: "openrouter:experimental__search_models",
		alias: "provider_search_models",
		requires_responses: true,
		warning: true,
	},
	Declaration {
		id: "provider.openrouter.tool_search@1",
		tool_type: "openrouter:tool_search",
		alias: "provider_tool_search",
		requires_responses: true,
		warning: true,
	},
	Declaration {
		id: "provider.openrouter.shell@1",
		tool_type: "openrouter:shell",
		alias: "provider_shell",
		requires_responses: true,
		warning: true,
	},
];

pub fn declaration(id: &str) -> Option<&'static Declaration> {
	TOOLS
		.iter()
		.find(|tool| tool.id == id || tool.tool_type == id)
}

/// Binding-owned configuration, copied into the immutable Binding snapshot.
/// No default is deserialized: an author must explicitly pin every spending cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Parameters {
	pub max_tool_calls: u32,
	pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PinnedTool {
	#[serde(rename = "type")]
	pub tool_type: String,
	pub binding: Parameters,
}

impl Declaration {
	pub fn defaults(&self) -> Parameters {
		Parameters {
			max_tool_calls: 3,
			parameters: match self.tool_type {
				"openrouter:web_search" => {
					json!({"engine":"exa","max_results":5,"max_total_results":15})
				}
				"openrouter:web_fetch" => json!({"engine":"openrouter","max_content_tokens":8192}),
				"openrouter:datetime" => json!({"timezone":"UTC"}),
				"openrouter:advisor" => {
					json!({"model":"", "max_completion_tokens":4096, "forward_transcript":false, "stream":false})
				}
				"openrouter:subagent" => {
					json!({"model":"", "max_completion_tokens":4096, "max_tool_calls":2, "tools":[], "inherit_functions":false})
				}
				"openrouter:experimental__search_models" | "openrouter:tool_search" => {
					json!({"max_results":5})
				}
				"openrouter:shell" => {
					json!({"engine":"openrouter","environment":{"type":"container_reference","network_policy":{"type":"disabled"}}})
				}
				// A panel and analyst must be chosen explicitly, never an upstream preset.
				_ => {
					json!({"analysis_models":[],"model":"","max_tool_calls":2,"max_completion_tokens":4096})
				}
			},
		}
	}

	pub fn validate(&self, binding: &Parameters) -> Result<()> {
		let bad = || {
			Error::Invalid(format!(
				"PROVIDER_TOOL_PARAMETERS: {} requires pinned parameters and caps within Aidash limits",
				self.tool_type
			))
		};
		if binding.max_tool_calls == 0 || binding.max_tool_calls > MAX_TOOL_CALLS {
			return Err(bad());
		}
		let params = binding.parameters.as_object().ok_or_else(bad)?;
		let bounded = |key: &str, max: u64| {
			params
				.get(key)
				.and_then(Value::as_u64)
				.is_some_and(|v| v > 0 && v <= max)
		};
		let fields: &[&str] = match self.tool_type {
			"openrouter:web_search" => {
				if !bounded("max_results", MAX_RESULTS)
					|| !bounded("max_total_results", MAX_TOTAL_RESULTS)
					|| params["max_total_results"].as_u64() < params["max_results"].as_u64()
					|| !matches!(
						params.get("engine").and_then(Value::as_str),
						Some("exa" | "parallel" | "perplexity")
					) {
					return Err(bad());
				}
				&[
					"engine",
					"max_results",
					"max_total_results",
					"allowed_domains",
					"excluded_domains",
				]
			}
			"openrouter:web_fetch" => {
				if !bounded("max_content_tokens", MAX_CONTENT_TOKENS)
					|| !matches!(
						params.get("engine").and_then(Value::as_str),
						Some("openrouter" | "exa" | "parallel")
					) {
					return Err(bad());
				}
				&[
					"engine",
					"max_content_tokens",
					"allowed_domains",
					"blocked_domains",
				]
			}
			"openrouter:datetime" => {
				if !params
					.get("timezone")
					.and_then(Value::as_str)
					.is_some_and(|s| {
						!s.is_empty()
							&& s.len() <= 128 && s
							.bytes()
							.all(|c| c.is_ascii_alphanumeric() || b"/_+-".contains(&c))
					}) {
					return Err(bad());
				}
				&["timezone"]
			}
			"openrouter:fusion" => {
				let models = params
					.get("analysis_models")
					.and_then(Value::as_array)
					.ok_or_else(bad)?;
				if models.is_empty()
					|| models.len() > MAX_ANALYSIS_MODELS
					|| !models.iter().all(model_id)
					|| !params.get("model").is_some_and(model_id)
					|| !bounded("max_tool_calls", MAX_INNER_TOOL_CALLS)
					|| !bounded("max_completion_tokens", MAX_COMPLETION_TOKENS)
				{
					return Err(bad());
				}
				&[
					"analysis_models",
					"model",
					"max_tool_calls",
					"max_completion_tokens",
				]
			}
			"openrouter:advisor" => {
				if !params.get("model").is_some_and(model_id)
					|| !bounded("max_completion_tokens", MAX_COMPLETION_TOKENS)
					|| params
						.get("forward_transcript")
						.and_then(Value::as_bool)
						.is_none() || params.get("stream") != Some(&json!(false))
				{
					return Err(bad());
				}
				&[
					"model",
					"max_completion_tokens",
					"forward_transcript",
					"stream",
					"instructions",
				]
			}
			"openrouter:subagent" => {
				if !params.get("model").is_some_and(model_id)
					|| !bounded("max_completion_tokens", MAX_COMPLETION_TOKENS)
					|| !bounded("max_tool_calls", MAX_INNER_TOOL_CALLS)
					|| params.get("tools") != Some(&json!([]))
					|| params.get("inherit_functions") != Some(&json!(false))
				{
					return Err(bad());
				}
				&[
					"model",
					"max_completion_tokens",
					"max_tool_calls",
					"tools",
					"inherit_functions",
					"instructions",
				]
			}
			"openrouter:experimental__search_models" | "openrouter:tool_search" => {
				if !bounded("max_results", MAX_RESULTS) {
					return Err(bad());
				}
				&["max_results"]
			}
			"openrouter:shell" => {
				let env = params
					.get("environment")
					.and_then(Value::as_object)
					.ok_or_else(bad)?;
				if params.get("engine") != Some(&json!("openrouter"))
					|| env.get("type") != Some(&json!("container_reference"))
					|| env
						.keys()
						.any(|key| !["type", "network_policy"].contains(&key.as_str()))
				{
					return Err(bad());
				}
				validate_network_policy(env.get("network_policy").ok_or_else(bad)?)?;
				&["engine", "environment"]
			}
			_ => return Err(bad()),
		};
		if let Some(instructions) = params.get("instructions") {
			if !instructions.as_str().is_some_and(|s| s.len() <= 8192) {
				return Err(bad());
			}
		}
		if params.keys().any(|key| !fields.contains(&key.as_str())) {
			return Err(bad());
		}
		for key in ["allowed_domains", "excluded_domains", "blocked_domains"] {
			if let Some(domains) = params.get(key) {
				let domains = domains.as_array().ok_or_else(bad)?;
				if domains.is_empty()
					|| domains.len() > 32
					|| !domains.iter().all(|domain| {
						domain.as_str().is_some_and(|s| {
							!s.is_empty()
								&& s.len() <= 253 && s
								.bytes()
								.all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c))
						})
					}) {
					return Err(bad());
				}
			}
		}
		if params.contains_key("allowed_domains") && params.contains_key("excluded_domains") {
			return Err(bad());
		}
		Ok(())
	}

	pub fn eligible(&self, model: &ModelConfig) -> Result<()> {
		reject_implicit_tools(&model.model_id)?;
		if model.provider != "openrouter"
			|| model.provider_credential.as_deref() != Some("openrouter")
			|| model.uses_plan_connection()
			|| self.requires_responses && model.wire_api != Some(crate::model::WireApi::Responses)
		{
			return Err(Error::Invalid("PROVIDER_TOOL_MODEL: Provider tools require an OpenRouter Run with a Tenant Provider Credential and compatible wire API".into()));
		}
		Ok(())
	}
}

fn model_id(value: &Value) -> bool {
	value.as_str().is_some_and(|s| {
		s.len() <= 200
			&& s.contains('/')
			&& !s.starts_with("openrouter/")
			&& s.bytes()
				.all(|c| c.is_ascii_alphanumeric() || b"/._-~".contains(&c))
	})
}

impl PinnedTool {
	pub fn validate(&self) -> Result<()> {
		declaration(&self.tool_type)
			.filter(|d| d.tool_type == self.tool_type)
			.ok_or_else(|| Error::Invalid("unknown Provider tool".into()))?
			.validate(&self.binding)
	}
	pub fn entry(&self) -> Value {
		json!({"type":self.tool_type,"parameters":self.binding.parameters})
	}
	pub fn digest(&self) -> String {
		crate::registry::rules::digest(&self.binding.parameters)
	}
}

/// Add only immutable server-tool entries; the strictest bound controls the shared loop.
pub fn append(body: &mut Value, tools: &[PinnedTool]) {
	if tools.is_empty() {
		return;
	}
	let entries = body
		.as_object_mut()
		.expect("request object")
		.entry("tools")
		.or_insert_with(|| json!([]))
		.as_array_mut()
		.expect("tools array");
	if tools
		.iter()
		.any(|tool| tool.tool_type == "openrouter:tool_search")
	{
		for entry in entries
			.iter_mut()
			.filter(|entry| entry["type"] == "function")
		{
			entry["defer_loading"] = json!(true);
		}
	}
	for tool in tools
		.iter()
		.filter(|tool| tool.tool_type == "openrouter:advisor")
	{
		entries.insert(0, tool.entry());
	}
	entries.extend(
		tools
			.iter()
			.filter(|tool| tool.tool_type != "openrouter:advisor")
			.map(PinnedTool::entry),
	);
	body["max_tool_calls"] = json!(
		tools
			.iter()
			.map(|t| t.binding.max_tool_calls)
			.min()
			.expect("nonempty")
	);
}

#[cfg(test)]
mod tests;

/// Shared Self-hosted and broker boundary. Unknown controls cannot override pinned caps.
pub fn authorize<'a>(
	body: &Value,
	signed: impl IntoIterator<Item = (&'a str, &'a str, u32)>,
) -> Result<()> {
	reject_implicit_tools(body["model"].as_str().unwrap_or_default())?;
	let bad = || {
		Error::Invalid("PROVIDER_TOOL_AUTHORITY: request differs from its signed Binding".into())
	};
	if [
		"plugins",
		"web_search_options",
		"x_search_filter",
		"preset",
		"deferred_tools",
		"stop_server_tools_when",
	]
	.iter()
	.any(|key| body.get(key).is_some())
	{
		return Err(bad());
	}
	let signed: std::collections::BTreeMap<_, _> = signed
		.into_iter()
		.map(|(kind, digest, calls)| (kind, (digest, calls)))
		.collect();
	let mut seen = std::collections::BTreeSet::new();
	if let Some(tools) = body.get("tools") {
		let entries = tools.as_array().ok_or_else(bad)?;
		if entries
			.iter()
			.position(|entry| entry["type"] == "openrouter:advisor")
			.is_some_and(|index| index != 0)
		{
			return Err(bad());
		}
		for entry in entries {
			let kind = entry["type"].as_str().ok_or_else(bad)?;
			if kind == "function" {
				continue;
			}
			let (digest, calls) = signed.get(kind).ok_or_else(bad)?;
			if !seen.insert(kind)
				|| entry.as_object().is_none_or(|object| {
					object
						.keys()
						.any(|key| key != "type" && key != "parameters")
				}) {
				return Err(bad());
			}
			let pinned = PinnedTool {
				tool_type: kind.into(),
				binding: Parameters {
					max_tool_calls: *calls,
					parameters: entry["parameters"].clone(),
				},
			};
			let mut authored = pinned.clone();
			if kind == "openrouter:shell" {
				let id = authored.binding.parameters["environment"]
					.as_object_mut()
					.and_then(|env| env.remove("container_id"))
					.ok_or_else(bad)?;
				if !id.as_str().is_some_and(valid_container_id) {
					return Err(bad());
				}
			}
			authored.validate()?;
			if pinned.digest() != *digest {
				return Err(bad());
			}
		}
	}
	if seen.is_empty() {
		if body.get("max_tool_calls").is_some() {
			return Err(bad());
		}
	} else if body
		.get("max_tool_calls")
		.and_then(Value::as_u64)
		.is_none_or(|calls| calls == 0 || seen.iter().any(|kind| calls > u64::from(signed[kind].1)))
	{
		return Err(bad());
	}
	Ok(())
}

impl Declaration {
	/// Aidash's closed Binding schema, independent of the model's arguments.
	pub fn binding_schema(&self) -> Value {
		let integer = |max| json!({"type":"integer","minimum":1,"maximum":max});
		let model = json!({"type":"string","minLength":3,"maxLength":200,"pattern":"^[a-zA-Z0-9_~.-]+/[a-zA-Z0-9_~./-]+$"});
		let domains = json!({"type":"array","minItems":1,"maxItems":32,"items":{"type":"string","minLength":1,"maxLength":253,"pattern":"^[a-zA-Z0-9.-]+$"}});
		let hosts = json!({"type":"array","minItems":1,"maxItems":MAX_NETWORK_HOSTS,"items":{"type":"string","minLength":1,"maxLength":253,"pattern":"^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$"}});
		let (mut properties, required) = match self.tool_type {
			"openrouter:web_search" => (
				json!({"engine":{"enum":["exa","parallel","perplexity"]},"max_results":integer(MAX_RESULTS),"max_total_results":integer(MAX_TOTAL_RESULTS),"allowed_domains":domains,"excluded_domains":domains}),
				json!(["engine", "max_results", "max_total_results"]),
			),
			"openrouter:web_fetch" => (
				json!({"engine":{"enum":["openrouter","exa","parallel"]},"max_content_tokens":integer(MAX_CONTENT_TOKENS),"allowed_domains":domains,"blocked_domains":domains}),
				json!(["engine", "max_content_tokens"]),
			),
			"openrouter:datetime" => (
				json!({"timezone":{"type":"string","minLength":1,"maxLength":128}}),
				json!(["timezone"]),
			),
			"openrouter:advisor" => (
				json!({"model":model,"max_completion_tokens":integer(MAX_COMPLETION_TOKENS),"forward_transcript":{"type":"boolean"},"stream":{"const":false},"instructions":{"type":"string","maxLength":8192}}),
				json!([
					"model",
					"max_completion_tokens",
					"forward_transcript",
					"stream"
				]),
			),
			"openrouter:subagent" => (
				json!({"model":model,"max_completion_tokens":integer(MAX_COMPLETION_TOKENS),"max_tool_calls":integer(MAX_INNER_TOOL_CALLS),"tools":{"const":[]},"inherit_functions":{"const":false},"instructions":{"type":"string","maxLength":8192}}),
				json!([
					"model",
					"max_completion_tokens",
					"max_tool_calls",
					"tools",
					"inherit_functions"
				]),
			),
			"openrouter:experimental__search_models" | "openrouter:tool_search" => (
				json!({"max_results":integer(MAX_RESULTS)}),
				json!(["max_results"]),
			),
			"openrouter:shell" => (
				json!({"engine":{"const":"openrouter"},"environment":{"type":"object","additionalProperties":false,"required":["type","network_policy"],"properties":{"type":{"const":"container_reference"},"network_policy":{"oneOf":[{"type":"object","additionalProperties":false,"required":["type"],"properties":{"type":{"const":"disabled"}}},{"type":"object","additionalProperties":false,"required":["type","allowed_domains"],"properties":{"type":{"const":"allowlist"},"allowed_domains":hosts}}]}}}}),
				json!(["engine", "environment"]),
			),
			_ => (
				json!({"analysis_models":{"type":"array","minItems":1,"maxItems":MAX_ANALYSIS_MODELS,"items":model},"model":model,"max_tool_calls":integer(MAX_INNER_TOOL_CALLS),"max_completion_tokens":integer(MAX_COMPLETION_TOKENS)}),
				json!([
					"analysis_models",
					"model",
					"max_tool_calls",
					"max_completion_tokens"
				]),
			),
		};
		for (key, value) in self
			.defaults()
			.parameters
			.as_object()
			.expect("defaults object")
		{
			if let Some(property) = properties.get_mut(key) {
				property["default"] = value.clone();
			}
		}
		json!({"type":"object","additionalProperties":false,"required":["max_tool_calls","parameters"],"properties":{"max_tool_calls":integer(u64::from(MAX_TOOL_CALLS)),"parameters":{"type":"object","additionalProperties":false,"required":required,"properties":properties}}})
	}
}

/// Read-only Context Journal inspection, independent of the lossy projection.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ProviderToolActivity")]
pub struct Activity {
	pub seq: u64,
	pub epoch: i64,
	pub items: Vec<Value>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub preview: Option<String>,
}
impl Activity {
	/// Inspection pages bound large results; the journal retains the full original.
	pub fn inspect(seq: u64, epoch: i64, items: Vec<Value>) -> Self {
		let serialized = serde_json::to_string(&items).expect("serializable activity");
		if serialized.len() > 65536 {
			let end = serialized.floor_char_boundary(4096);
			Self {
				seq,
				epoch,
				items: vec![],
				preview: Some(serialized[..end].into()),
			}
		} else {
			Self {
				seq,
				epoch,
				items,
				preview: None,
			}
		}
	}
}

/// Legacy model controls would add unapproved tools after request admission.
pub fn reject_implicit_tools(model: &str) -> Result<()> {
	if model.trim().starts_with("@preset/")
		|| model
			.split(':')
			.skip(1)
			.any(|suffix| suffix.eq_ignore_ascii_case("online"))
	{
		return Err(Error::Invalid("IMPLICIT_PROVIDER_TOOL: use an approved Provider tool Binding instead of :online or a preset".into()));
	}
	Ok(())
}

/// A custom id is persisted before any request; arbitrary upstream container ids are never authored.
pub fn valid_container_id(id: &str) -> bool {
	!id.is_empty()
		&& id.len() <= 40
		&& id
			.bytes()
			.all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}

pub fn validate_network_policy(policy: &Value) -> Result<()> {
	let bad = || {
		Error::Invalid(
			"PROVIDER_TOOL_NETWORK: use disabled or a bounded exact-host allowlist".into(),
		)
	};
	let object = policy.as_object().ok_or_else(bad)?;
	match policy["type"].as_str() {
		Some("disabled") if object.len() == 1 => Ok(()),
		Some("allowlist") if object.len() == 2 => {
			let hosts = policy["allowed_domains"].as_array().ok_or_else(bad)?;
			if hosts.is_empty()
				|| hosts.len() > MAX_NETWORK_HOSTS
				|| !hosts.iter().all(|v| {
					v.as_str().is_some_and(|host| {
						!host.is_empty()
							&& host.len() <= 253 && host.split('.').all(|label| {
							!label.is_empty()
								&& !label.starts_with('-')
								&& !label.ends_with('-') && label.bytes().all(|c| {
								(c.is_ascii_lowercase() || c.is_ascii_digit()) || c == b'-'
							})
						})
					})
				}) {
				return Err(bad());
			}
			Ok(())
		}
		_ => Err(bad()),
	}
}

/// Closed exact system identities; tags or descriptions cannot bypass acknowledgment.
pub fn warning_reference(id: &str, version: &str) -> bool {
	version == "1.0.0"
		&& TOOLS
			.iter()
			.any(|d| d.warning && d.id.strip_suffix("@1") == Some(id))
}

/// The exact disclosure recorded on the approval revision. No runtime consent is added.
pub fn warning_acknowledgment(id: &str, version: &str) -> Option<String> {
	if !warning_reference(id, version) {
		return None;
	}
	let mut text = "OpenRouter executes this tool outside Aidash's Execution Pod, gVisor sandbox and Egress gateway. Aidash sandbox protections and the restriction on access to Tenant Secrets do not apply there. Aidash does not provision Tenant Secrets to it. Outputs are not Research evidence. The Agent Binding fixes spending caps; provider activity and billed usage are recorded.".to_owned();
	if id.ends_with("advisor") || id.ends_with("subagent") {
		text.push_str(" The child model runs outside Aidash's delegation chain and per-agent budgets. Only the pinned model and completion budget apply; subagent function inheritance and nested tools are disabled.");
	}
	if id.ends_with("shell") {
		text.push_str(" Aidash creates at most one OpenRouter container per Run, lazily, and reuses it. Containers are not deleted by Aidash. OpenRouter sleeps them after five idle minutes; files persist at OpenRouter up to 30 days after last use. There are no Aidash sandbox protections. Produced files are only listed and never imported into the Working Area. Network is disabled unless the Binding pins an allowlist. Spending is bounded by max_tool_calls (Aidash maximum 8), OpenRouter clamps of 100 commands per call, 300000 ms per command and 65536 characters per output stream, and the model request timeout. Sandbox seconds are billed only while a request runs, with OpenRouter's minimum charge for a new or sleeping sandbox.");
	}
	Some(text)
}

/// Observable Responses history types; accepting history never grants a new tool entry.
pub fn replayable(kind: &str) -> bool {
	matches!(
		kind,
		"openrouter:advisor"
			| "openrouter:subagent"
			| "openrouter:shell"
			| "shell_call"
			| "shell_call_output"
			| "openrouter:tool_search"
			| "tool_search_call"
			| "tool_search_output"
			| "openrouter:experimental__search_models"
	)
}

/// The only top-level keys of an observable provider item. The inference
/// adapter keeps exactly these, and a replayed item may carry nothing else.
pub const REPLAY_KEYS: &[&str] = &[
	"type",
	"id",
	"call_id",
	"name",
	"status",
	"action",
	"arguments",
	"output",
	"result",
	"results",
	"url",
	"title",
	"content",
	"analysis",
	"responses",
	"instance_name",
	"model",
	"advice",
	"prompt",
	"task_name",
	"task_description",
	"outcome",
	"container_id",
	"files",
	"models",
	"total_results",
	"showing",
	"tools",
	"error",
	"execution",
];

const REPLAY_DEPTH: usize = 8;
const REPLAY_FORBIDDEN_KEYS: &[&str] = &[
	"image_url",
	"input_image",
	"input_file",
	"input_audio",
	"file_data",
	"file_url",
	"video_url",
];

/// Whether a replayed provider item could have come out of the inference
/// adapter: closed top-level keys, bounded depth, no media or remote
/// references, no nested server tool that would widen the signed tool set, and
/// every `container_id` equal to the Run's signed container (any well-formed id
/// when the request carries no shell tool). The broker applies this to client
/// input it cannot otherwise attribute.
pub fn replay_item_is_closed(item: &Value, container: Option<&str>) -> bool {
	fn nested(value: &Value, depth: usize, container: Option<&str>) -> bool {
		if depth > REPLAY_DEPTH {
			return false;
		}
		match value {
			Value::Array(items) => items.iter().all(|item| nested(item, depth + 1, container)),
			Value::Object(map) => map.iter().all(|(key, value)| {
				if REPLAY_FORBIDDEN_KEYS.contains(&key.as_str()) {
					return false;
				}
				if key == "container_id" {
					return value.as_str().is_some_and(|id| {
						valid_container_id(id) && container.is_none_or(|signed| signed == id)
					});
				}
				if key == "type"
					&& value.as_str().is_some_and(|kind| {
						kind.starts_with("openrouter:")
							|| matches!(kind, "input_image" | "input_file" | "input_audio")
					}) {
					return false;
				}
				nested(value, depth + 1, container)
			}),
			_ => true,
		}
	}
	let Some(map) = item.as_object() else {
		return false;
	};
	map.keys().all(|key| REPLAY_KEYS.contains(&key.as_str()))
		&& map
			.iter()
			.filter(|(key, _)| key.as_str() != "type")
			.all(|(key, value)| {
				if key == "container_id" {
					return value.as_str().is_some_and(|id| {
						valid_container_id(id) && container.is_none_or(|signed| signed == id)
					});
				}
				nested(value, 1, container)
			})
}
