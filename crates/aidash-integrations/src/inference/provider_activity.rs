//! Retain observable provider-owned results, excluding private reasoning.
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn usage(value: &Value) -> BTreeMap<String, u64> {
	value
		.pointer("/usage/server_tool_use")
		.and_then(Value::as_object)
		.into_iter()
		.flatten()
		.filter_map(|(key, value)| {
			value
				.as_u64()
				.filter(|_| key.len() <= 128)
				.map(|count| (key.clone(), count))
		})
		.collect()
}

pub fn chat(message: &Value) -> Vec<Value> {
	message["annotations"]
		.as_array()
		.into_iter()
		.flatten()
		.filter(|item| item["type"] == "url_citation")
		.cloned()
		.collect()
}

pub fn item(value: &Value) -> Option<Value> {
	let kind = value["type"].as_str()?;
	if !matches!(
		kind,
		"web_search_call"
			| "web_fetch_call"
			| "datetime_call"
			| "fusion_call"
			| "openrouter:web_search"
			| "openrouter:web_fetch"
			| "openrouter:datetime"
			| "openrouter:fusion"
	) && !(matches!(
		kind,
		"server_tool_call" | "server_tool_result" | "function_call"
	) && aidash_domain::tool::provider_tools::declaration(
		value["name"].as_str().unwrap_or_default(),
	)
	.is_some())
	{
		return None;
	}
	let mut result = json!({});
	for key in [
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
	] {
		if let Some(value) = value.get(key) {
			result[key] = value.clone();
		}
	}
	Some(observable(result))
}

pub fn responses(response: &Value) -> Vec<Value> {
	let calls: std::collections::BTreeSet<_> = response["output"]
		.as_array()
		.into_iter()
		.flatten()
		.filter(|item| {
			item["type"] == "function_call"
				&& aidash_domain::tool::provider_tools::declaration(
					item["name"].as_str().unwrap_or_default(),
				)
				.is_some()
		})
		.filter_map(|item| item["call_id"].as_str())
		.collect();
	let mut activity = Vec::new();
	for value in response["output"].as_array().into_iter().flatten() {
		if value["type"] == "function_call_output"
			&& value["call_id"]
				.as_str()
				.is_some_and(|id| calls.contains(id))
		{
			activity.push(observable(value.clone()));
		} else if let Some(item) = item(value) {
			activity.push(item);
		}
		if value["type"] == "message" {
			for part in value["content"].as_array().into_iter().flatten() {
				activity.extend(chat(part));
			}
		}
	}
	activity
}

fn observable(mut value: Value) -> Value {
	match &mut value {
		Value::Object(object) => {
			for key in ["reasoning", "reasoning_details", "encrypted_content"] {
				object.remove(key);
			}
			// Responses function outputs may encode a tool result as JSON text.
			// Inspect that payload before retaining it, just like object results.
			for key in ["output", "result"] {
				if let Some(text) = object.get(key).and_then(Value::as_str)
					&& let Ok(parsed @ (Value::Object(_) | Value::Array(_))) =
						serde_json::from_str::<Value>(text)
				{
					object.insert(key.into(), parsed);
				}
			}
			for child in object.values_mut() {
				*child = observable(std::mem::take(child));
			}
		}
		Value::Array(items) => {
			for item in items {
				*item = observable(std::mem::take(item));
			}
		}
		_ => {}
	}
	value
}
