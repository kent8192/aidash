use aidash_domain::{
	model::ModelConfig,
	registry::{AgentConfig, Entry},
};
use aidash_integrations::cli_backend::{Failure, Result, config::Config};
use serde_json::{Value, json};
use std::{io::Write, path::Path};

pub fn definitions(
	config: &Config,
	model_entry: &str,
	agent_entry: &str,
	version: &str,
) -> Result<(Value, Value)> {
	if [model_entry, agent_entry].iter().any(|s| {
		s.is_empty()
			|| s.len() > 100
			|| !s
				.bytes()
				.all(|c| c.is_ascii_alphanumeric() || b"-._".contains(&c))
	}) || model_entry == agent_entry
		|| semver::Version::parse(version).is_err()
	{
		return Err(Failure("invalid_registry_identity"));
	}
	let model = json!({"id":model_entry,"version":version,"kind":"model","name":{"en":"Personal inference model"},"description":{"en":"Personal inference endpoint; requires Catalog Approval"},"config":{
		"provider":"openrouter","model_id":config.model,"endpoint":format!("http://127.0.0.1:{}/v1",config.port),
		"request_timeout_secs":config.timeout_seconds+65,"streaming":false,"stream_stall_timeout_secs":config.timeout_seconds+65,
		"context_window":config.context_window,"max_output_tokens":config.max_output_tokens,"modalities":["text"],"cost":{},"projection_versions":["legacy"],"cache_mode":"none"}});
	let agent = json!({"id":agent_entry,"version":version,"kind":"agent","name":{"en":"Personal inference agent"},"description":{"en":"Minimal personal inference agent; no compaction provider dependency"},"config":{
		"schema_version":1,"model":{"id":model_entry,"version":version},"instructions":"Complete the user's task using the Aidash tools when needed. Report the result briefly.",
		"bindings":[],"remove_default":["compaction"],"max_steps":8,"projection_version":"legacy"}});
	validate(&model, &agent)?;
	Ok((model, agent))
}

pub fn validate(model: &Value, agent: &Value) -> Result<()> {
	let model_entry: Entry =
		serde_json::from_value(model.clone()).map_err(|_| Failure("invalid_model_entry"))?;
	let agent_entry: Entry =
		serde_json::from_value(agent.clone()).map_err(|_| Failure("invalid_agent_entry"))?;
	let m: ModelConfig = serde_json::from_value(model["config"].clone())
		.map_err(|_| Failure("invalid_model_config"))?;
	m.validate_projection_versions()
		.map_err(|_| Failure("invalid_projection"))?;
	let _: AgentConfig = serde_json::from_value(agent["config"].clone())
		.map_err(|_| Failure("invalid_agent_config"))?;
	// The generated minimal Agent has no capability bindings. Run the same
	// admission rules as registration, including the exact local origin grant.
	struct NoCapabilities;
	impl aidash_application::ports::registry::CoreToolCatalog for NoCapabilities {
		fn specifications(
			&self,
			_: &aidash_domain::capabilities::CoreCapabilities,
		) -> std::collections::BTreeMap<String, aidash_domain::provider::ToolSpec> {
			std::collections::BTreeMap::new()
		}
	}
	let origin = m
		.endpoint
		.strip_suffix("/v1")
		.ok_or(Failure("invalid_example_endpoint"))?;
	let origins = aidash_domain::keyless::PermittedOrigins::parse(&[origin])
		.map_err(|_| Failure("invalid_example_origin"))?;
	let validation = aidash_application::registry::DefinitionValidation::new(std::sync::Arc::new(
		NoCapabilities,
	))
	.with_keyless_origins(aidash_application::provider_access::KeylessAccess::permitting(origins));
	validation
		.validate_in(&model_entry, true)
		.map_err(|_| Failure("invalid_model_admission"))?;
	validation
		.validate_in(&agent_entry, true)
		.map_err(|_| Failure("invalid_agent_admission"))?;
	Ok(())
}

pub fn render(
	config: &Config,
	destination: &Path,
	model_entry: &str,
	agent_entry: &str,
	version: &str,
) -> Result<()> {
	let (model, agent) = definitions(config, model_entry, agent_entry, version)?;
	std::fs::create_dir_all(destination).map_err(|_| Failure("example_directory_failed"))?;
	for (name, value) in [("model.json", model), ("agent.json", agent)] {
		let mut file = std::fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.open(destination.join(name))
			.map_err(|_| Failure("example_exists_or_unwritable"))?;
		file.write_all(
			serde_json::to_string_pretty(&value)
				.map_err(|_| Failure("example_serialization"))?
				.as_bytes(),
		)
		.map_err(|_| Failure("example_write_failed"))?;
	}
	println!(
		"validated model.json and agent.json written; registration and Catalog Approval remain required"
	);
	Ok(())
}

#[cfg(test)]
pub(super) mod tests {
	use super::*;
	// Called by the HTTP integration test so the committed templates, actual
	// Registry admission and Reinhardt client are verified together.
	pub fn committed_templates_match_the_real_registry_and_generated_definitions() {
		let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/cli-backends");
		for (provider, profile, port) in [
			("claude", "claude", 18190),
			("codex", "codex-native", 18191),
		] {
			let mut values = aidash_integrations::cli_backend::config::parse_env(
				&std::fs::read_to_string(root.join(profile).join(".env.example")).unwrap(),
			)
			.unwrap();
			values.insert("AIDASH_CLI_MODEL".into(), "fixture-model-1".into());
			values.insert("AIDASH_CLI_CONTEXT_WINDOW".into(), "8192".into());
			let config = Config::from_values(values).unwrap();
			assert_eq!(config.port, port);
			let (model, agent) = definitions(
				&config,
				"local.fixture-model",
				"local.fixture-agent",
				"1.0.0",
			)
			.unwrap();
			let read = |name: &str| -> Value {
				let text = std::fs::read_to_string(root.join("registry").join(name))
					.unwrap()
					.replace("__MODEL_ENTRY__", "local.fixture-model")
					.replace("__AGENT_ENTRY__", "local.fixture-agent")
					.replace("__MODEL_ID__", "fixture-model-1")
					.replace("\"__CONTEXT_WINDOW__\"", "8192");
				serde_json::from_str(&text).unwrap()
			};
			assert_eq!(read(&format!("model-{provider}.example.json")), model);
			assert_eq!(read("agent.example.json"), agent);
			validate(&model, &agent).unwrap();
			let parsed: ModelConfig = serde_json::from_value(model["config"].clone()).unwrap();
			aidash_domain::provider_credentials::validate_grants(
				&parsed.endpoint,
				&parsed.provider,
				parsed.provider_credential.as_deref(),
				parsed.plan_connection.as_deref(),
			)
			.unwrap();
			assert_eq!(agent["config"]["model"]["id"], model["id"]);
		}
	}
}
