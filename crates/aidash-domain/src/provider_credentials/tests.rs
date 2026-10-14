use super::*;
use rstest::rstest;
#[test]
fn model_ids_match_capability_bounds_and_catalog_paths() {
	assert!(validate_model_id(&"a".repeat(256)).is_ok());
	for invalid in [
		"a".repeat(257),
		"vendor/../model".into(),
		"vendor//model".into(),
		"vendor/model?key=secret".into(),
	] {
		assert!(validate_model_id(&invalid).is_err());
	}
}
#[rstest]
#[case("https://openrouter.ai/api/v1", "openrouter", true)]
#[case("https://openrouter.ai/api/v1/", "openrouter", false)]
#[case("https://attacker.test/api/v1", "openrouter", false)]
#[case("https://openrouter.ai/api/v1", "other", false)]
fn tenant_source_is_closed_to_the_catalog_entry(
	#[case] endpoint: &str,
	#[case] provider: &str,
	#[case] valid: bool,
) {
	assert_eq!(
		validate_source(endpoint, provider, Some("openrouter")).is_ok(),
		valid
	);
}
#[test]
fn catalog_id_rejects_record_ids_and_unknown_providers() {
	assert!(Provider::parse("credential:019a0000-0000-7000-8000-000000000001").is_err());
	assert!(Provider::parse("openai").is_err());
	assert_eq!(Provider::parse("chatgpt").unwrap(), Provider::Chatgpt);
	// A keyless configuration may call any valid endpoint.
	assert!(validate_source("http://localhost:8000/v1", "openrouter", None).is_ok());
}
#[test]
fn model_and_embedding_configurations_reject_environment_keys() {
	// ADR 0006: no model-provider configuration can name an environment key.
	let model = serde_json::json!({
		"provider":"openrouter","model_id":"vendor/model","endpoint":"https://openrouter.ai/api/v1",
		"credential_env":"AIDASH_SECRET_OPENROUTER","context_window":1000,"modalities":["text"],"cost":{}
	});
	assert!(serde_json::from_value::<crate::model::ModelConfig>(model.clone()).is_err());
	let mut keyless = model;
	keyless.as_object_mut().unwrap().remove("credential_env");
	assert!(serde_json::from_value::<crate::model::ModelConfig>(keyless).is_ok());
	let embedding = serde_json::json!({
		"provider":"openai","endpoint":"http://localhost:8000/v1","credential_env":null,
		"model":"fixture","model_version":"1","dimensions":3
	});
	assert!(serde_json::from_value::<crate::semantic::EmbeddingConfig>(embedding).is_err());
}

#[test]
fn each_catalog_entry_has_one_kind_of_grant() {
	// ADR 0007: no entry falls back from one kind of grant to the other.
	assert_eq!(Provider::Openrouter.grant(), Grant::ProviderCredential);
	assert_eq!(Provider::Chatgpt.grant(), Grant::PlanConnection);
	assert_eq!(
		Provider::credential("openrouter").unwrap(),
		Provider::Openrouter
	);
	assert!(Provider::credential("chatgpt").is_err());
	assert_eq!(Provider::plan("chatgpt").unwrap(), Provider::Chatgpt);
	assert!(Provider::plan("openrouter").is_err());
	assert_eq!(Provider::Chatgpt.base_url(), "https://api.openai.com/v1");
	assert!(Provider::Chatgpt.key_settings_url("ab12").is_none());
	assert_eq!(
		Provider::Openrouter.key_settings_url("ab12").as_deref(),
		Some("https://openrouter.ai/keys/ab12")
	);
}
#[rstest]
#[case("https://api.openai.com/v1", "chatgpt", None, Some("chatgpt"), true)]
#[case(
	"https://openrouter.ai/api/v1",
	"openrouter",
	Some("openrouter"),
	None,
	true
)]
#[case("http://localhost:8000/v1", "openrouter", None, None, true)]
// A Provider Credential never serves the plan entry, nor the reverse.
#[case("https://api.openai.com/v1", "chatgpt", Some("chatgpt"), None, false)]
#[case(
	"https://openrouter.ai/api/v1",
	"openrouter",
	None,
	Some("openrouter"),
	false
)]
#[case(
	"https://api.openai.com/v1",
	"chatgpt",
	Some("openrouter"),
	Some("chatgpt"),
	false
)]
// The plan entry fixes its provider and base URL.
#[case("https://api.openai.com/v1/", "chatgpt", None, Some("chatgpt"), false)]
#[case("https://attacker.test/v1", "chatgpt", None, Some("chatgpt"), false)]
#[case("https://api.openai.com/v1", "openai", None, Some("chatgpt"), false)]
fn models_name_at_most_one_grant(
	#[case] endpoint: &str,
	#[case] provider: &str,
	#[case] credential: Option<&str>,
	#[case] plan: Option<&str>,
	#[case] valid: bool,
) {
	assert_eq!(
		validate_grants(endpoint, provider, credential, plan).is_ok(),
		valid
	);
}
