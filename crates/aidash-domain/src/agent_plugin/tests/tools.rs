use super::*;
use crate::{agent_plugin::fixtures::reference_with_servers, exposure::BindingExposure};
use rstest::rstest;

fn remote(name: &str) -> Value {
	json!({"type": "streamable-http", "url": format!("https://{name}.example.com/mcp")})
}

fn plugin_with(name: &str, servers: &[&str]) -> AgentPluginRef {
	let servers: serde_json::Map<String, Value> = servers
		.iter()
		.map(|server| ((*server).to_owned(), remote(server)))
		.collect();
	reference_with_servers(name, Value::Object(servers))
}

fn acme() -> AgentPluginRef {
	plugin_with("acme-tools", &["docs"])
}

fn schema() -> Value {
	json!({"type": "object", "properties": {"q": {"type": "string"}}})
}

fn tool(name: &str) -> ListedTool {
	ListedTool {
		name: name.to_owned(),
		description: Some(format!("Does {name}.")),
		input_schema: schema(),
	}
}

fn tools(names: &[&str]) -> Listing {
	Listing::Listed(names.iter().map(|name| tool(name)).collect())
}

fn key(plugin: &AgentPluginRef, server: &str) -> (PluginName, ServerName) {
	(plugin.name.clone(), server_name(server))
}

type Listings = BTreeMap<(PluginName, ServerName), Listing>;

fn listings(entries: &[(&AgentPluginRef, &str, Listing)]) -> Listings {
	entries
		.iter()
		.map(|(plugin, server, listing)| (key(plugin, server), listing.clone()))
		.collect()
}

fn choose(plugin: &AgentPluginRef, server: &str, settings: ServerSettings) -> PluginSettings {
	BTreeMap::from([(
		plugin.name.clone(),
		BTreeMap::from([(server_name(server), settings)]),
	)])
}

fn run(plugins: &[AgentPluginRef], settings: &PluginSettings, listed: &Listings) -> PluginTools {
	pin(plugins, settings, listed, &BTreeSet::new())
}

fn pinned_names(pinned: &PluginTools) -> Vec<(String, String, Vec<&str>)> {
	pinned
		.servers
		.iter()
		.map(|server| {
			(
				server.plugin.to_string(),
				server.server.to_string(),
				server.tools.iter().map(|tool| tool.name.as_str()).collect(),
			)
		})
		.collect()
}

fn finding(
	plugin: &AgentPluginRef,
	server: &str,
	outcome: Outcome,
	reason: Reason,
	detail: &str,
) -> PluginFinding {
	PluginFinding {
		plugin: plugin.name.clone(),
		finding: Finding {
			component: Component::Server(server.to_owned()),
			outcome,
			reason,
			detail: detail.to_owned(),
		},
	}
}

fn reasons(pinned: &PluginTools) -> Vec<Reason> {
	pinned
		.findings
		.iter()
		.map(|finding| finding.finding.reason)
		.collect()
}

#[test]
fn a_listed_server_pins_each_tool_with_its_alias_digest_and_description() {
	let plugin = acme();
	let listed = listings(&[(&plugin, "docs", tools(&["search"]))]);

	let pinned = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);

	assert!(pinned.findings.is_empty());
	assert_eq!(
		pinned.servers,
		[PinnedServer {
			plugin: plugin.name.clone(),
			digest: plugin.digest.clone(),
			server: server_name("docs"),
			tools: vec![PinnedTool {
				name: "search".into(),
				alias: "acme-tools__docs__search".into(),
				description: "Does search.".into(),
				input_schema: schema(),
				schema_digest: schema_digest(&schema()),
				read_only: false,
				eager: false,
			}],
		}]
	);
}

#[test]
fn a_tool_without_a_description_pins_an_empty_one() {
	let plugin = acme();
	let mut bare = tool("search");
	bare.description = None;
	let listed = listings(&[(&plugin, "docs", Listing::Listed(vec![bare]))]);

	let pinned = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);

	assert_eq!(pinned.servers[0].tools[0].description, "");
}

#[test]
fn the_schema_digest_ignores_how_the_server_ordered_the_keys() {
	let one = json!({"type": "object", "properties": {"a": {}, "b": {}}});
	let other: Value =
		serde_json::from_str(r#"{"properties": {"b": {}, "a": {}}, "type": "object"}"#).unwrap();

	assert_eq!(schema_digest(&one), schema_digest(&other));
	assert_ne!(schema_digest(&one), schema_digest(&schema()));
}

#[test]
fn servers_pin_in_plugin_order_then_server_key_order() {
	let first = plugin_with("zeta-tools", &["beta", "alpha"]);
	let second = plugin_with("acme-tools", &["gamma"]);
	let listed = listings(&[
		(&first, "alpha", tools(&["b", "a"])),
		(&first, "beta", tools(&["c"])),
		(&second, "gamma", tools(&["d"])),
	]);

	let pinned = run(&[first, second], &PluginSettings::new(), &listed);

	assert_eq!(
		pinned_names(&pinned),
		[
			("zeta-tools".into(), "alpha".into(), vec!["a", "b"]),
			("zeta-tools".into(), "beta".into(), vec!["c"]),
			("acme-tools".into(), "gamma".into(), vec!["d"]),
		]
	);
}

#[test]
fn the_order_a_server_lists_its_tools_in_does_not_change_the_pin() {
	let plugin = acme();
	let settings = choose(
		&plugin,
		"docs",
		ServerSettings {
			tools: Some(["gone".to_owned(), "b".to_owned()].into()),
			..Default::default()
		},
	);
	let mut invalid = tool("a-invalid");
	invalid.input_schema = json!([]);
	let forward = vec![tool("b"), tool("c"), invalid.clone(), tool("b")];
	let backward: Vec<_> = forward.iter().rev().cloned().collect();

	let one = run(
		std::slice::from_ref(&plugin),
		&settings,
		&listings(&[(&plugin, "docs", Listing::Listed(forward))]),
	);
	let other = run(
		std::slice::from_ref(&plugin),
		&settings,
		&listings(&[(&plugin, "docs", Listing::Listed(backward))]),
	);

	assert_eq!(one, other);
}

#[test]
fn a_remote_server_nobody_listed_is_skipped() {
	let plugin = acme();

	let pinned = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&Listings::new(),
	);

	assert!(pinned.servers.is_empty());
	assert_eq!(
		pinned.findings,
		[finding(
			&plugin,
			"docs",
			Outcome::Skipped,
			Reason::ServerUnavailable,
			"not listed"
		)]
	);
}

#[test]
fn an_unavailable_server_is_skipped_with_its_cause_and_the_others_still_pin() {
	let plugin = plugin_with("acme-tools", &["down", "up"]);
	let listed = listings(&[
		(&plugin, "down", Listing::Unavailable("timeout".into())),
		(&plugin, "up", tools(&["search"])),
	]);

	let pinned = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);

	assert_eq!(
		pinned_names(&pinned),
		[("acme-tools".into(), "up".into(), vec!["search"])]
	);
	assert_eq!(
		pinned.findings,
		[finding(
			&plugin,
			"down",
			Outcome::Skipped,
			Reason::ServerUnavailable,
			"timeout"
		)]
	);
}

#[test]
fn stdio_servers_are_ignored_entirely() {
	let plugin = reference_with_servers(
		"acme-tools",
		json!({"docs": remote("docs"), "local": {"type": "stdio", "command": "node"}}),
	);
	let unlisted = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listings(&[(&plugin, "docs", tools(&["a"]))]),
	);
	let listed = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listings(&[
			(&plugin, "docs", tools(&["a"])),
			(&plugin, "local", tools(&["b"])),
		]),
	);

	assert_eq!(unlisted, listed);
	assert_eq!(
		pinned_names(&listed),
		[("acme-tools".into(), "docs".into(), vec!["a"])]
	);
	assert!(listed.findings.is_empty());
}

#[test]
fn an_allowlist_keeps_only_the_tools_it_names_without_reporting_the_rest() {
	let plugin = acme();
	let settings = choose(
		&plugin,
		"docs",
		ServerSettings {
			tools: Some(["a".to_owned(), "c".to_owned()].into()),
			..Default::default()
		},
	);
	let listed = listings(&[(&plugin, "docs", tools(&["a", "b", "c"]))]);

	let pinned = run(std::slice::from_ref(&plugin), &settings, &listed);

	assert_eq!(
		pinned_names(&pinned),
		[("acme-tools".into(), "docs".into(), vec!["a", "c"])]
	);
	assert!(pinned.findings.is_empty());
}

#[test]
fn an_allowlisted_tool_the_server_did_not_list_is_reported() {
	let plugin = acme();
	let settings = choose(
		&plugin,
		"docs",
		ServerSettings {
			tools: Some(["a".to_owned(), "gone".to_owned()].into()),
			..Default::default()
		},
	);
	let listed = listings(&[(&plugin, "docs", tools(&["a"]))]);

	let pinned = run(std::slice::from_ref(&plugin), &settings, &listed);

	assert_eq!(
		pinned_names(&pinned),
		[("acme-tools".into(), "docs".into(), vec!["a"])]
	);
	assert_eq!(
		pinned.findings,
		[finding(
			&plugin,
			"docs",
			Outcome::Ignored,
			Reason::ToolNotListed,
			"gone"
		)]
	);
}

#[test]
fn a_server_that_lost_every_tool_still_appears_with_an_empty_list() {
	let plugin = acme();
	let settings = choose(
		&plugin,
		"docs",
		ServerSettings {
			tools: Some(["gone".to_owned()].into()),
			..Default::default()
		},
	);
	let listed = listings(&[(&plugin, "docs", tools(&["a"]))]);

	let pinned = run(std::slice::from_ref(&plugin), &settings, &listed);

	assert_eq!(
		pinned_names(&pinned),
		[("acme-tools".into(), "docs".into(), vec![])]
	);
}

fn schema_of_size(bytes: usize) -> Value {
	let value = json!({"a": "x".repeat(bytes - 8)});
	assert_eq!(value.to_string().len(), bytes);
	value
}

#[rstest]
#[case::schema_that_is_a_string(json!("object"), "schema_not_object")]
#[case::schema_that_is_an_array(json!([]), "schema_not_object")]
#[case::schema_that_is_null(Value::Null, "schema_not_object")]
#[case::schema_one_byte_over(schema_of_size(MAX_SCHEMA_BYTES + 1), "schema_too_large")]
fn a_tool_with_an_unusable_schema_is_skipped(#[case] input_schema: Value, #[case] why: &str) {
	let plugin = acme();
	let mut broken = tool("broken");
	broken.input_schema = input_schema;
	let listed = listings(&[(&plugin, "docs", Listing::Listed(vec![tool("fine"), broken]))]);

	let pinned = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);

	assert_eq!(
		pinned_names(&pinned),
		[("acme-tools".into(), "docs".into(), vec!["fine"])]
	);
	assert_eq!(
		pinned.findings,
		[finding(
			&plugin,
			"docs",
			Outcome::Skipped,
			Reason::ToolInvalid,
			&format!("broken: {why}")
		)]
	);
}

#[test]
fn a_schema_at_the_size_limit_is_pinned() {
	let plugin = acme();
	let mut large = tool("large");
	large.input_schema = schema_of_size(MAX_SCHEMA_BYTES);
	let listed = listings(&[(&plugin, "docs", Listing::Listed(vec![large]))]);

	let pinned = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);

	assert_eq!(
		pinned_names(&pinned),
		[("acme-tools".into(), "docs".into(), vec!["large"])]
	);
}

#[rstest]
#[case::at_the_limit(MAX_DESCRIPTION_BYTES, true)]
#[case::one_byte_over(MAX_DESCRIPTION_BYTES + 1, false)]
fn a_tool_description_is_bounded(#[case] bytes: usize, #[case] pinned: bool) {
	let plugin = acme();
	let mut described = tool("described");
	described.description = Some("d".repeat(bytes));
	let listed = listings(&[(&plugin, "docs", Listing::Listed(vec![described]))]);

	let result = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);

	assert_eq!(!result.servers[0].tools.is_empty(), pinned);
	assert_eq!(
		reasons(&result),
		if pinned {
			vec![]
		} else {
			vec![Reason::ToolInvalid]
		}
	);
}

#[rstest]
#[case::empty("")]
#[case::too_long(&"t".repeat(MAX_TOOL_NAME + 1))]
#[case::newline("a\nb")]
#[case::nul("a\0b")]
fn a_tool_with_an_unusable_name_is_skipped(#[case] name: &str) {
	let plugin = acme();
	let listed = listings(&[(&plugin, "docs", tools(&["fine", name]))]);

	let pinned = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);

	assert_eq!(
		pinned_names(&pinned),
		[("acme-tools".into(), "docs".into(), vec!["fine"])]
	);
	assert_eq!(reasons(&pinned), [Reason::ToolInvalid]);
	let detail = &pinned.findings[0].finding.detail;
	assert!(detail.chars().count() <= MAX_TOOL_NAME + ": name".len());
	assert!(!detail.chars().any(char::is_control));
}

#[test]
fn a_name_of_the_longest_length_is_pinned_under_a_bounded_alias() {
	let plugin = acme();
	let name = "t".repeat(MAX_TOOL_NAME);
	let listed = listings(&[(&plugin, "docs", tools(&[&name]))]);

	let pinned = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);

	assert_eq!(pinned.servers[0].tools[0].name, name);
	assert_eq!(pinned.servers[0].tools[0].alias.len(), 64);
}

fn numbered(count: usize) -> Vec<String> {
	(0..count).map(|n| format!("t{n:03}")).collect()
}

fn listing_of(names: &[String]) -> Listing {
	Listing::Listed(names.iter().map(|name| tool(name)).collect())
}

#[test]
fn a_server_pins_at_most_64_tools_in_name_order() {
	let plugin = acme();
	let names = numbered(MAX_SERVER_TOOLS + 3);
	let listed = listings(&[(&plugin, "docs", listing_of(&names))]);

	let pinned = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);

	let kept: Vec<_> = pinned.servers[0]
		.tools
		.iter()
		.map(|t| t.name.clone())
		.collect();
	assert_eq!(kept, names[..MAX_SERVER_TOOLS]);
	assert_eq!(
		pinned.findings,
		names[MAX_SERVER_TOOLS..]
			.iter()
			.map(|name| finding(&plugin, "docs", Outcome::Skipped, Reason::ToolLimit, name))
			.collect::<Vec<_>>()
	);
}

#[test]
fn a_run_pins_at_most_128_tools_and_a_server_over_the_limit_keeps_an_empty_entry() {
	let plugin = plugin_with("acme-tools", &["a", "b", "c", "d"]);
	let names = numbered(MAX_SERVER_TOOLS);
	let listed = listings(&[
		(&plugin, "a", listing_of(&names)),
		(&plugin, "b", listing_of(&names[..60])),
		(&plugin, "c", listing_of(&names)),
		(&plugin, "d", listing_of(&names[..1])),
	]);

	let pinned = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);

	let counts: Vec<_> = pinned
		.servers
		.iter()
		.map(|server| (server.server.to_string(), server.tools.len()))
		.collect();
	assert_eq!(
		counts,
		[("a", 64), ("b", 60), ("c", 4), ("d", 0)]
			.map(|(server, count)| (server.to_owned(), count))
	);
	assert_eq!(pinned.findings.len(), 60 + 1);
	assert!(
		reasons(&pinned)
			.iter()
			.all(|reason| *reason == Reason::ToolLimit)
	);
	assert_eq!(pinned.findings[0].finding.detail, names[4]);
}

#[test]
fn a_tool_refused_for_its_alias_does_not_use_a_slot() {
	let plugin = acme();
	let names = numbered(MAX_SERVER_TOOLS + 1);
	let taken = BTreeSet::from([model_alias(&plugin.name, &server_name("docs"), &names[0])]);
	let listed = listings(&[(&plugin, "docs", listing_of(&names))]);

	let pinned = pin(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
		&taken,
	);

	assert_eq!(pinned.servers[0].tools.len(), MAX_SERVER_TOOLS);
	assert_eq!(reasons(&pinned), [Reason::ReservedAlias]);
	assert_eq!(pinned.findings[0].finding.detail, names[0]);
}

#[test]
fn an_alias_that_is_taken_is_skipped_and_the_rest_pin() {
	let plugin = acme();
	let taken = BTreeSet::from(["acme-tools__docs__a".to_owned()]);
	let listed = listings(&[(&plugin, "docs", tools(&["a", "b"]))]);

	let pinned = pin(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
		&taken,
	);

	assert_eq!(
		pinned_names(&pinned),
		[("acme-tools".into(), "docs".into(), vec!["b"])]
	);
	assert_eq!(
		pinned.findings,
		[finding(
			&plugin,
			"docs",
			Outcome::Skipped,
			Reason::ReservedAlias,
			"a"
		)]
	);
}

#[test]
fn the_earlier_tool_keeps_an_alias_a_later_one_would_share() {
	let plugin = plugin_with("acme-tools", &["a.b", "a_b"]);
	let listed = listings(&[
		(&plugin, "a.b", tools(&["get.file", "get_file", "get_file"])),
		(&plugin, "a_b", tools(&["get_file"])),
	]);

	let pinned = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);

	assert_eq!(
		pinned_names(&pinned),
		[
			("acme-tools".into(), "a.b".into(), vec!["get.file"]),
			("acme-tools".into(), "a_b".into(), vec![]),
		]
	);
	assert_eq!(
		pinned.findings,
		[
			finding(
				&plugin,
				"a.b",
				Outcome::Skipped,
				Reason::AliasCollision,
				"get_file"
			),
			finding(
				&plugin,
				"a.b",
				Outcome::Skipped,
				Reason::AliasCollision,
				"get_file"
			),
			finding(
				&plugin,
				"a_b",
				Outcome::Skipped,
				Reason::AliasCollision,
				"get_file"
			),
		]
	);
}

fn declared(name: &str, schema_digest: &str) -> ReadOnlyTool {
	ReadOnlyTool {
		name: name.to_owned(),
		schema_digest: schema_digest.to_owned(),
	}
}

#[test]
fn a_tool_is_read_only_only_against_the_schema_it_was_declared_for() {
	let plugin = acme();
	let stale = format!("sha256:{}", "0".repeat(64));
	let settings = choose(
		&plugin,
		"docs",
		ServerSettings {
			read_only: vec![
				declared("matching", &schema_digest(&schema())),
				declared("changed", &stale),
				declared("not-listed", &schema_digest(&schema())),
			],
			..Default::default()
		},
	);
	let listed = listings(&[(
		&plugin,
		"docs",
		tools(&["changed", "matching", "undeclared"]),
	)]);

	let pinned = run(std::slice::from_ref(&plugin), &settings, &listed);

	let flags: Vec<_> = pinned.servers[0]
		.tools
		.iter()
		.map(|tool| (tool.name.as_str(), tool.read_only))
		.collect();
	assert_eq!(
		flags,
		[
			("changed", false),
			("matching", true),
			("undeclared", false)
		]
	);
	assert_eq!(
		pinned.findings,
		[finding(
			&plugin,
			"docs",
			Outcome::Ignored,
			Reason::ReadOnlyChanged,
			"changed"
		)]
	);
}

#[test]
fn a_read_only_declaration_for_a_tool_that_was_not_pinned_is_not_reported() {
	let plugin = acme();
	let settings = choose(
		&plugin,
		"docs",
		ServerSettings {
			tools: Some(["other".to_owned()].into()),
			read_only: vec![declared("skipped", &format!("sha256:{}", "0".repeat(64)))],
			..Default::default()
		},
	);
	let listed = listings(&[(&plugin, "docs", tools(&["other", "skipped"]))]);

	let pinned = run(std::slice::from_ref(&plugin), &settings, &listed);

	assert!(pinned.findings.is_empty());
}

#[rstest]
#[case::eager(Some(BindingExposure::Eager), true)]
#[case::deferred(Some(BindingExposure::Deferred), false)]
#[case::unset(None, false)]
fn the_servers_exposure_decides_whether_its_tools_are_eager(
	#[case] exposure: Option<BindingExposure>,
	#[case] eager: bool,
) {
	let plugin = plugin_with("acme-tools", &["docs", "other"]);
	let settings = choose(
		&plugin,
		"docs",
		ServerSettings {
			exposure,
			..Default::default()
		},
	);
	let listed = listings(&[
		(&plugin, "docs", tools(&["a", "b"])),
		(&plugin, "other", tools(&["c"])),
	]);

	let pinned = run(std::slice::from_ref(&plugin), &settings, &listed);

	let flags: Vec<_> = pinned
		.servers
		.iter()
		.flat_map(|server| server.tools.iter().map(|tool| tool.eager))
		.collect();
	assert_eq!(flags, [eager, eager, false]);
}

#[test]
fn pinning_twice_gives_the_same_result() {
	let plugin = plugin_with("acme-tools", &["docs", "other"]);
	let listed = listings(&[
		(&plugin, "docs", tools(&["b", "a"])),
		(&plugin, "other", Listing::Unavailable("timeout".into())),
	]);

	let first = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);
	let second = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);

	assert_eq!(first, second);
}

#[test]
fn what_a_run_pinned_survives_storage() {
	let plugin = acme();
	let listed = listings(&[(&plugin, "docs", tools(&["a", ""]))]);
	let pinned = run(
		std::slice::from_ref(&plugin),
		&PluginSettings::new(),
		&listed,
	);

	let stored = serde_json::to_value(&pinned).unwrap();

	assert_eq!(
		serde_json::from_value::<PluginTools>(stored).unwrap(),
		pinned
	);
	assert_eq!(
		serde_json::to_value(Reason::AliasCollision).unwrap(),
		json!("alias_collision")
	);
}
