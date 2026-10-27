//! Pure Responses input serialization shared by transport and request budgets.
//!
//! This contains only model-visible input: adapters add model selection,
//! transport controls and the Tenant Cache Salt. The Native IR keeps complete
//! observable turns; provider reasoning is never retained or replayed (ADR 0083).
use super::{ContentPart, ModelRequest};
use crate::{Error, Result};
use serde_json::{Map, Value, json};

/// Every Aidash tool belongs to this Responses namespace.
pub const TOOL_NAMESPACE: &str = "aidash";

/// The serialized input with this request's actual encoded media. Transports
/// use this entry point; budgets use [`input_body`] with media placeholders.
pub fn encoded_input_body(request: &ModelRequest) -> Result<Value> {
	input_body(
		request,
		request.content_parts.iter().map(ContentPart::openrouter),
	)
}

/// The serialized input with this request's actual encoded media for
/// OpenRouter's Responses API, whose tools are flat and calls are unqualified.
pub fn encoded_openrouter_input_body(request: &ModelRequest) -> Result<Value> {
	openrouter_input_body(
		request,
		request.content_parts.iter().map(ContentPart::openrouter),
	)
}

/// Serialize the projected messages and the supplied media framing in order.
/// Runtime callers supply encoded media, while budgets supply empty payload
/// frames and reserve their provider-side media allowance separately.
pub fn input_body(request: &ModelRequest, parts: impl Iterator<Item = Value>) -> Result<Value> {
	build(request, parts, true)
}

/// OpenRouter's Responses API accepts flat function tools and carries no
/// namespace field on calls, so its transport uses this serialization. The
/// model-visible messages and media framing are otherwise identical.
pub fn openrouter_input_body(
	request: &ModelRequest,
	parts: impl Iterator<Item = Value>,
) -> Result<Value> {
	build(request, parts, false)
}

fn build(
	request: &ModelRequest,
	parts: impl Iterator<Item = Value>,
	namespaced: bool,
) -> Result<Value> {
	let chat = request.body(parts, false);
	let messages = chat["messages"]
		.as_array()
		.ok_or_else(|| Error::Invalid("model request has no messages".into()))?;
	let mut input = Vec::new();
	for message in messages.iter().skip(1) {
		match message["role"].as_str() {
			Some(role @ ("user" | "assistant")) => {
				// A call-only assistant turn has no text message. Each call is
				// still an independent input item, followed by its result.
				if !message["content"].is_null() {
					input.push(json!({"role":role,"content":content(&message["content"])?}));
				}
				for call in message["tool_calls"].as_array().into_iter().flatten() {
					let mut item = json!({
						"type":"function_call",
						"call_id":call["id"],
						"name":call["function"]["name"],
						"arguments":call["function"]["arguments"],
					});
					if namespaced {
						item["namespace"] = json!(TOOL_NAMESPACE);
					}
					input.push(item);
				}
			}
			Some("tool") => input.push(json!({
				"type":"function_call_output",
				"call_id":message["tool_call_id"],
				"output":message["content"],
			})),
			_ => return Err(Error::Invalid("unsupported Responses message role".into())),
		}
	}
	let mut body = json!({"instructions":request.instructions,"input":input});
	if !request.tools.is_empty() {
		let tools = request.tools.iter().map(|tool| {
			json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.parameters})
		}).collect::<Vec<_>>();
		body["tools"] = if namespaced {
			json!([{
				"type":"namespace",
				"name":TOOL_NAMESPACE,
				"description":"Tools of this Aidash agent.",
				"tools":tools,
			}])
		} else {
			json!(tools)
		};
	}
	if let Some(format) = &request.response_format {
		body["text"] = json!({"format":text_format(format)?});
	}
	Ok(body)
}

fn content(value: &Value) -> Result<Vec<Value>> {
	match value {
		Value::String(text) => Ok(vec![json!({"type":"input_text","text":text})]),
		Value::Array(parts) => parts
			.iter()
			.map(|part| match part["type"].as_str() {
				Some("text") => Ok(json!({"type":"input_text","text":part["text"]})),
				Some("image_url") => {
					Ok(json!({"type":"input_image","image_url":part["image_url"]["url"]}))
				}
				_ => Err(Error::Invalid("unsupported Responses input".into())),
			})
			.collect(),
		_ => Err(Error::Invalid(
			"unsupported Responses message content".into(),
		)),
	}
}

/// Chat Completions' `response_format` in the Responses `text.format` shape.
fn text_format(format: &Value) -> Result<Value> {
	match format["type"].as_str() {
		Some("json_schema") => {
			let schema = &format["json_schema"];
			let mut value = Map::new();
			value.insert("type".into(), json!("json_schema"));
			for key in ["name", "schema", "strict", "description"] {
				if let Some(field) = schema.get(key) {
					value.insert(key.into(), field.clone());
				}
			}
			Ok(Value::Object(value))
		}
		Some("json_object") => Ok(json!({"type":"json_object"})),
		Some("text") => Ok(json!({"type":"text"})),
		_ => Err(Error::Invalid("unsupported response format".into())),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::projection::native::{NativeContext, NativeMessage};
	use crate::provider::{ModelRequest, ToolCall, ToolSpec};

	fn request() -> ModelRequest {
		ModelRequest {
			instructions: "Answer".into(),
			tools: vec![ToolSpec {
				name: "search".into(),
				description: "Search".into(),
				parameters: json!({"type":"object"}),
			}],
			context: crate::provider::ModelContext::Native(NativeContext {
				attachments: vec![],
				history: vec![NativeMessage::Assistant {
					text: Some("Looking".into()),
					tool_calls: vec![ToolCall {
						id: "call-1".into(),
						name: "search".into(),
						arguments: json!({"q":"x"}),
					}],
				}],
				volatile: String::new(),
			}),
			..Default::default()
		}
	}

	#[test]
	fn plan_input_keeps_the_aidash_tool_namespace() {
		let body = input_body(&request(), std::iter::empty()).unwrap();
		assert_eq!(body["tools"][0]["type"], "namespace");
		assert_eq!(body["tools"][0]["name"], TOOL_NAMESPACE);
		assert_eq!(body["input"][1]["namespace"], TOOL_NAMESPACE);
	}

	#[test]
	fn openrouter_input_uses_flat_function_tools_and_calls() {
		let body = openrouter_input_body(&request(), std::iter::empty()).unwrap();
		assert_eq!(body["tools"][0]["type"], "function");
		assert_eq!(body["tools"][0]["name"], "search");
		assert!(body["tools"][0].get("tools").is_none());
		assert!(body["input"][1].get("namespace").is_none());
		assert_eq!(body["input"][1]["type"], "function_call");
	}
}
