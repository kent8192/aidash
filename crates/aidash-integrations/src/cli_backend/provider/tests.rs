use super::*;
use crate::cli_backend::{
	config::{self, Auth},
	protocol,
};
use serde_json::json;

fn values() -> Values {
	config::parse_env("AIDASH_CLI_PROVIDER=claude\nAIDASH_CLI_AUTH_MODE=native\nAIDASH_CLI_MODEL=fixture-model-1\nAIDASH_CLI_CONTEXT_WINDOW=8192").unwrap()
}
fn config() -> Config {
	Config::from_values(values()).unwrap()
}
fn request() -> Request {
	let tools: Vec<_> = aidash_application::tools::builtins().values().map(|tool| {
		let spec=tool.specification();json!({"type":"function","function":{"name":spec.name,"description":spec.description,"parameters":spec.parameters}})
	}).collect();
	Request::parse(&serde_json::to_vec(&json!({"model":"fixture-model-1","messages":[{"role":"user","content":"Hi"}],"max_tokens":100,"tools":tools})).unwrap(),&config()).unwrap()
}
fn claude(envelope: Value) -> Vec<u8> {
	[json!({"type":"system","subtype":"init","tools":[],"model":"fixture-model-1"}),json!({"type":"assistant","message":{"model":"fixture-model-1","content":[{"type":"text","text":"answer"}],"stop_reason":"end_turn"}}),json!({"type":"result","subtype":"success","is_error":false,"structured_output":envelope,"usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":30,"output_tokens":4},"total_cost_usd":0.005})].iter().map(Value::to_string).collect::<Vec<_>>().join("\n").into_bytes()
}

#[test]
fn literal_env_and_precedence() {
	let parsed = config::parse_env(
		"# comment\nAIDASH_CLI_MODEL = 'literal-model' # comment\nCODEX_API_KEY=\n",
	)
	.unwrap();
	assert_eq!(parsed["AIDASH_CLI_MODEL"], "literal-model");
	assert_eq!(parsed["CODEX_API_KEY"], "");
	for text in [
		"AIDASH_CLI_MODLE=x",
		"export AIDASH_CLI_MODEL=x",
		"AIDASH_CLI_MODEL=$(touch /tmp/should-not-exist)",
		"AIDASH_CLI_MODEL=`id`",
		"AIDASH_CLI_MODEL=\"a\"suffix",
		"AIDASH_CLI_MODEL=a\nAIDASH_CLI_MODEL=b",
	] {
		assert!(config::parse_env(text).is_err());
	}
	let tmp = tempfile::tempdir().unwrap();
	let file = tmp.path().join("config");
	std::fs::write(&file,"AIDASH_CLI_PROVIDER=claude\nAIDASH_CLI_AUTH_MODE=native\nAIDASH_CLI_MODEL=file-model\nAIDASH_CLI_CONTEXT_WINDOW=8192").unwrap();
	let ambient = Values::from([("AIDASH_CLI_MODEL".into(), "env-model".into())]);
	let overrides = Values::from([("AIDASH_CLI_MODEL".into(), "arg-model".into())]);
	assert_eq!(
		config::load(&file, &ambient, &overrides).unwrap().model,
		"arg-model"
	);
	assert_eq!(
		config::load(&file, &ambient, &Values::new()).unwrap().model,
		"env-model"
	);
	assert_eq!(
		config::load(&file, &Values::new(), &Values::new())
			.unwrap()
			.model,
		"file-model"
	);
}

#[test]
fn required_values_modes_and_secret_environment_are_explicit() {
	for (key, value) in [
		("AIDASH_CLI_MODEL", ""),
		("AIDASH_CLI_MODEL", "REPLACE_ME"),
		("AIDASH_CLI_MODEL", "sonnet"),
		("AIDASH_CLI_PORT", "0"),
		("AIDASH_CLI_TIMEOUT_SECONDS", "no"),
		("AIDASH_CLI_CONTEXT_WINDOW", ""),
	] {
		let mut v = values();
		v.insert(key.into(), value.into());
		assert!(Config::from_values(v).is_err());
	}
	let ambient = Values::from([
		("HOME".into(), "/home/test".into()),
		("CODEX_API_KEY".into(), "fixture-secret".into()),
		("ANTHROPIC_API_KEY".into(), "foreign-secret".into()),
		("NODE_OPTIONS".into(), "--require untrusted".into()),
		("CLAUDE_CODE_OAUTH_TOKEN".into(), "other-secret".into()),
	]);
	let child = config().child_env(&ambient);
	assert_eq!(child, Values::from([("HOME".into(), "/home/test".into())]));
	for (mode, secret) in [
		("native", None),
		("access-token", Some("CODEX_ACCESS_TOKEN")),
		("access-token-login", None),
		("api-key", Some("CODEX_API_KEY")),
		("api-key-login", None),
	] {
		let mut v = values();
		v.insert("AIDASH_CLI_PROVIDER".into(), "codex".into());
		v.insert("AIDASH_CLI_AUTH_MODE".into(), mode.into());
		if let Some(key) = secret {
			assert!(Config::from_values(v.clone()).is_err());
			v.insert(key.into(), "fixture-selected-secret".into());
		}
		let c = Config::from_values(v.clone()).unwrap();
		let env = c.child_env(&ambient);
		assert!(
			!c.diagnostic()
				.to_string()
				.contains("fixture-selected-secret")
		);
		for key in ["CODEX_ACCESS_TOKEN", "CODEX_API_KEY"] {
			assert_eq!(env.contains_key(key), secret == Some(key));
		}
		if c.auth == Auth::Native {
			v.insert("CODEX_API_KEY".into(), "unexpected".into());
			assert!(Config::from_values(v).is_err());
		}
	}
}

#[test]
fn unsupported_claude_token_never_falls_back_to_saved_login() {
	let tmp = tempfile::tempdir().unwrap();
	let file = tmp.path().join("config");
	std::fs::write(&file, "AIDASH_CLI_PROVIDER=claude\nAIDASH_CLI_AUTH_MODE=native\nAIDASH_CLI_MODEL=fixture-model-1\nAIDASH_CLI_CONTEXT_WINDOW=8192").unwrap();
	let ambient = Values::from([("CLAUDE_CODE_OAUTH_TOKEN".into(), "fixture-token".into())]);
	assert!(matches!(
		config::load(&file, &ambient, &Values::new()),
		Err(Failure("claude_oauth_token_backend_unsupported"))
	));
	let mut from_file = values();
	from_file.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), "fixture-token".into());
	assert!(matches!(
		Config::from_values(from_file.clone()),
		Err(Failure("claude_oauth_token_backend_unsupported"))
	));
	from_file.insert("AIDASH_CLI_PROVIDER".into(), "codex".into());
	assert!(matches!(
		Config::from_values(from_file),
		Err(Failure("conflicting_auth_environment"))
	));
	let overrides = Values::from([("CLAUDE_CODE_OAUTH_TOKEN".into(), "fixture-token".into())]);
	assert!(matches!(
		config::load(&file, &Values::new(), &overrides),
		Err(Failure("invalid_cli_override"))
	));
	assert!(
		!config()
			.child_env(&ambient)
			.contains_key("CLAUDE_CODE_OAUTH_TOKEN")
	);
}

#[test]
fn examples_parse_and_initialization_preserves_private_settings() {
	let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/cli-backends");
	let tmp = tempfile::tempdir().unwrap();
	for profile in [
		"claude",
		"codex-native",
		"codex-access-token",
		"codex-api-key",
	] {
		let template = root.join(profile).join(".env.example");
		let dest = tmp.path().join(profile).join(".env");
		let text = std::fs::read_to_string(&template).unwrap();
		let mut v = config::parse_env(&text).unwrap();
		assert!(Config::from_values(v.clone()).is_err());
		v.insert("AIDASH_CLI_MODEL".into(), "fixture-model-1".into());
		v.insert("AIDASH_CLI_CONTEXT_WINDOW".into(), "8192".into());
		if profile == "codex-access-token" {
			v.insert("CODEX_ACCESS_TOKEN".into(), "fixture-secret".into());
		}
		if profile == "codex-api-key" {
			v.insert("CODEX_API_KEY".into(), "fixture-secret".into());
		}
		Config::from_values(v).unwrap();
		assert!(config::initialize(&template, &dest).unwrap());
		std::fs::write(&dest, "preserve private secret").unwrap();
		assert!(!config::initialize(&template, &dest).unwrap());
		assert_eq!(
			std::fs::read_to_string(&dest).unwrap(),
			"preserve private secret"
		);
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			assert_eq!(
				std::fs::metadata(dest).unwrap().permissions().mode() & 0o777,
				0o600
			);
		}
	}
}

#[test]
fn claude_usage_tools_and_terminal_states() {
	let request = request();
	let envelope = json!({"text":"","tool_calls":[{"name":"workspace_message","arguments":"{\"content\":\"done\"}"}]});
	let output = protocol::completion(
		Provider::Claude,
		&claude(envelope.clone()),
		&request,
		"fixture-model-1",
	)
	.unwrap();
	let parsed = crate::inference::parse_openai(&serde_json::to_vec(&output).unwrap()).unwrap();
	assert_eq!(parsed.reported.input_tokens, Some(60));
	assert_eq!(parsed.reported.cache_read_tokens, Some(20));
	assert!(parsed.reported.cost.is_none());
	assert_eq!(parsed.tool_calls[0].name, "workspace_message");
	for envelope in [
		json!({"text":"","tool_calls":[{"name":"unknown","arguments":"{}"}]}),
		json!({"text":"","tool_calls":[{"name":"workspace_message","arguments":"[]"}]}),
		json!({"text":"","tool_calls":[{"name":"workspace_message","arguments":"{}"}]}),
		json!({"text":"","tool_calls":[]}),
	] {
		assert!(
			protocol::completion(
				Provider::Claude,
				&claude(envelope),
				&request,
				"fixture-model-1"
			)
			.is_err()
		);
	}
	for mutation in ["native", "refusal", "partial", "model", "error"] {
		let mut events: Vec<Value> = String::from_utf8(claude(envelope.clone()))
			.unwrap()
			.lines()
			.map(|l| serde_json::from_str(l).unwrap())
			.collect();
		match mutation {
			"native" => events[0]["tools"] = json!(["Bash"]),
			"refusal" => events[2]["stop_reason"] = json!("refusal"),
			"partial" => {
				events.pop();
			}
			"model" => events[0]["model"] = json!("other"),
			_ => events[2]["is_error"] = json!(true),
		};
		assert!(
			protocol::completion(
				Provider::Claude,
				events
					.iter()
					.map(Value::to_string)
					.collect::<Vec<_>>()
					.join("\n")
					.as_bytes(),
				&request,
				"fixture-model-1"
			)
			.is_err()
		);
	}
}

#[test]
fn codex_parser_does_not_claim_isolation_or_report_an_unobserved_model() {
	let events = [
		json!({"type":"thread.started"}),
		json!({"type":"turn.started"}),
		json!({"type":"item.completed","item":{"type":"agent_message","text":"{\"text\":\"OK\",\"tool_calls\":[]}"}}),
		json!({"type":"turn.completed","usage":{"input_tokens":9,"cached_input_tokens":8,"output_tokens":2}}),
	];
	let bytes = events
		.iter()
		.map(Value::to_string)
		.collect::<Vec<_>>()
		.join("\n");
	let result = protocol::completion(
		Provider::Codex,
		bytes.as_bytes(),
		&request(),
		"fixture-model-1",
	)
	.unwrap();
	assert!(result["model"].is_null());
	assert_eq!(result["usage"]["prompt_tokens"], 9);
	assert!(result["usage"]["cost"].is_null());
	for event in [
		json!({"type":"turn.failed"}),
		json!({"type":"item.completed","item":{"type":"command_execution"}}),
		json!({"type":"item.completed","item":{"type":"web_search"}}),
	] {
		assert!(
			protocol::completion(
				Provider::Codex,
				event.to_string().as_bytes(),
				&request(),
				"fixture-model-1"
			)
			.is_err()
		);
	}
}

#[tokio::test]
async fn fixture_cli_stdin_environment_and_result() {
	let tmp = tempfile::tempdir().unwrap();
	let executable = tmp.path().join("fixture.py");
	let fixture =
		Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/cli-backends/claude.py");
	std::fs::copy(fixture, &executable).unwrap();
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
	}
	let mut c = config();
	c.executable = executable;
	c.work_dir = tmp.path().into();
	let backend = Backend::fixture(c, &std::env::vars().collect());
	let result = backend.fixture_infer(&request()).await.unwrap();
	assert_eq!(result["choices"][0]["message"]["content"], "OK");
	assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
}
