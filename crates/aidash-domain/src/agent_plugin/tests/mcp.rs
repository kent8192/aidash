use super::*;
use rstest::rstest;

mod remote;
mod stdio;

fn tree_with(servers: Value) -> Vec<PluginFile> {
	vec![
		manifest_file("demo"),
		mcp_file(servers),
		file("bin/server.js", "// server"),
		file("bin/tool/run.sh", "# run"),
	]
}

fn load_entry(entry: Value) -> LoadedPlugin {
	loaded(&tree_with(json!({"srv": entry})))
}

fn the_only_server(entry: Value) -> McpServer {
	let plugin = load_entry(entry);
	assert_eq!(plugin.findings, []);
	assert_eq!(plugin.servers.len(), 1);
	plugin.servers.into_values().next().unwrap()
}

fn the_only_finding(entry: Value) -> Finding {
	let plugin = load_entry(entry);
	assert_eq!(plugin.servers, BTreeMap::new());
	assert_eq!(plugin.findings.len(), 1, "{:?}", plugin.findings);
	plugin.findings.into_iter().next().unwrap()
}

fn assert_invalid(entry: Value) {
	let finding = the_only_finding(entry.clone());
	assert_eq!(
		(finding.component, finding.outcome, finding.reason),
		(
			Component::Server("srv".into()),
			Outcome::Skipped,
			Reason::ServerInvalid
		),
		"{entry}"
	);
}

fn kinds(findings: &[Finding]) -> Vec<(Component, Outcome, Reason)> {
	findings
		.iter()
		.map(|finding| (finding.component.clone(), finding.outcome, finding.reason))
		.collect()
}

#[test]
fn a_plugin_without_mcp_json_has_no_servers_and_no_findings() {
	let plugin = loaded(&[manifest_file("demo")]);
	assert_eq!(plugin.servers, BTreeMap::new());
	assert_eq!(plugin.findings, []);
}

#[test]
fn an_empty_server_object_is_valid() {
	let plugin = loaded(&tree_with(json!({})));
	assert_eq!((plugin.servers, plugin.findings), (BTreeMap::new(), vec![]));
}

#[rstest]
#[case::array(json!([]))]
#[case::string(json!("mcp"))]
#[case::missing_schema(json!({"mcpServers": {}}))]
#[case::schema_not_a_string(json!({"$schema": 1, "mcpServers": {}}))]
#[case::foreign_schema(json!({"$schema": "https://example.com/mcp.schema.json", "mcpServers": {}}))]
#[case::plugin_schema(json!({"$schema": PLUGIN_SCHEMA, "mcpServers": {}}))]
#[case::missing_servers(json!({"$schema": MCP_SCHEMA}))]
#[case::servers_not_an_object(json!({"$schema": MCP_SCHEMA, "mcpServers": []}))]
#[case::extra_top_level_key(json!({"$schema": MCP_SCHEMA, "mcpServers": {}, "servers": {}}))]
fn an_invalid_mcp_json_disables_mcp_but_not_skills(#[case] document: Value) {
	let files = [
		manifest_file("demo"),
		skill("alpha"),
		file("mcp.json", &document.to_string()),
	];
	let plugin = loaded(&files);
	assert_eq!(plugin.servers, BTreeMap::new());
	assert_eq!(
		kinds(&plugin.findings),
		[(Component::Mcp, Outcome::Skipped, Reason::McpInvalid)]
	);
	assert_eq!(plugin.skills.len(), 1);
}

#[test]
fn unreadable_mcp_json_disables_mcp() {
	for bytes in [b"{".as_slice(), b"\xff\xfe".as_slice(), b"".as_slice()] {
		let files = [
			manifest_file("demo"),
			PluginFile {
				path: "mcp.json".into(),
				bytes: bytes.to_vec(),
			},
		];
		let plugin = loaded(&files);
		assert_eq!(plugin.servers, BTreeMap::new());
		assert_eq!(plugin.findings.len(), 1);
		assert_eq!(plugin.findings[0].reason, Reason::McpInvalid);
	}
}

#[test]
fn a_version_mismatch_disables_mcp_but_not_skills() {
	let document = json!({
		"$schema": "https://agent-plugins.org/schemas/2.0.0/mcp.schema.json",
		"mcpServers": {"srv": {"type": "stdio", "command": "node"}, "other": 3},
	});
	let files = [
		manifest_file("demo"),
		skill("alpha"),
		file("mcp.json", &document.to_string()),
	];
	let plugin = loaded(&files);
	assert_eq!(plugin.servers, BTreeMap::new());
	assert_eq!(
		plugin.findings,
		[skipped(Component::Mcp, Reason::McpVersionMismatch, "2.0.0")]
	);
	assert_eq!(plugin.skills.len(), 1);
}

#[rstest]
#[case::space("bad name")]
#[case::slash("a/b")]
#[case::empty("")]
#[case::colon("a:b")]
#[case::unicode("サーバー")]
#[case::too_long(&"a".repeat(65))]
fn a_key_that_is_not_a_server_name_skips_the_entry(#[case] key: &str) {
	let plugin = loaded(&tree_with(
		json!({ key: {"type": "stdio", "command": "node"} }),
	));
	assert_eq!(plugin.servers, BTreeMap::new());
	assert_eq!(
		plugin.findings,
		[skipped(
			Component::Server(key.into()),
			Reason::ServerName,
			key
		)]
	);
}

#[rstest]
#[case::mixed("Srv_1.a-b")]
#[case::longest(&"a".repeat(64))]
fn valid_server_names_load(#[case] key: &str) {
	let plugin = loaded(&tree_with(
		json!({ key: {"type": "stdio", "command": "node"} }),
	));
	assert_eq!(plugin.servers.keys().next().unwrap().as_str(), key);
}

#[rstest]
#[case::not_an_object(json!("node"))]
#[case::null(json!(null))]
#[case::no_type(json!({"command": "node"}))]
#[case::type_not_a_string(json!({"type": 1, "command": "node"}))]
#[case::unknown_type(json!({"type": "websocket", "url": "https://example.com/mcp"}))]
#[case::http_alias(json!({"type": "http", "url": "https://example.com/mcp"}))]
fn an_entry_without_a_known_type_is_invalid(#[case] entry: Value) {
	assert_invalid(entry);
}

#[test]
fn one_bad_entry_does_not_stop_the_others_and_findings_follow_key_order() {
	let plugin = loaded(&tree_with(json!({
		"zulu": {"type": "sse", "url": "https://example.com/sse"},
		"alpha": {"type": "stdio", "command": "uvx", "args": ["tool"]},
		"mike": {"type": "stdio", "command": "bash"},
		"echo": {"type": "streamable-http", "url": "https://example.com/mcp"},
		"bad key": {"type": "stdio", "command": "node"},
		"delta": 5,
	})));
	assert_eq!(
		plugin
			.servers
			.keys()
			.map(ServerName::as_str)
			.collect::<Vec<_>>(),
		["alpha", "echo"]
	);
	let skip = |key: &str, reason| (Component::Server(key.into()), Outcome::Skipped, reason);
	assert_eq!(
		kinds(&plugin.findings),
		[
			skip("bad key", Reason::ServerName),
			skip("delta", Reason::ServerInvalid),
			skip("mike", Reason::UnsupportedCommand),
			skip("zulu", Reason::UnsupportedTransport),
		]
	);
}
