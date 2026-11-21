//! Inspect the Native messages captured by scripted Chat Completions providers.
use aidash_domain::projection::native::{Envelope, Source};
use serde_json::{Map, Value, json};

/// Extract the inputs supplied to the model and the tool results it can act on.
pub fn captured_context(body: &Value) -> Value {
	let mut snapshot = Map::new();
	let mut history = Vec::new();
	let mut calls = Map::new();
	for message in body["messages"].as_array().expect("Chat request messages") {
		if let Some(tool_calls) = message["tool_calls"].as_array() {
			for call in tool_calls {
				let id = call["id"].as_str().expect("tool call id");
				let arguments: Value = serde_json::from_str(
					call["function"]["arguments"]
						.as_str()
						.expect("tool call arguments"),
				)
				.expect("JSON tool arguments");
				calls.insert(
					id.into(),
					json!({"id":id,"name":call["function"]["name"],"arguments":arguments}),
				);
			}
		}

		if !matches!(message["role"].as_str(), Some("user" | "tool")) {
			continue;
		}
		let texts: Vec<_> = match &message["content"] {
			Value::String(text) => vec![text.as_str()],
			Value::Array(parts) => parts
				.iter()
				.filter_map(|part| part["text"].as_str())
				.collect(),
			_ => vec![],
		};
		for envelope in texts
			.into_iter()
			.filter_map(|text| serde_json::from_str::<Envelope>(text).ok())
		{
			match envelope.source {
				Source::Step | Source::Evidence | Source::Harness => {
					if let Value::Object(fields) = envelope.content {
						for (key, value) in fields {
							match (snapshot.get_mut(&key), value) {
								(Some(Value::Array(items)), Value::Array(more)) => {
									items.extend(more)
								}
								(_, value) => {
									snapshot.insert(key, value);
								}
							}
						}
					}
				}
				Source::Tool => {
					let id = envelope.provenance["invocation"]
						.as_str()
						.expect("tool invocation");
					history.push(
						json!({"kind":"tool","call":calls.get(id),"result":envelope.content}),
					);
				}
				Source::ContextSource => {
					let sources = snapshot
						.entry("context_sources")
						.or_insert_with(|| Value::Array(Vec::new()));
					sources
						.as_array_mut()
						.expect("context sources")
						.push(json!(envelope));
				}
				source => {
					let key = serde_json::to_value(source).expect("source name");
					snapshot.insert(key.as_str().unwrap().into(), envelope.content);
				}
			}
		}
	}
	json!({"snapshot":snapshot,"history":history})
}
