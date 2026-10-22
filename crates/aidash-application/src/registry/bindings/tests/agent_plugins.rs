use super::*;
use crate::test_support::{PLUGIN_SOURCE, plugin_reference, stored_plugin};
use aidash_domain::{
	agent_plugin::PinnedAgentPlugin,
	registry::{AgentConfig, bindings::BindingOrigin},
};
use rstest::rstest;

fn take_in(catalog: &mut Catalog, config: &mut AgentBindings, plugin: StoredAgentPlugin) {
	config.agent_plugins.push(plugin_reference(&plugin));
	catalog.plugins.insert(plugin.plugin.digest.clone(), plugin);
}

fn exposure(policy: &str) -> Option<Value> {
	(policy == "deferred@1").then(|| json!({"version": "deferred@1"}))
}

fn origin(snapshot: &BindingSnapshot, operation: &str) -> BindingOrigin {
	snapshot
		.bindings
		.iter()
		.find(|binding| binding.identity == QualifiedRef::builtin(NODE, operation))
		.unwrap_or_else(|| panic!("{operation} is not bound"))
		.origin
}

#[rstest]
#[case::legacy("legacy@1", &["skill_list", "skill_load", "skill_read"])]
#[case::deferred("deferred@1", &["skill_asset_read"])]
#[tokio::test]
async fn plugin_skills_are_pinned_and_bring_the_skill_support_tools(
	#[case] policy: &str,
	#[case] support: &[&str],
) {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	config.exposure = exposure(policy).map(|value| serde_json::from_value(value).unwrap());
	let plugin = stored_plugin(
		"acme-tools",
		&[("lint", "Lint it."), ("deploy", "Ship it.")],
	);
	take_in(&mut catalog, &mut config, plugin.clone());

	let snapshot = snapshot(&mut catalog, &config, false).await.unwrap();

	let skills: Vec<_> = plugin
		.plugin
		.skills
		.iter()
		.map(|skill| skill.attachment(&plugin.plugin.digest, PLUGIN_SOURCE))
		.collect();
	assert_eq!(
		snapshot.agent_plugins,
		[PinnedAgentPlugin {
			digest: plugin.plugin.digest.clone(),
			skills: skills.clone(),
		}]
	);
	for operation in support {
		assert_eq!(origin(&snapshot, operation), BindingOrigin::SkillSupport);
	}
	let settings = AgentConfig::from_snapshot(&snapshot).unwrap();
	assert_eq!(
		serde_json::to_value(&settings.skill_attachments).unwrap(),
		serde_json::to_value(&skills).unwrap()
	);
	assert!(settings.core_capabilities.skills);
}

#[tokio::test]
async fn an_agent_without_plugins_pins_none() {
	let mut catalog = Catalog::new();

	let snapshot = snapshot(&mut catalog, &agent_config(), false)
		.await
		.unwrap();

	assert!(snapshot.agent_plugins.is_empty());
	assert!(
		serde_json::to_value(&snapshot)
			.unwrap()
			.get("agent_plugins")
			.is_none()
	);
}

#[tokio::test]
async fn plugin_skills_stand_in_for_instructions() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	config.instructions.clear();
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
	take_in(
		&mut catalog,
		&mut config,
		stored_plugin("acme-tools", &[("lint", "Lint it.")]),
	);

	assert!(snapshot(&mut catalog, &config, false).await.is_ok());
}

#[tokio::test]
async fn plugin_skills_need_every_skill_support_tool() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	config.remove_default.push("skill_read".into());
	assert!(snapshot(&mut catalog, &config, false).await.is_ok());
	take_in(
		&mut catalog,
		&mut config,
		stored_plugin("acme-tools", &[("lint", "Lint it.")]),
	);

	let error = snapshot(&mut catalog, &config, false).await.unwrap_err();

	assert!(error.to_string().contains("Skill support"), "{error}");
}

#[tokio::test]
async fn a_plugin_without_skills_pins_an_empty_list_and_adds_no_support() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	take_in(&mut catalog, &mut config, stored_plugin("acme-tools", &[]));

	let snapshot = snapshot(&mut catalog, &config, false).await.unwrap();

	assert_eq!(snapshot.agent_plugins.len(), 1);
	assert!(snapshot.agent_plugins[0].skills.is_empty());
	assert_eq!(origin(&snapshot, "skill_read"), BindingOrigin::Default);
}

#[tokio::test]
async fn an_agent_with_plugins_is_not_admitted_remotely() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	take_in(
		&mut catalog,
		&mut config,
		stored_plugin("acme-tools", &[("lint", "Lint it.")]),
	);

	let error = snapshot(&mut catalog, &config, true).await.unwrap_err();

	assert_eq!(error.to_string(), "Agent Plugins are unavailable remotely");
}

#[tokio::test]
async fn a_missing_copy_refuses_the_agent_by_name() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	let plugin = stored_plugin("acme-tools", &[("lint", "Lint it.")]);
	config.agent_plugins.push(plugin_reference(&plugin));

	let error = snapshot(&mut catalog, &config, false).await.unwrap_err();

	assert!(
		matches!(&error, Error::Invalid(message) if message == "AGENT_PLUGIN_MISSING: acme-tools"),
		"{error:?}"
	);
}

#[rstest]
#[case::other_version("other_version")]
#[case::other_source("other_source")]
#[case::other_copy_under_the_digest("other_copy")]
#[tokio::test]
async fn a_copy_that_no_longer_matches_the_definition_conflicts(#[case] change: &str) {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	let plugin = stored_plugin("acme-tools", &[("lint", "Lint it.")]);
	config.agent_plugins.push(plugin_reference(&plugin));
	let stored = match change {
		"other_version" => {
			let mut stored = plugin.clone();
			stored.plugin.manifest.version = Some("9.9.9".into());
			stored
		}
		"other_source" => StoredAgentPlugin {
			source: "https://github.com/mirror/tools/tree/abcdef".into(),
			..plugin.clone()
		},
		_ => stored_plugin("other-tools", &[("lint", "Lint it.")]),
	};
	catalog.plugins.insert(plugin.plugin.digest.clone(), stored);

	let error = snapshot(&mut catalog, &config, false).await.unwrap_err();

	assert!(
		matches!(&error, Error::Conflict(message) if message == "AGENT_PLUGIN_CHANGED: acme-tools"),
		"{error:?}"
	);
}

struct WithoutPlugins(Catalog);
#[async_trait]
impl BindingCatalog for WithoutPlugins {
	async fn definition(&mut self, reference: &QualifiedRef) -> Result<Entry> {
		self.0.definition(reference).await
	}
	async fn installation(
		&mut self,
		projection: &aidash_domain::registry::Projection,
	) -> Result<()> {
		self.0.installation(projection).await
	}
	async fn source(&mut self, definition: &Entry) -> Result<()> {
		self.0.source(definition).await
	}
}

async fn admit(catalog: &mut WithoutPlugins, config: &AgentBindings) -> Result<BindingSnapshot> {
	resolve(
		catalog,
		&providers(),
		reference("agent"),
		&agent_entry(config),
		false,
	)
	.await
}

#[tokio::test]
async fn a_catalog_that_never_sees_plugins_refuses_them_and_serves_every_other_agent() {
	let mut config = agent_config();
	let mut seen = Catalog::new();
	take_in(
		&mut seen,
		&mut config,
		stored_plugin("acme-tools", &[("lint", "Lint it.")]),
	);
	let mut catalog = WithoutPlugins(seen);

	let refused = admit(&mut catalog, &config).await.unwrap_err();
	let served = admit(&mut catalog, &agent_config()).await;

	assert!(
		matches!(&refused, Error::Invalid(message) if message == "AGENT_PLUGIN_UNAVAILABLE"),
		"{refused:?}"
	);
	assert!(served.is_ok());
}
