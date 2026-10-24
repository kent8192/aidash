//! Complete typed Run-state fixtures from the current development baseline.
use serde_json::{Value, json};
/// Compose a complete current tool-response fixture; callers supply only the
/// values relevant to their boundary. This is never a persisted-state converter.
#[allow(dead_code)] // Each integration target uses a subset of these complete state fixtures.
pub fn tool_call(fields: Value) -> aidash_server::domain::ToolCallState {
	let mut data = json!(aidash_server::domain::ToolCallState {
		response_epoch: 0,
		request_window: 128000,
		..Default::default()
	});
	for (key, value) in fields.as_object().expect("tool fixture fields") {
		if key == "response" {
			let mut response = json!(aidash_server::provider::ModelResponse::default());
			for (key, value) in value.as_object().expect("response fixture") {
				response[key] = value.clone();
			}
			data[key] = response;
		} else {
			data[key] = value.clone();
		}
	}
	serde_json::from_value(data).expect("current ToolCall fixture")
}
#[allow(dead_code)] // Each integration target uses a subset of these complete state fixtures.
pub fn pending(state: aidash_server::domain::RunState) -> Value {
	json!({"state_version":1,"data":json!(state)["data"],"recovery":{"retry":null,"lease_recovered":false}})
}
#[allow(dead_code)] // Each integration target uses a subset of these complete state fixtures.
pub fn context(fields: Value) -> Value {
	let mut context = json!(aidash_server::context::Context::default());
	for (key, value) in fields.as_object().expect("context fixture fields") {
		context[key] = value.clone();
	}
	context
}

#[allow(dead_code)] // Each integration target uses a subset of these complete state fixtures.
pub fn tool_pending(fields: Value) -> Value {
	pending(aidash_server::domain::RunState::ToolCall(Box::new(
		tool_call(fields),
	)))
}
