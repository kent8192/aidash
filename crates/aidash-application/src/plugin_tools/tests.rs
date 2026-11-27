use super::*;
use crate::{
	ports::tools::ListFailure,
	test_support::{
		FakeSessions, Listed, http_tool, listed_tool, plugin_snapshot, plugin_snapshot_with,
		stored_plugin, stored_plugin_with_servers,
	},
};
use aidash_domain::agent_plugin::{Component, ConnectionCause, Outcome, Reason};
use serde_json::json;
use std::time::Duration;
use tokio::time::Instant;

fn servers(names: &[&str]) -> serde_json::Value {
	names
		.iter()
		.map(|name| {
			(
				(*name).to_owned(),
				json!({"type": "streamable-http", "url": format!("https://{name}.example.com/mcp")}),
			)
		})
		.collect::<serde_json::Map<_, _>>()
		.into()
}

fn plugin(name: &str, names: &[&str]) -> crate::ports::bindings::StoredAgentPlugin {
	stored_plugin_with_servers(name, servers(names), &[])
}

fn aliases(pinned: &PluginTools) -> Vec<&str> {
	pinned
		.servers
		.iter()
		.flat_map(|server| &server.tools)
		.map(|tool| tool.alias.as_str())
		.collect()
}

fn findings(pinned: &PluginTools) -> Vec<(String, Reason, String)> {
	pinned
		.findings
		.iter()
		.map(|entry| {
			let Component::Server(server) = &entry.finding.component else {
				panic!("a server finding");
			};
			(
				server.clone(),
				entry.finding.reason,
				entry.finding.detail.clone(),
			)
		})
		.collect()
}

#[tokio::test]
async fn a_listed_server_pins_its_tools_over_the_runs_session() {
	let snapshot = plugin_snapshot(vec![stored_plugin("acme", &[])], serde_json::Value::Null);
	let sessions = FakeSessions::default();
	sessions.script("docs", Listed::Tools(vec![listed_tool("search")]));
	let run = Uuid::new_v4();

	let pinned = pin_for_run(&sessions, run, &snapshot).await.unwrap();

	assert_eq!(aliases(&pinned), ["acme__docs__search"]);
	assert!(pinned.findings.is_empty());
	let listings = sessions.listings();
	assert_eq!(listings.len(), 1);
	assert_eq!(listings[0].0, run);
	assert_eq!(listings[0].1.plugin.as_str(), "acme");
	assert_eq!(listings[0].1.server.as_str(), "docs");
}

#[tokio::test]
async fn an_invalid_listed_schema_is_excluded_from_the_runs_provider_tools() {
	let snapshot = plugin_snapshot(vec![stored_plugin("acme", &[])], serde_json::Value::Null);
	let sessions = FakeSessions::default();
	let mut invalid = listed_tool("broken");
	invalid.input_schema = json!({"type": 7});
	sessions.script("docs", Listed::Tools(vec![invalid, listed_tool("search")]));

	let pinned = pin_for_run(&sessions, Uuid::new_v4(), &snapshot)
		.await
		.unwrap();
	let offered = crate::tools::agent_plugin::pinned_specifications(Some(&pinned));

	assert_eq!(
		offered.keys().map(String::as_str).collect::<Vec<_>>(),
		["acme__docs__search"]
	);
	assert_eq!(
		findings(&pinned),
		[(
			"docs".into(),
			Reason::ToolInvalid,
			"broken: schema_invalid".into()
		)]
	);
	assert_eq!(pinned.findings[0].finding.outcome, Outcome::Skipped);
}

#[tokio::test]
async fn a_server_the_plugin_serves_over_stdio_is_not_listed() {
	let plugin = stored_plugin_with_servers(
		"acme",
		json!({
			"docs": {"type": "streamable-http", "url": "https://docs.example.com/mcp"},
			"local": {"command": "node", "args": ["server.js"]},
		}),
		&[],
	);
	let snapshot = plugin_snapshot(vec![plugin], serde_json::Value::Null);
	let sessions = FakeSessions::default();
	sessions.script("docs", Listed::Tools(vec![]));

	pin_for_run(&sessions, Uuid::new_v4(), &snapshot)
		.await
		.unwrap();

	let listed: Vec<_> = sessions
		.listings()
		.into_iter()
		.map(|(_, server)| server.server.to_string())
		.collect();
	assert_eq!(listed, ["docs"]);
}

#[rstest::rstest]
#[case::refused(Listed::Forbidden, "forbidden")]
#[case::unreachable(Listed::Failed(ListFailure::Unreachable), "unreachable")]
#[case::large_listing(
	Listed::Failed(ListFailure::ListingTooLarge),
	"mcp_listing_too_large: MCP tool listing exceeds 1 MiB"
)]
#[case::initialize(Listed::Failed(ListFailure::InitializeFailed), "initialize_failed")]
#[case::answer(Listed::Failed(ListFailure::InvalidResponse), "invalid_response")]
#[case::invalid(Listed::Invalid, "invalid_response")]
#[case::shared_secret_unavailable(Listed::SecretUnavailable(None), "secret_unavailable")]
#[case::personal_secret_required(
	Listed::SecretUnavailable(Some("docs-token")),
	"secret_unavailable"
)]
#[case::secret_rejected(Listed::SecretRejected, "secret_rejected")]
#[case::leaky(
	Listed::Leaky("GET https://docs.example.com/mcp?token=hunter2 returned 500: <html>".into()),
	"unreachable"
)]
#[tokio::test]
async fn a_server_that_fails_is_left_out_with_a_typed_cause_and_nothing_else(
	#[case] listed: Listed,
	#[case] cause: &str,
) {
	let snapshot = plugin_snapshot(vec![stored_plugin("acme", &[])], serde_json::Value::Null);
	let sessions = FakeSessions::default();
	sessions.script("docs", listed);

	let pinned = pin_for_run(&sessions, Uuid::new_v4(), &snapshot)
		.await
		.unwrap();

	assert!(aliases(&pinned).is_empty());
	assert_eq!(
		findings(&pinned),
		[("docs".into(), Reason::ServerUnavailable, cause.into())]
	);
	assert_eq!(pinned.findings[0].finding.outcome, Outcome::Skipped);
}

#[tokio::test(start_paused = true)]
async fn a_server_that_never_answers_times_out_after_ten_seconds_and_the_others_still_pin() {
	let snapshot = plugin_snapshot(
		vec![plugin("acme", &["docs", "slow", "wiki"])],
		serde_json::Value::Null,
	);
	let sessions = FakeSessions::default();
	sessions
		.script("docs", Listed::Tools(vec![listed_tool("search")]))
		.script("slow", Listed::Hang)
		.script("wiki", Listed::Tools(vec![listed_tool("read")]));
	let started = Instant::now();

	let pinned = pin_for_run(&sessions, Uuid::new_v4(), &snapshot)
		.await
		.unwrap();

	assert_eq!(started.elapsed(), SERVER_TIMEOUT);
	assert_eq!(aliases(&pinned), ["acme__docs__search", "acme__wiki__read"]);
	assert_eq!(
		findings(&pinned),
		[(
			"slow".into(),
			Reason::ServerUnavailable,
			"timeout: MCP tool listing exceeded 15 seconds".into()
		)]
	);
}

#[tokio::test(start_paused = true)]
async fn all_remote_listings_start_concurrently() {
	let names: Vec<String> = (0..6).map(|index| format!("s{index}")).collect();
	let refs: Vec<&str> = names.iter().map(String::as_str).collect();
	let snapshot = plugin_snapshot(vec![plugin("acme", &refs)], serde_json::Value::Null);
	let sessions = FakeSessions::default();
	for name in &names {
		sessions.script_after(
			name,
			Duration::from_secs(1),
			Listed::Tools(vec![listed_tool("search")]),
		);
	}
	let started = Instant::now();

	let pinned = pin_for_run(&sessions, Uuid::new_v4(), &snapshot)
		.await
		.unwrap();

	assert_eq!(pinned.servers.len(), 6);
	assert_eq!(sessions.peak(), names.len());
	assert_eq!(started.elapsed(), Duration::from_secs(1));
}

#[tokio::test]
async fn aliases_of_the_runs_bindings_are_taken() {
	let colliding = http_tool(
		crate::test_support::PLUGIN_NODE,
		"taken",
		"acme__docs__search",
	);
	let snapshot = plugin_snapshot_with(
		vec![stored_plugin("acme", &[])],
		serde_json::Value::Null,
		vec![colliding],
		|root| {
			root.config["bindings"] = json!([crate::test_support::binding(
				"tool",
				crate::test_support::PLUGIN_NODE,
				"taken"
			)]);
		},
	);
	let sessions = FakeSessions::default();
	sessions.script(
		"docs",
		Listed::Tools(vec![listed_tool("search"), listed_tool("read")]),
	);

	let pinned = pin_for_run(&sessions, Uuid::new_v4(), &snapshot)
		.await
		.unwrap();

	assert_eq!(aliases(&pinned), ["acme__docs__read"]);
	assert_eq!(
		findings(&pinned),
		[("docs".into(), Reason::ReservedAlias, "search".into())]
	);
}

#[tokio::test]
async fn the_agents_choices_narrow_the_pin() {
	let snapshot = plugin_snapshot(
		vec![stored_plugin("acme", &[])],
		json!({"acme": {"docs": {"tools": ["read"]}}}),
	);
	let sessions = FakeSessions::default();
	sessions.script(
		"docs",
		Listed::Tools(vec![listed_tool("search"), listed_tool("read")]),
	);

	let pinned = pin_for_run(&sessions, Uuid::new_v4(), &snapshot)
		.await
		.unwrap();

	assert_eq!(aliases(&pinned), ["acme__docs__read"]);
}

#[tokio::test]
async fn an_agent_without_plugins_lists_nothing() {
	let snapshot = plugin_snapshot(vec![], serde_json::Value::Null);
	let sessions = FakeSessions::default();

	let pinned = pin_for_run(&sessions, Uuid::new_v4(), &snapshot)
		.await
		.unwrap();

	assert_eq!(pinned, PluginTools::default());
	assert!(sessions.listings().is_empty());
}

#[rstest::rstest]
#[case::missing(None)]
#[case::scope(Some("files.write"))]
#[tokio::test]
async fn pinning_retains_a_structured_authentication_finding_without_waiting(
	#[case] scope: Option<&str>,
) {
	let snapshot = plugin_snapshot(vec![stored_plugin("acme", &[])], serde_json::Value::Null);
	let agent = agent_bindings(&snapshot).unwrap();
	let aidash_domain::agent_plugin::McpServer::Remote(config) =
		agent.agent_plugins[0].servers.values().next().unwrap()
	else {
		panic!("remote server");
	};
	let resource =
		aidash_domain::oauth_connections::canonical_resource(config.url.as_str()).unwrap();
	let challenge = match scope {
		Some(scope) => ConnectionCause::ConnectionScopeInsufficient {
			resource: resource.clone(),
			scope: Some(scope.into()),
		},
		None => ConnectionCause::ConnectionRequired {
			resource: resource.clone(),
		},
	};
	let sessions = FakeSessions::default();
	sessions.script("docs", Listed::Connection(challenge.clone()));
	let pinned = pin_for_run(&sessions, Uuid::new_v4(), &snapshot)
		.await
		.unwrap();
	assert!(pinned.servers.is_empty());
	let stored = serde_json::to_value(&pinned).unwrap();
	assert_eq!(
		stored["findings"][0]["connection"],
		serde_json::to_value(&challenge).unwrap()
	);
	assert_eq!(pinned.findings[0].finding.reason, Reason::ServerUnavailable);
	assert_eq!(pinned.findings[0].finding.detail, challenge.code());
	assert_eq!(
		pinned.findings[0].connection.as_ref().unwrap().resource(),
		resource
	);
}

#[tokio::test]
async fn consent_relists_only_the_missing_server_and_preserves_the_other_pins() {
	let snapshot = plugin_snapshot(
		vec![plugin("acme", &["docs", "other"])],
		serde_json::Value::Null,
	);
	let sessions = FakeSessions::default();
	let resource = "https://docs.example.com/mcp";
	sessions.script(
		"docs",
		Listed::Connection(ConnectionCause::ConnectionRequired {
			resource: resource.into(),
		}),
	);
	sessions.script("other", Listed::Tools(vec![listed_tool("kept")]));
	let run = Uuid::now_v7();
	let pinned = pin_for_run(&sessions, run, &snapshot).await.unwrap();
	let kept = pinned.servers[0].clone();
	sessions.script("docs", Listed::Tools(vec![listed_tool("search")]));
	sessions.script("other", Listed::Tools(vec![listed_tool("changed")]));
	let connected = reconnect_for_run(&sessions, run, &snapshot, &pinned, resource)
		.await
		.unwrap();
	assert_eq!(connected.servers[0], kept);
	assert_eq!(
		aliases(&connected),
		["acme__other__kept", "acme__docs__search"]
	);
	assert!(connected.findings.is_empty());
	assert_eq!(
		sessions
			.listings()
			.iter()
			.filter(|(_, server)| server.server.as_str() == "other")
			.count(),
		1
	);
	assert_eq!(
		sessions
			.listings()
			.iter()
			.filter(|(_, server)| server.server.as_str() == "docs")
			.count(),
		2
	);
}

fn attach_target() -> (PluginServer, RemoteServer) {
	(
		PluginServer {
			plugin: serde_json::from_value(json!("acme")).unwrap(),
			digest: format!("sha256:{}", "a".repeat(64)),
			server: serde_json::from_value(json!("docs")).unwrap(),
		},
		serde_json::from_value(json!({"url": "https://docs.example.com/mcp"})).unwrap(),
	)
}

#[tokio::test]
async fn an_attach_time_listing_returns_names_and_the_digests_the_run_start_pin_checks() {
	let sessions = FakeSessions::default();
	let tool = listed_tool("search");
	sessions.script("docs", Listed::Tools(vec![tool.clone()]));
	let (server, config) = attach_target();

	let listing = list_for_attach(&sessions, &server, &config, None)
		.await
		.unwrap();

	assert_eq!(
		listing,
		AttachListing::Listed {
			tools: vec![AttachTool {
				name: "search".into(),
				schema_digest: agent_plugin::schema_digest(&tool.input_schema),
			}]
		}
	);
	// The listing session is its own and ends before the answer.
	let listings = sessions.listings();
	assert_eq!(listings.len(), 1);
	assert_eq!(sessions.closed(), [listings[0].0]);
}

#[rstest::rstest]
#[case::not_connected(
	Listed::Connection(ConnectionCause::ConnectionRequired {
		resource: "https://docs.example.com/mcp".into(),
	}),
	"connection_required"
)]
#[case::refused(Listed::Forbidden, "forbidden")]
#[case::failed(Listed::Failed(ListFailure::Unreachable), "unreachable")]
#[case::secret(Listed::SecretUnavailable(Some("docs-token")), "secret_unavailable")]
#[case::rejected(Listed::SecretRejected, "secret_rejected")]
#[tokio::test]
async fn an_attach_time_listing_that_fails_says_why_and_lists_nothing(
	#[case] listed: Listed,
	#[case] reason: &str,
) {
	let sessions = FakeSessions::default();
	sessions.script("docs", listed);
	let (server, config) = attach_target();

	let listing = list_for_attach(&sessions, &server, &config, None)
		.await
		.unwrap();

	assert_eq!(
		listing,
		AttachListing::Unavailable {
			reason: reason.into()
		}
	);
}

#[rstest::rstest]
#[case::not_connected(
	Error::Invalid(aidash_domain::oauth_connections::CONNECTION_REQUIRED.into()),
	"connection_required"
)]
#[case::shared_secret(
	Error::Invalid(aidash_domain::tenant_secrets::UNAVAILABLE.into()),
	"secret_unavailable"
)]
#[case::personal_secret(
	crate::tenant_secrets::personal_required("docs-token"),
	"secret_unavailable"
)]
fn an_unresolved_credential_is_a_typed_reason(#[case] error: Error, #[case] reason: &str) {
	assert_eq!(
		AttachListing::unavailable(&error),
		AttachListing::Unavailable {
			reason: reason.into()
		}
	);
}
