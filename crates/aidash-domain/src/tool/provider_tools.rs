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

#[derive(Debug, Clone, Copy)]
pub struct Declaration {
	pub id: &'static str,
	pub tool_type: &'static str,
	pub alias: &'static str,
	pub requires_responses: bool,
}

pub const TOOLS: &[Declaration] = &[
	Declaration {
		id: "provider.openrouter.web_search@1",
		tool_type: "openrouter:web_search",
		alias: "provider_web_search",
		requires_responses: false,
	},
	Declaration {
		id: "provider.openrouter.web_fetch@1",
		tool_type: "openrouter:web_fetch",
		alias: "provider_web_fetch",
		requires_responses: false,
	},
	Declaration {
		id: "provider.openrouter.datetime@1",
		tool_type: "openrouter:datetime",
		alias: "provider_datetime",
		requires_responses: false,
	},
	Declaration {
		id: "provider.openrouter.fusion@1",
		tool_type: "openrouter:fusion",
		alias: "provider_fusion",
		requires_responses: false,
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
			_ => return Err(bad()),
		};
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
	entries.extend(tools.iter().map(PinnedTool::entry));
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
		for entry in tools.as_array().ok_or_else(bad)? {
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
			pinned.validate()?;
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
