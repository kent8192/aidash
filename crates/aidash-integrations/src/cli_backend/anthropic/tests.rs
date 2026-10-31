use super::*;
use crate::cli_backend::{
	config::{self, Values},
	provider::Backend,
};

fn values() -> Values {
	config::parse_env("AIDASH_CLI_PROVIDER=claude\nAIDASH_CLI_AUTH_MODE=oauth-token\nAIDASH_CLI_MODEL=fixture-model-1\nAIDASH_CLI_CONTEXT_WINDOW=8192\nCLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-fixture-not-real").unwrap()
}
fn request() -> Request {
	let spec = aidash_application::tools::builtins()["workspace_message"].specification();
	Request::parse(&serde_json::to_vec(&json!({"model":"fixture-model-1","max_tokens":100,"messages":[{"role":"system","content":"Aidash system"},{"role":"user","content":"Hi"}],"tools":[{"type":"function","function":spec}]})).unwrap(), &Config::from_values(values()).unwrap()).unwrap()
}
fn response() -> Value {
	json!({"type":"message","role":"assistant","model":"fixture-model-1","stop_reason":"tool_use","content":[{"type":"tool_use","id":"toolu_fixture","name":"workspace_message","input":{"content":"done"}}],"usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":30,"output_tokens":4}})
}

#[tokio::test]
async fn explicit_oauth_mode_needs_no_cli_and_never_forwards_token() {
	let config = Config::from_values(values()).unwrap();
	assert!(!config.diagnostic().to_string().contains("sk-ant-oat"));
	assert!(config.child_env(&Values::new()).is_empty());
	assert_eq!(config.diagnostic()["transport"], "anthropic-messages-api");
	Backend::checked(config, &Values::new()).await.unwrap();
	for (key, value) in [
		("CLAUDE_CODE_OAUTH_TOKEN", ""),
		("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-api03-wrong-kind"),
		("CODEX_ACCESS_TOKEN", "foreign"),
		("AIDASH_CLI_EXECUTABLE", "claude"),
		("CLAUDE_CONFIG_DIR", "/tmp/other-login"),
		("AIDASH_CLI_PROVIDER", "codex"),
	] {
		let mut v = values();
		v.insert(key.into(), value.into());
		assert!(Config::from_values(v).is_err(), "{key}");
	}
}

#[test]
fn messages_tool_results_usage_and_terminal_states() {
	let mut request = request();
	let first = messages(&request).unwrap();
	assert_eq!(first["system"][0]["text"], "Aidash system");
	assert_eq!(first["tools"][0]["name"], "workspace_message");
	assert_eq!(
		first["tools"][0]["input_schema"],
		request.body["tools"][0]["function"]["parameters"]
	);
	let result = completion(&response(), &request, "fixture-model-1").unwrap();
	let parsed = crate::inference::parse_openai(&serde_json::to_vec(&result).unwrap()).unwrap();
	assert_eq!(parsed.reported.input_tokens, Some(60));
	assert_eq!(parsed.reported.cache_read_tokens, Some(20));
	assert!(parsed.reported.cost.is_none());
	let message = result["choices"][0]["message"].clone();
	let id = message["tool_calls"][0]["id"].clone();
	request.body["messages"].as_array_mut().unwrap().extend([
		message,
		json!({"role":"tool","tool_call_id":id,"content":"executed by Aidash"}),
	]);
	let followup = messages(&request).unwrap();
	assert_eq!(followup["messages"][1]["content"][0]["type"], "tool_use");
	assert_eq!(followup["messages"][2]["content"][0]["type"], "tool_result");
	assert_eq!(followup["messages"][2]["content"][0]["tool_use_id"], id);
	for (pointer, bad) in [
		("/stop_reason", json!("refusal")),
		("/stop_reason", json!("max_tokens")),
		("/stop_reason", json!("pause_turn")),
		("/stop_reason", Value::Null),
		("/model", json!("wrong-model")),
		("/content/0/name", json!("unknown")),
		("/content/0/input", json!([])),
		("/content/0/input", json!({})),
		("/content/0/type", json!("server_tool_use")),
		("/usage/output_tokens", json!(101)),
	] {
		let mut v = response();
		*v.pointer_mut(pointer).unwrap() = bad;
		assert!(
			completion(&v, &request, "fixture-model-1").is_err(),
			"{pointer}"
		);
	}
	let mut v = response();
	v["usage"] = json!({});
	assert_eq!(
		completion(&v, &request, "fixture-model-1").unwrap()["usage"],
		json!({})
	);
	request.body["messages"].as_array_mut().unwrap().pop();
	assert!(matches!(
		messages(&request),
		Err(Failure("missing_tool_results"))
	));
}

#[tokio::test]
async fn wire_auth_errors_redirects_and_timeout_are_safe() {
	use axum::{
		Json, Router,
		http::{HeaderMap, StatusCode},
		routing::post,
	};
	for (status, expected) in [
		(401, "anthropic_token_invalid_expired_or_unsupported"),
		(403, "anthropic_token_permission_denied"),
		(404, "anthropic_model_unavailable"),
		(429, "anthropic_rate_or_usage_limit"),
		(302, "anthropic_redirect_refused"),
		(500, "anthropic_request_failed"),
	] {
		let app = Router::new().route(
			"/v1/messages",
			post(
				move |headers: HeaderMap, Json(body): Json<Value>| async move {
					assert_eq!(
						headers["authorization"],
						"Bearer sk-ant-oat01-fixture-not-real"
					);
					assert_eq!(headers["anthropic-beta"], "oauth-2025-04-20");
					assert_eq!(headers["anthropic-version"], "2023-06-01");
					assert!(!headers.contains_key("x-api-key"));
					assert!(
						headers["user-agent"]
							.to_str()
							.unwrap()
							.starts_with("aidash-")
					);
					assert_eq!(body["model"], "fixture-model-1");
					(
						StatusCode::from_u16(status).unwrap(),
						"private upstream error",
					)
				},
			),
		);
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
		let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
		let error = call(&Config::from_values(values()).unwrap(), &request(), &url)
			.await
			.unwrap_err();
		assert_eq!(error, Failure(expected));
		task.abort();
		let _ = task.await;
	}
	let app = Router::new().route(
		"/v1/messages",
		post(|| async {
			tokio::time::sleep(Duration::from_secs(10)).await;
			Json(response())
		}),
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
	let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	let mut config = Config::from_values(values()).unwrap();
	config.timeout_seconds = 1;
	assert_eq!(
		call(&config, &request(), &url).await.unwrap_err(),
		Failure("anthropic_timeout")
	);
	task.abort();
	let _ = task.await;
}
