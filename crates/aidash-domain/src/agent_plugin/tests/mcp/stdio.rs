use super::*;
use rstest::rstest;

fn stdio(command: Command) -> StdioServer {
	StdioServer {
		command,
		args: Vec::new(),
		env: BTreeMap::new(),
		cwd: Cwd::PluginRoot,
	}
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
