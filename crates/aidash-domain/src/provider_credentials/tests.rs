use super::*;
use rstest::rstest;
#[rstest]
#[case("https://openrouter.ai/api/v1", "openrouter", None, true)]
#[case("https://openrouter.ai/api/v1/", "openrouter", None, false)]
#[case("https://attacker.test/api/v1", "openrouter", None, false)]
#[case(
	"https://openrouter.ai/api/v1",
	"openrouter",
	Some("AIDASH_SECRET_KEY"),
	false
)]
#[case("https://openrouter.ai/api/v1", "other", None, false)]
fn tenant_source_is_closed_and_excludes_environment_fallback(
	#[case] endpoint: &str,
	#[case] provider: &str,
	#[case] env: Option<&str>,
	#[case] valid: bool,
) {
	assert_eq!(
		validate_source(endpoint, provider, env, Some("openrouter")).is_ok(),
		valid
	);
}
#[test]
fn catalog_id_rejects_record_ids_and_unknown_providers() {
	assert!(Provider::parse("credential:019a0000-0000-7000-8000-000000000001").is_err());
	assert!(Provider::parse("openai").is_err());
	assert!(
		validate_source(
			"http://localhost:8000/v1",
			"openrouter",
			Some("AIDASH_SECRET_KEY"),
			None
		)
		.is_ok()
	);
}
