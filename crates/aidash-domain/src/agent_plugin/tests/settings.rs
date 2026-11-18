use super::*;
use crate::{
	agent_plugin::fixtures::reference_with_servers,
	exposure::{BindingExposure, ExposurePolicy},
	registry::{AgentConfig, bindings::AgentBindings},
	tool::SecretOwner,
};
use rstest::rstest;

const PLUGIN: &str = "acme-tools";
const REMOTE: &str = "docs";
const STDIO: &str = "local";

fn digest(fill: &str) -> String {
	format!("sha256:{}", fill.repeat(64))
}

fn plugin() -> AgentPluginRef {
	reference_with_servers(
		PLUGIN,
		json!({
			REMOTE: {"type": "streamable-http", "url": "https://docs.example.com/mcp"},
			STDIO: {"type": "stdio", "command": "node"},
			"legacy": {"type": "sse", "url": "https://legacy.example.com/sse"},
		}),
	)
}

fn agent(settings: PluginSettings, deferred: bool) -> AgentBindings {
	let mut agent: AgentBindings = serde_json::from_value(json!({
		"schema_version": 1,
		"model": {"id": "model", "version": "1.0.0"},
		"instructions": "Work on the Task.",
	}))
	.unwrap();
	agent.agent_plugins = vec![plugin()];
	agent.agent_plugin_settings = settings;
	if deferred {
		agent.exposure = Some(ExposurePolicy::Deferred(Default::default()));
	}
	agent
}

fn on(server: &str, settings: ServerSettings) -> PluginSettings {
	BTreeMap::from([(
		plugin_name(PLUGIN),
		BTreeMap::from([(server_name(server), settings)]),
	)])
}

fn names(count: usize) -> BTreeSet<String> {
	(0..count).map(|n| format!("tool-{n}")).collect()
}

fn read_only(name: &str, schema_digest: String) -> ReadOnlyTool {
	ReadOnlyTool {
		name: name.to_owned(),
		schema_digest,
	}
}

fn secret(name: &str, header: &str) -> ServerSecret {
	ServerSecret {
		name: name.to_owned(),
		header: header.to_owned(),
		form: None,
		owner: SecretOwner::Any,
	}
}

fn accepted(settings: PluginSettings, deferred: bool) -> bool {
	agent(settings, deferred).validate().is_ok()
}

#[test]
fn an_agent_without_settings_keeps_its_bytes() {
	let stored = serde_json::to_value(agent(PluginSettings::new(), false)).unwrap();

	assert!(stored.get("agent_plugin_settings").is_none());
	let reloaded: AgentBindings = serde_json::from_value(stored.clone()).unwrap();
	assert_eq!(serde_json::to_value(reloaded).unwrap(), stored);
}

#[test]
fn settings_that_choose_nothing_are_not_serialized() {
	let stored = serde_json::to_value(ServerSettings::default()).unwrap();

	assert_eq!(stored, json!({}));
}

#[test]
fn settings_round_trip_through_the_definition_and_the_settings_view() {
	let settings = on(
		REMOTE,
		ServerSettings {
			tools: Some(["search".to_owned()].into()),
			exposure: Some(BindingExposure::Eager),
			read_only: vec![read_only("search", digest("a"))],
			secret: Some(secret("docs-token", "X-Api-Key")),
		},
	);
	let definition = agent(settings.clone(), true);

	let stored = serde_json::to_value(&definition).unwrap();
	let config = AgentConfig::from_definition(serde_json::from_value(stored.clone()).unwrap());

	assert_eq!(
		stored["agent_plugin_settings"][PLUGIN][REMOTE],
		json!({
			"tools": ["search"],
			"exposure": "eager",
			"read_only": [{"name": "search", "schema_digest": digest("a")}],
			"secret": {"name": "docs-token", "header": "X-Api-Key"},
		})
	);
	assert_eq!(config.agent_plugin_settings, settings);
	assert_eq!(config.definition().agent_plugin_settings, settings);
	assert_eq!(serde_json::to_value(&config).unwrap(), stored);
}

#[rstest]
#[case::a_field_nobody_defined(json!({"extra": true}))]
#[case::a_read_only_declaration_without_a_digest(json!({"read_only": [{"name": "search"}]}))]
#[case::a_secret_without_a_name(json!({"secret": {"header": "Authorization"}}))]
#[case::a_secret_with_a_form_nobody_defined(json!({"secret": {"name": "token", "form": "basic"}}))]
#[case::a_read_only_declaration_with_a_field_nobody_defined(
	json!({"read_only": [{"name": "search", "schema_digest": "x", "extra": 1}]})
)]
fn stored_settings_refuse_unknown_or_missing_fields(#[case] stored: Value) {
	assert!(serde_json::from_value::<ServerSettings>(stored).is_err());
}

#[test]
fn settings_may_narrow_expose_and_declare_read_only_tools() {
	let settings = on(
		REMOTE,
		ServerSettings {
			tools: Some(names(3)),
			exposure: Some(BindingExposure::Deferred),
			read_only: vec![read_only("tool-1", digest("0"))],
			secret: None,
		},
	);

	assert!(accepted(settings, true));
}

#[test]
fn an_empty_allowlist_offers_nothing_and_is_accepted() {
	let settings = on(
		REMOTE,
		ServerSettings {
			tools: Some(BTreeSet::new()),
			..Default::default()
		},
	);

	assert!(accepted(settings, false));
}

#[test]
fn a_plugin_that_is_not_attached_is_refused() {
	let settings = BTreeMap::from([(
		plugin_name("other-tools"),
		BTreeMap::from([(server_name(REMOTE), ServerSettings::default())]),
	)]);

	assert!(!accepted(settings, false));
}

#[test]
fn a_server_the_plugin_does_not_have_is_refused() {
	assert!(!accepted(on("missing", ServerSettings::default()), false));
}

#[test]
fn a_server_the_plugin_skipped_is_refused() {
	assert!(!accepted(on("legacy", ServerSettings::default()), false));
}

#[rstest]
#[case::remote(REMOTE, true)]
#[case::stdio(STDIO, false)]
fn only_a_remote_server_declares_read_only_tools(#[case] server: &str, #[case] ok: bool) {
	let settings = on(
		server,
		ServerSettings {
			read_only: vec![read_only("search", digest("a"))],
			..Default::default()
		},
	);

	assert_eq!(accepted(settings, false), ok);
}

#[test]
fn a_stdio_server_may_still_narrow_its_tools() {
	let settings = on(
		STDIO,
		ServerSettings {
			tools: Some(names(1)),
			..Default::default()
		},
	);

	assert!(accepted(settings, false));
}

#[test]
fn a_tool_is_declared_read_only_once() {
	let settings = on(
		REMOTE,
		ServerSettings {
			read_only: vec![
				read_only("search", digest("a")),
				read_only("search", digest("b")),
			],
			..Default::default()
		},
	);

	assert!(!accepted(settings, false));
}

#[rstest]
#[case::lowercase_hex(digest("a"), true)]
#[case::uppercase(digest("A"), false)]
#[case::short(format!("sha256:{}", "a".repeat(63)), false)]
#[case::long(format!("sha256:{}", "a".repeat(65)), false)]
#[case::other_algorithm(format!("sha512:{}", "a".repeat(64)), false)]
#[case::bare("a".repeat(64), false)]
#[case::empty(String::new(), false)]
fn a_schema_digest_is_sha256_in_lowercase_hex(#[case] schema_digest: String, #[case] ok: bool) {
	let settings = on(
		REMOTE,
		ServerSettings {
			read_only: vec![read_only("search", schema_digest)],
			..Default::default()
		},
	);

	assert_eq!(accepted(settings, false), ok);
}

#[rstest]
#[case::at_the_limit(MAX_SERVER_TOOLS, true)]
#[case::past_the_limit(MAX_SERVER_TOOLS + 1, false)]
fn a_server_allows_a_bounded_allowlist(#[case] count: usize, #[case] ok: bool) {
	let settings = on(
		REMOTE,
		ServerSettings {
			tools: Some(names(count)),
			..Default::default()
		},
	);

	assert_eq!(accepted(settings, false), ok);
}

#[rstest]
#[case::at_the_limit(MAX_SERVER_TOOLS, true)]
#[case::past_the_limit(MAX_SERVER_TOOLS + 1, false)]
fn a_server_allows_a_bounded_number_of_read_only_declarations(
	#[case] count: usize,
	#[case] ok: bool,
) {
	let settings = on(
		REMOTE,
		ServerSettings {
			read_only: names(count)
				.into_iter()
				.map(|name| read_only(&name, digest("a")))
				.collect(),
			..Default::default()
		},
	);

	assert_eq!(accepted(settings, false), ok);
}

#[rstest]
#[case::plain("search", true)]
#[case::empty("", false)]
#[case::longest("t".repeat(MAX_TOOL_NAME), true)]
#[case::too_long("t".repeat(MAX_TOOL_NAME + 1), false)]
#[case::counted_in_characters_not_bytes("é".repeat(MAX_TOOL_NAME), true)]
#[case::newline("search\nfiles", false)]
#[case::nul("search\0", false)]
#[case::delete("search\u{7f}", false)]
fn tool_names_are_one_to_128_characters_without_control_characters(
	#[case] name: String,
	#[case] ok: bool,
) {
	let in_allowlist = on(
		REMOTE,
		ServerSettings {
			tools: Some([name.clone()].into()),
			..Default::default()
		},
	);
	let in_declarations = on(
		REMOTE,
		ServerSettings {
			read_only: vec![read_only(&name, digest("a"))],
			..Default::default()
		},
	);

	assert_eq!(accepted(in_allowlist, false), ok);
	assert_eq!(accepted(in_declarations, false), ok);
}

#[rstest]
#[case::legacy(false, false)]
#[case::deferred(true, true)]
fn exposure_needs_the_deferred_policy_like_a_binding_does(
	#[case] deferred: bool,
	#[case] ok: bool,
) {
	let settings = on(
		REMOTE,
		ServerSettings {
			exposure: Some(BindingExposure::Eager),
			..Default::default()
		},
	);

	assert_eq!(accepted(settings, deferred), ok);
}

#[test]
fn a_secret_defaults_to_authorization_and_any_owner() {
	let stored: ServerSettings =
		serde_json::from_value(json!({"secret": {"name": "docs-token"}})).unwrap();

	let secret = stored.secret.unwrap();
	assert_eq!(secret.header, SECRET_HEADER);
	assert_eq!(secret.owner, SecretOwner::Any);
	assert!(secret.bearer());
	assert_eq!(
		serde_json::to_value(&secret).unwrap(),
		json!({"name": "docs-token", "header": "Authorization"})
	);
}

#[rstest]
#[case::authorization("Authorization", true)]
#[case::authorization_in_any_case("aUTHORIZATION", true)]
#[case::another_header("X-Api-Key", false)]
fn without_a_form_only_authorization_carries_the_bearer_form(
	#[case] header: &str,
	#[case] bearer: bool,
) {
	assert_eq!(secret("token", header).bearer(), bearer);
}

/// ADR 0073 (2026-10-23): the definition chooses raw or Bearer form; Authorization defaults to
/// Bearer and other headers to raw. Bearer goes only in Authorization, which the broker can send.
#[rstest]
#[case::bearer_authorization("Authorization", Some(SecretForm::Bearer), true, true)]
#[case::raw_authorization("Authorization", Some(SecretForm::Raw), true, false)]
#[case::raw_authorization_in_any_case("authorization", Some(SecretForm::Raw), true, false)]
#[case::raw_api_key("X-Api-Key", Some(SecretForm::Raw), true, false)]
#[case::bearer_api_key("X-Api-Key", Some(SecretForm::Bearer), false, false)]
fn a_form_chooses_raw_anywhere_and_bearer_only_in_authorization(
	#[case] header: &str,
	#[case] form: Option<SecretForm>,
	#[case] ok: bool,
	#[case] bearer: bool,
) {
	let mut chosen = secret("docs-token", header);
	chosen.form = form;
	let settings = on(
		REMOTE,
		ServerSettings {
			secret: Some(chosen.clone()),
			..Default::default()
		},
	);

	assert_eq!(accepted(settings, false), ok);
	assert_eq!(chosen.bearer(), bearer);
}

#[test]
fn a_chosen_form_is_written_out_and_read_back() {
	let stored = json!({"name": "docs-token", "header": "Authorization", "form": "raw"});

	let secret: ServerSecret = serde_json::from_value(stored.clone()).unwrap();

	assert_eq!(secret.form, Some(SecretForm::Raw));
	assert_eq!(serde_json::to_value(&secret).unwrap(), stored);
}

#[test]
fn a_personal_secret_is_written_out() {
	let mut personal = secret("token", "Authorization");
	personal.owner = SecretOwner::Personal;

	assert_eq!(
		serde_json::to_value(&personal).unwrap(),
		json!({"name": "token", "header": "Authorization", "owner": "personal"})
	);
}

#[rstest]
#[case::remote(REMOTE, true)]
#[case::stdio(STDIO, false)]
fn only_a_remote_server_authenticates_with_a_secret(#[case] server: &str, #[case] ok: bool) {
	let settings = on(
		server,
		ServerSettings {
			secret: Some(secret("docs-token", "Authorization")),
			..Default::default()
		},
	);

	assert_eq!(accepted(settings, false), ok);
}

#[rstest]
#[case::plain("docs-token", true)]
#[case::uppercase("Docs-Token", false)]
#[case::empty("", false)]
#[case::too_long("t".repeat(65), false)]
fn a_secret_name_follows_the_tenant_secret_rule(#[case] name: String, #[case] ok: bool) {
	let settings = on(
		REMOTE,
		ServerSettings {
			secret: Some(secret(&name, "Authorization")),
			..Default::default()
		},
	);

	assert_eq!(accepted(settings, false), ok);
}

#[rstest]
#[case::authorization("Authorization", true)]
#[case::api_key("X-Api-Key", true)]
#[case::transport_owned_by_the_plugin_list("Mcp-Session-Id", false)]
#[case::accept_owned_by_the_plugin_list("Accept", false)]
#[case::content_type_owned_by_the_plugin_list("content-type", false)]
#[case::refused_by_the_broker_cookie("Cookie", false)]
#[case::refused_by_the_broker_expect("Expect", false)]
#[case::refused_by_the_broker_prefix("X-Forwarded-For", false)]
#[case::refused_by_the_broker_aidash_prefix("X-Aidash-Tenant", false)]
#[case::refused_by_the_broker_sec_prefix("Sec-Fetch-Mode", false)]
#[case::proxy_credentials("Proxy-Authorization", false)]
#[case::underscore_the_broker_cannot_send("X_Api_Key", false)]
#[case::empty("", false)]
fn a_secret_header_is_refused_when_either_rule_refuses_it(#[case] header: &str, #[case] ok: bool) {
	let settings = on(
		REMOTE,
		ServerSettings {
			secret: Some(secret("docs-token", header)),
			..Default::default()
		},
	);

	assert_eq!(accepted(settings, false), ok);
}

#[test]
fn a_secret_replaces_the_plugin_header_of_the_same_name_in_any_case() {
	let headers = vec![
		crate::agent_plugin::Header {
			name: "x-api-key".into(),
			value: "literal".into(),
		},
		crate::agent_plugin::Header {
			name: "X-Trace".into(),
			value: "on".into(),
		},
	];

	let kept = secret("docs-token", "X-API-Key").replace_in(&headers);

	assert_eq!(kept.len(), 1);
	assert_eq!(kept[0].name, "X-Trace");
}
