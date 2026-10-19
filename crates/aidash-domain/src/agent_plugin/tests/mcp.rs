use super::*;
use rstest::rstest;

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

fn stdio(command: Command) -> StdioServer {
	StdioServer {
		command,
		args: Vec::new(),
		env: BTreeMap::new(),
		cwd: Cwd::PluginRoot,
	}
}

fn http(url: &str, headers: &[(&str, &str)]) -> McpServer {
	McpServer::Remote(RemoteServer {
		url: url.parse().unwrap(),
		headers: headers
			.iter()
			.map(|(name, value)| Header {
				name: (*name).into(),
				value: (*value).into(),
			})
			.collect(),
	})
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

#[rstest]
#[case::no_command(json!({"type": "stdio"}))]
#[case::empty_command(json!({"type": "stdio", "command": ""}))]
#[case::command_not_a_string(json!({"type": "stdio", "command": ["node"]}))]
#[case::args_not_an_array(json!({"type": "stdio", "command": "node", "args": "a"}))]
#[case::arg_not_a_string(json!({"type": "stdio", "command": "node", "args": ["a", 1]}))]
#[case::env_not_an_object(json!({"type": "stdio", "command": "node", "env": ["A=1"]}))]
#[case::env_value_not_a_string(json!({"type": "stdio", "command": "node", "env": {"A": 1}}))]
#[case::env_plugin_root(json!({"type": "stdio", "command": "node", "env": {"PLUGIN_ROOT": "/x"}}))]
#[case::env_plugin_data(json!({"type": "stdio", "command": "node", "env": {"A": "1", "PLUGIN_DATA": "/x"}}))]
#[case::cwd_not_a_string(json!({"type": "stdio", "command": "node", "cwd": 1}))]
#[case::cwd_relative_without_prefix(json!({"type": "stdio", "command": "node", "cwd": "bin"}))]
#[case::cwd_dot(json!({"type": "stdio", "command": "node", "cwd": "."}))]
#[case::cwd_absolute(json!({"type": "stdio", "command": "node", "cwd": "/tmp"}))]
#[case::cwd_placeholder_prefix(json!({"type": "stdio", "command": "node", "cwd": "${PLUGIN_ROOTX}"}))]
#[case::cwd_shell_variable(json!({"type": "stdio", "command": "node", "cwd": "$PLUGIN_ROOT"}))]
#[case::cwd_other_placeholder(json!({"type": "stdio", "command": "node", "cwd": "${HOME}"}))]
#[case::cwd_backslash(json!({"type": "stdio", "command": "node", "cwd": "./a\\b"}))]
#[case::cross_variant_url(json!({"type": "stdio", "command": "node", "url": "https://example.com/mcp"}))]
#[case::cross_variant_headers(json!({"type": "stdio", "command": "node", "headers": {}}))]
#[case::unknown_key(json!({"type": "stdio", "command": "node", "timeout": 5}))]
fn a_malformed_stdio_entry_is_invalid(#[case] entry: Value) {
	assert_invalid(entry);
}

#[rstest]
#[case::shell_string("npx -y @scope/pkg")]
#[case::leading_space(" node")]
#[case::trailing_space("node ")]
#[case::tab("node\tx")]
#[case::semicolon("node;ls")]
#[case::ampersand("node&")]
#[case::pipe("node|cat")]
#[case::redirect_out("node>out")]
#[case::redirect_in("node<in")]
#[case::backtick("`node`")]
#[case::dollar("$NODE")]
#[case::substitution("$(node)")]
#[case::parenthesis("(node)")]
#[case::single_quote("'node'")]
#[case::double_quote("\"node\"")]
#[case::backslash("node\\x")]
#[case::nul("node\u{0}x")]
#[case::bell("node\u{7}")]
#[case::placeholder("${PLUGIN_ROOT}/bin/server.js")]
#[case::absolute("/usr/bin/node")]
#[case::relative_without_dot("bin/server.js")]
#[case::parent_without_dot("../server.js")]
#[case::trailing_slash("./bin/")]
fn a_command_must_be_one_executable_token(#[case] command: &str) {
	assert_invalid(json!({"type": "stdio", "command": command}));
}

#[rstest]
#[case::node("node", Executable::Node)]
#[case::npm("npm", Executable::Npm)]
#[case::npx("npx", Executable::Npx)]
#[case::python3("python3", Executable::Python3)]
#[case::uv("uv", Executable::Uv)]
#[case::uvx("uvx", Executable::Uvx)]
fn the_runner_images_bare_commands_are_supported(
	#[case] name: &str,
	#[case] executable: Executable,
) {
	assert_eq!(
		the_only_server(json!({"type": "stdio", "command": name})),
		McpServer::Stdio(stdio(Command::Bare(executable)))
	);
}

#[rstest]
#[case::python("python")]
#[case::bash("bash")]
#[case::deno("deno")]
#[case::case_sensitive("Node")]
#[case::dot("..")]
fn any_other_bare_command_is_skipped_as_unsupported(#[case] name: &str) {
	assert_eq!(
		the_only_finding(json!({"type": "stdio", "command": name})),
		skipped(
			Component::Server("srv".into()),
			Reason::UnsupportedCommand,
			name
		)
	);
}

#[rstest]
#[case::simple("./bin/server.js", "bin/server.js")]
#[case::nested("./bin/tool/run.sh", "bin/tool/run.sh")]
#[case::dot_segment("./bin/./server.js", "bin/server.js")]
#[case::resolved_parent("./bin/tool/../server.js", "bin/server.js")]
#[case::double_slash("./bin//server.js", "bin/server.js")]
fn a_plugin_relative_command_is_normalized(#[case] command: &str, #[case] normalized: &str) {
	assert_eq!(
		the_only_server(json!({"type": "stdio", "command": command})),
		McpServer::Stdio(stdio(Command::PluginRelative(normalized.into())))
	);
}

#[rstest]
#[case::parent("./../server.js")]
#[case::deep_parent("./bin/../../server.js")]
#[case::only_parent("./..")]
fn a_command_that_climbs_out_of_the_plugin_is_skipped(#[case] command: &str) {
	assert_eq!(
		the_only_finding(json!({"type": "stdio", "command": command})),
		skipped(
			Component::Server("srv".into()),
			Reason::PathOutsideRoot,
			command
		)
	);
}

#[rstest]
#[case::absent("./missing.js")]
#[case::directory("./bin")]
#[case::root("./")]
#[case::case_differs("./bin/Server.js")]
#[case::normalizes_to_absent("./bin/../missing.js")]
fn a_command_that_is_not_a_file_of_the_plugin_is_skipped(#[case] command: &str) {
	assert_eq!(
		the_only_finding(json!({"type": "stdio", "command": command})),
		skipped(
			Component::Server("srv".into()),
			Reason::MissingCommand,
			command
		)
	);
}

#[test]
fn args_and_env_are_kept() {
	let server = the_only_server(json!({
		"type": "stdio",
		"command": "npx",
		"args": ["-y", "@scope/pkg@1.2.3", "--root", "${PLUGIN_ROOT}"],
		"env": {"B": "2", "A": "${PLUGIN_DATA}/x"},
	}));
	assert_eq!(
		server,
		McpServer::Stdio(StdioServer {
			args: ["-y", "@scope/pkg@1.2.3", "--root", "${PLUGIN_ROOT}"]
				.map(String::from)
				.to_vec(),
			env: BTreeMap::from([
				("A".into(), "${PLUGIN_DATA}/x".into()),
				("B".into(), "2".into())
			]),
			..stdio(Command::Bare(Executable::Npx))
		})
	);
}

#[rstest]
#[case::absent(None, Cwd::PluginRoot)]
#[case::dot_slash(Some("./"), Cwd::PluginRoot)]
#[case::dot_slash_dot(Some("./."), Cwd::PluginRoot)]
#[case::normalizes_to_root(Some("./sub/.."), Cwd::PluginRoot)]
#[case::relative(Some("./sub/dir"), Cwd::Plugin("sub/dir".into()))]
#[case::relative_normalized(Some("./sub/../other/./x/"), Cwd::Plugin("other/x".into()))]
#[case::root_placeholder(Some("${PLUGIN_ROOT}"), Cwd::PluginRoot)]
#[case::root_placeholder_slash(Some("${PLUGIN_ROOT}/"), Cwd::PluginRoot)]
#[case::root_placeholder_path(Some("${PLUGIN_ROOT}/sub/../x"), Cwd::Plugin("x".into()))]
#[case::data_placeholder(Some("${PLUGIN_DATA}"), Cwd::Data(String::new()))]
#[case::data_placeholder_slash(Some("${PLUGIN_DATA}/"), Cwd::Data(String::new()))]
#[case::data_placeholder_path(Some("${PLUGIN_DATA}/cache/./v1"), Cwd::Data("cache/v1".into()))]
fn a_working_directory_is_a_normalized_path_below_its_base(
	#[case] cwd: Option<&str>,
	#[case] expected: Cwd,
) {
	let mut entry = json!({"type": "stdio", "command": "node"});
	if let Some(cwd) = cwd {
		entry["cwd"] = json!(cwd);
	}
	assert_eq!(
		the_only_server(entry),
		McpServer::Stdio(StdioServer {
			cwd: expected,
			..stdio(Command::Bare(Executable::Node))
		})
	);
}

#[rstest]
#[case::relative("./..")]
#[case::relative_deep("./a/../../b")]
#[case::root_placeholder("${PLUGIN_ROOT}/../x")]
#[case::data_placeholder("${PLUGIN_DATA}/..")]
#[case::data_placeholder_deep("${PLUGIN_DATA}/a/../../x")]
fn a_working_directory_that_climbs_out_of_its_base_is_skipped(#[case] cwd: &str) {
	assert_eq!(
		the_only_finding(json!({"type": "stdio", "command": "node", "cwd": cwd})),
		skipped(
			Component::Server("srv".into()),
			Reason::PathOutsideRoot,
			cwd
		)
	);
}

#[rstest]
#[case::no_url(json!({"type": "streamable-http"}))]
#[case::empty_url(json!({"type": "streamable-http", "url": ""}))]
#[case::url_not_a_string(json!({"type": "streamable-http", "url": 7}))]
#[case::relative_url(json!({"type": "streamable-http", "url": "/mcp"}))]
#[case::no_scheme(json!({"type": "streamable-http", "url": "example.com/mcp"}))]
#[case::ftp(json!({"type": "streamable-http", "url": "ftp://example.com/mcp"}))]
#[case::websocket(json!({"type": "streamable-http", "url": "wss://example.com/mcp"}))]
#[case::cleartext(json!({"type": "streamable-http", "url": "http://example.com/mcp"}))]
#[case::cleartext_ip(json!({"type": "streamable-http", "url": "http://203.0.113.5/mcp"}))]
#[case::unspecified_address(json!({"type": "streamable-http", "url": "http://0.0.0.0/mcp"}))]
#[case::private_address_over_http(json!({"type": "streamable-http", "url": "http://10.0.0.1/mcp"}))]
#[case::localhost_lookalike(json!({"type": "streamable-http", "url": "http://localhost.example.com/mcp"}))]
#[case::loopback_lookalike(json!({"type": "streamable-http", "url": "http://127.0.0.1.example.com/mcp"}))]
#[case::username(json!({"type": "streamable-http", "url": "https://user@example.com/mcp"}))]
#[case::password(json!({"type": "streamable-http", "url": "https://user:secret@example.com/mcp"}))]
#[case::empty_password(json!({"type": "streamable-http", "url": "https://:@example.com/mcp"}))]
#[case::fragment(json!({"type": "streamable-http", "url": "https://example.com/mcp#frag"}))]
#[case::empty_fragment(json!({"type": "streamable-http", "url": "https://example.com/mcp#"}))]
#[case::inner_space(json!({"type": "streamable-http", "url": "https://example.com/a b"}))]
#[case::leading_space(json!({"type": "streamable-http", "url": " https://example.com/mcp"}))]
#[case::newline(json!({"type": "streamable-http", "url": "https://example.com/mcp\n"}))]
#[case::headers_not_an_object(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": []}))]
#[case::header_value_not_a_string(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X-A": 1}}))]
#[case::header_name_with_space(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X A": "1"}}))]
#[case::header_name_with_colon(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X:A": "1"}}))]
#[case::empty_header_name(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"": "1"}}))]
#[case::header_value_with_newline(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X-A": "a\nb"}}))]
#[case::header_value_with_nul(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X-A": "a\u{0}b"}}))]
#[case::header_value_with_delete(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X-A": "a\u{7f}"}}))]
#[case::duplicate_header_in_two_casings(json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"X-Key": "1", "x-key": "2"}}))]
#[case::cross_variant_command(json!({"type": "streamable-http", "url": "https://example.com/mcp", "command": "node"}))]
#[case::cross_variant_env(json!({"type": "streamable-http", "url": "https://example.com/mcp", "env": {}}))]
#[case::cross_variant_cwd(json!({"type": "streamable-http", "url": "https://example.com/mcp", "cwd": "./"}))]
fn a_malformed_remote_entry_is_invalid(#[case] entry: Value) {
	assert_invalid(entry);
}

#[rstest]
#[case::https("https://example.com/mcp", "https://example.com/mcp")]
#[case::https_port_and_query(
	"https://example.com:8443/mcp?v=1",
	"https://example.com:8443/mcp?v=1"
)]
#[case::localhost("http://localhost:8080/mcp", "http://localhost:8080/mcp")]
#[case::loopback_v4("http://127.0.0.1/mcp", "http://127.0.0.1/mcp")]
#[case::loopback_range("http://127.9.9.9:9000/", "http://127.9.9.9:9000/")]
#[case::loopback_v6("http://[::1]:3000/mcp", "http://[::1]:3000/mcp")]
#[case::loopback_mapped_v6("http://[::ffff:127.0.0.1]/mcp", "http://[::ffff:7f00:1]/mcp")]
#[case::https_to_loopback("https://localhost/mcp", "https://localhost/mcp")]
#[case::host_is_normalized("HTTPS://Example.COM", "https://example.com/")]
fn a_remote_endpoint_is_https_or_loopback(#[case] url: &str, #[case] stored: &str) {
	assert_eq!(
		the_only_server(json!({"type": "streamable-http", "url": url})),
		http(stored, &[])
	);
}

#[test]
fn headers_keep_their_casing_and_are_listed_by_name() {
	let server = the_only_server(json!({
		"type": "streamable-http",
		"url": "https://example.com/mcp",
		"headers": {"X-Tenant": "acme", "Accept-Language": "en", "x-flags": "a\tb"},
	}));
	assert_eq!(
		server,
		http(
			"https://example.com/mcp",
			&[
				("Accept-Language", "en"),
				("X-Tenant", "acme"),
				("x-flags", "a\tb")
			]
		)
	);
}

#[test]
fn a_valid_sse_entry_is_skipped_as_an_unsupported_transport() {
	assert_eq!(
		the_only_finding(
			json!({"type": "sse", "url": "https://example.com/sse", "headers": {"X-A": "1"}})
		),
		skipped(
			Component::Server("srv".into()),
			Reason::UnsupportedTransport,
			"sse"
		)
	);
}

#[rstest]
#[case::cleartext(json!({"type": "sse", "url": "http://example.com/sse"}))]
#[case::no_url(json!({"type": "sse"}))]
#[case::command(json!({"type": "sse", "url": "https://example.com/sse", "command": "node"}))]
fn an_invalid_sse_entry_is_invalid_rather_than_unsupported(#[case] entry: Value) {
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
