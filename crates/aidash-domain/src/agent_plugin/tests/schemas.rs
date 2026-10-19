use super::*;
use crate::agent_plugin::{manifest, mcp};
use rstest::rstest;
use std::collections::BTreeSet;

const PLUGIN_SCHEMA_TEXT: &str = include_str!("../schemas/1.0.0/plugin.schema.json");
const MCP_SCHEMA_TEXT: &str = include_str!("../schemas/1.0.0/mcp.schema.json");

fn schema(text: &str) -> Value {
	serde_json::from_str(text).unwrap()
}

fn property_names(object: &Value) -> BTreeSet<String> {
	object["properties"]
		.as_object()
		.unwrap()
		.keys()
		.cloned()
		.collect()
}

fn names(keys: &[&str]) -> BTreeSet<String> {
	keys.iter().map(|key| (*key).to_owned()).collect()
}

#[test]
fn the_schema_identifiers_are_the_vendored_ones() {
	let plugin = schema(PLUGIN_SCHEMA_TEXT);
	let mcp = schema(MCP_SCHEMA_TEXT);
	assert_eq!(plugin["$id"], PLUGIN_SCHEMA);
	assert_eq!(plugin["properties"]["$schema"]["const"], PLUGIN_SCHEMA);
	assert_eq!(mcp["$id"], MCP_SCHEMA);
	assert_eq!(mcp["properties"]["$schema"]["const"], MCP_SCHEMA);
}

#[test]
fn the_schema_constants_name_the_supported_version() {
	for (constant, document) in [(PLUGIN_SCHEMA, "plugin"), (MCP_SCHEMA, "mcp")] {
		let expected = format!("{SCHEMA_PREFIX}{SUPPORTED_VERSION}/{document}.schema.json");
		assert_eq!(constant, expected);
	}
}

#[test]
fn the_manifest_keys_are_the_schemas() {
	let plugin = schema(PLUGIN_SCHEMA_TEXT);
	assert_eq!(names(&manifest::TOP_LEVEL_KEYS), property_names(&plugin));
	assert_eq!(
		names(&manifest::AUTHOR_KEYS),
		property_names(&plugin["properties"]["author"])
	);
}

#[test]
fn the_mcp_keys_are_the_schemas() {
	let mcp_schema = schema(MCP_SCHEMA_TEXT);
	assert_eq!(names(&mcp::TOP_LEVEL_KEYS), property_names(&mcp_schema));
	let defs = &mcp_schema["$defs"];
	assert_eq!(
		names(&mcp::STDIO_KEYS),
		property_names(&defs["stdioServer"])
	);
	assert_eq!(
		names(&mcp::REMOTE_KEYS),
		property_names(&defs["streamableHttpServer"])
	);
	assert_eq!(names(&mcp::REMOTE_KEYS), property_names(&defs["sseServer"]));
}

#[test]
fn the_server_variants_are_the_schemas() {
	let defs = &schema(MCP_SCHEMA_TEXT)["$defs"];
	let variants: BTreeSet<_> = ["stdioServer", "streamableHttpServer", "sseServer"]
		.iter()
		.map(|def| {
			defs[*def]["properties"]["type"]["const"]
				.as_str()
				.unwrap()
				.to_owned()
		})
		.collect();
	assert_eq!(variants, names(&["stdio", "streamable-http", "sse"]));
}

fn validator(text: &str) -> jsonschema::Validator {
	jsonschema::validator_for(&schema(text)).unwrap()
}

#[rstest]
#[case::single("a")]
#[case::hyphenated("my-plugin")]
#[case::dotted("my.plugin")]
#[case::dot_hyphen("a.-b")]
#[case::hyphen_dot("a-.b")]
#[case::digits("0a9")]
#[case::longest(&"a".repeat(64))]
#[case::too_long(&"a".repeat(65))]
#[case::empty("")]
#[case::leading_hyphen("-a")]
#[case::trailing_hyphen("a-")]
#[case::leading_dot(".a")]
#[case::trailing_dot("a.")]
#[case::double_hyphen("a--b")]
#[case::double_dot("a..b")]
#[case::uppercase("Abc")]
#[case::underscore("a_b")]
#[case::space("a b")]
#[case::unicode("café")]
#[case::trailing_newline("a\n")]
fn plugin_names_are_accepted_exactly_when_the_schema_accepts_them(#[case] name: &str) {
	let schema_accepts =
		validator(PLUGIN_SCHEMA_TEXT).is_valid(&json!({"$schema": PLUGIN_SCHEMA, "name": name}));
	assert_eq!(
		PluginName::try_from(name.to_owned()).is_ok(),
		schema_accepts
	);
}

#[rstest]
#[case::dot_slash("./", true)]
#[case::dot_slash_path("./a/b", true)]
#[case::root("${PLUGIN_ROOT}", true)]
#[case::root_path("${PLUGIN_ROOT}/a", true)]
#[case::data("${PLUGIN_DATA}", true)]
#[case::data_path("${PLUGIN_DATA}/a", true)]
#[case::data_prefix_only("${PLUGIN_DATAx}", false)]
#[case::root_prefix_only("${PLUGIN_ROOT}x", false)]
#[case::dot(".", false)]
#[case::relative("a/b", false)]
#[case::absolute("/a", false)]
#[case::shell_variable("$PLUGIN_ROOT/a", false)]
fn working_directories_are_accepted_exactly_when_the_schema_accepts_them(
	#[case] cwd: &str,
	#[case] valid: bool,
) {
	let document = json!({
		"$schema": MCP_SCHEMA,
		"mcpServers": {"srv": {"type": "stdio", "command": "node", "cwd": cwd}},
	});
	assert_eq!(validator(MCP_SCHEMA_TEXT).is_valid(&document), valid);
	let plugin = loaded(&[
		manifest_file("demo"),
		file("mcp.json", &document.to_string()),
	]);
	assert_eq!(!plugin.servers.is_empty(), valid, "{:?}", plugin.findings);
}

#[rstest]
#[case::plugin_root("PLUGIN_ROOT")]
#[case::plugin_data("PLUGIN_DATA")]
fn reserved_environment_names_are_refused_as_the_schema_refuses_them(#[case] name: &str) {
	let document = json!({
		"$schema": MCP_SCHEMA,
		"mcpServers": {"srv": {"type": "stdio", "command": "node", "env": {name: "x"}}},
	});
	assert!(!validator(MCP_SCHEMA_TEXT).is_valid(&document));
	let plugin = loaded(&[
		manifest_file("demo"),
		file("mcp.json", &document.to_string()),
	]);
	assert!(plugin.servers.is_empty());
}
