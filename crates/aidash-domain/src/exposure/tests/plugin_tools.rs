use super::*;
use crate::agent_plugin::{
	ListedTool, Listing, PluginSettings, ServerSettings, fixtures::reference, pin, schema_digest,
};

const PLUGIN: &str = "acme-tools";

fn listed(names: &[&str]) -> Listing {
	Listing::Listed(
		names
			.iter()
			.map(|name| ListedTool {
				name: (*name).to_owned(),
				description: Some(format!("Does {name}.")),
				input_schema: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
			})
			.collect(),
	)
}

fn pinned(exposure: Option<BindingExposure>, names: &[&str]) -> PluginTools {
	let plugin = reference(PLUGIN, &[]);
	let docs = ServerName::try_from("docs".to_owned()).unwrap();
	let settings: PluginSettings = [(
		plugin.name.clone(),
		[(
			docs.clone(),
			ServerSettings {
				exposure,
				..Default::default()
			},
		)]
		.into(),
	)]
	.into();
	let listings = [((plugin.name.clone(), docs), listed(names))].into();
	pin(&[plugin], &settings, &listings, &BTreeSet::new())
}

/// Dispatch advertises a pinned tool under its alias, like any other bound tool.
fn advertised(run: &mut Run, pinned: &PluginTools) {
	for tool in pinned.servers.iter().flat_map(|server| &server.tools) {
		run.specs.insert(
			tool.alias.clone(),
			ToolSpec {
				name: tool.alias.clone(),
				description: tool.description.clone(),
				parameters: tool.input_schema.clone(),
			},
		);
	}
}

fn with_plugins(run: &mut Run, pinned: &PluginTools) -> Vec<Capability> {
	advertised(run, pinned);
	catalog(&run.snapshot(), &run.specs, &[], Some(pinned)).unwrap()
}

#[test]
fn a_pinned_plugin_tool_is_a_discoverable_tool_with_its_own_identity() {
	let mut run = Run::default();
	run.mandatory().tool("lookup", "Lookup", 0);
	let pinned = pinned(None, &["search", "fetch"]);

	let catalog = with_plugins(&mut run, &pinned);

	assert!(catalog.windows(2).all(|pair| pair[0].alias < pair[1].alias));
	let search = capability(&catalog, "acme-tools__docs__search");
	assert_eq!(search.kind, CapabilityKind::Tool);
	assert_eq!(
		search.identity,
		CapabilityIdentity::AgentPluginTool {
			plugin: pinned.servers[0].plugin.clone(),
			server: pinned.servers[0].server.clone(),
			tool: "search".into(),
		}
	);
	assert_eq!(
		search.identity.to_string(),
		"agent-plugin:acme-tools/docs/search"
	);
	assert_eq!(search.digest, pinned.servers[0].tools[1].schema_digest);
	assert_eq!(search.name, "search");
	assert_eq!(search.description, "Does search.");
	assert_eq!(search.detail["name"], "acme-tools__docs__search");
	assert_eq!(
		search.definition_bytes,
		tool_bytes(&run.specs["acme-tools__docs__search"]).unwrap()
	);
	assert!(!search.mandatory && !search.eager);
	assert!(search.bundle.is_none() && search.companions.is_empty());
	assert!(catalog.iter().any(|c| c.alias == "acme-tools__docs__fetch"));
}

#[test]
fn a_plugin_tool_is_found_and_loaded_like_a_registry_tool() {
	let mut run = Run::default();
	run.mandatory();
	let pinned = pinned(None, &["search"]);
	let catalog = with_plugins(&mut run, &pinned);
	let budget = DeferredBudgets::default();
	let alias = "acme-tools__docs__search";
	let state = ExposureState::default();

	let before = select(&budget, &catalog, &state).unwrap();
	let found = search(&catalog, &state, "search", None).unwrap();
	let digest = capability(&catalog, alias).digest.clone();
	let output = load(&budget, &catalog, &state, alias, &digest, 1).unwrap();
	let after = select(&budget, &catalog, &applied(&state, &output)).unwrap();

	assert!(!before.tools.contains(alias));
	assert!(
		before
			.index
			.contains(&format!("{alias} [tool]: Does search."))
	);
	assert_eq!(found["results"][0]["alias"], alias);
	assert_eq!(found["results"][0]["loaded"], false);
	assert_eq!(output["status"], "loaded");
	assert!(after.tools.contains(alias));
	assert_eq!(
		describe(&catalog, &budget, alias).unwrap()["identity"],
		json!({"agent_plugin_tool": {"plugin": PLUGIN, "server": "docs", "tool": "search"}})
	);
}

#[test]
fn a_changed_digest_is_refused_when_loading_a_plugin_tool() {
	let mut run = Run::default();
	run.mandatory();
	let catalog = with_plugins(&mut run, &pinned(None, &["search"]));

	assert_eq!(
		load(
			&DeferredBudgets::default(),
			&catalog,
			&ExposureState::default(),
			"acme-tools__docs__search",
			&schema_digest(&json!({})),
			1,
		),
		Err(Error::Invalid("CAPABILITY_CHANGED".into()))
	);
}

#[test]
fn an_eager_plugin_tool_is_selected_like_an_eager_binding() {
	let mut run = Run::default();
	run.mandatory()
		.tool("lookup", "Lookup", 0)
		.eager("tool", "lookup");
	let pinned = pinned(Some(BindingExposure::Eager), &["search"]);
	let catalog = with_plugins(&mut run, &pinned);
	let budget = DeferredBudgets::default();
	let alias = "acme-tools__docs__search";

	assert!(capability(&catalog, alias).eager);
	assert!(capability(&catalog, "lookup").eager);
	let state = ExposureState::default();
	let selection = select(&budget, &catalog, &state).unwrap();
	assert!(selection.tools.contains(alias) && selection.tools.contains("lookup"));
	assert!(!selection.index.contains(alias));

	let output = unload(&catalog, &state, alias).unwrap();
	let state = applied(&state, &output);
	assert!(state.unloaded_eager.contains(alias));
	assert!(
		!select(&budget, &catalog, &state)
			.unwrap()
			.tools
			.contains(alias)
	);
	let digest = capability(&catalog, alias).digest.clone();
	let reloaded = load(&budget, &catalog, &state, alias, &digest, 2).unwrap();
	let state = applied(&state, &reloaded);
	assert!(
		select(&budget, &catalog, &state)
			.unwrap()
			.tools
			.contains(alias)
	);
}

#[test]
fn a_deferred_server_exposure_leaves_its_tools_discoverable() {
	let mut run = Run::default();
	run.mandatory();
	let catalog = with_plugins(
		&mut run,
		&pinned(Some(BindingExposure::Deferred), &["search"]),
	);

	assert!(!capability(&catalog, "acme-tools__docs__search").eager);
}

#[test]
fn a_pinned_tool_dispatch_does_not_advertise_is_not_offered() {
	let mut run = Run::default();
	run.mandatory();
	let pinned = pinned(None, &["search", "fetch"]);
	advertised(&mut run, &pinned);
	run.specs.remove("acme-tools__docs__fetch");

	let catalog = catalog(&run.snapshot(), &run.specs, &[], Some(&pinned)).unwrap();

	assert!(
		catalog
			.iter()
			.any(|c| c.alias == "acme-tools__docs__search")
	);
	assert!(!catalog.iter().any(|c| c.alias == "acme-tools__docs__fetch"));
}

#[test]
fn without_a_pin_the_catalog_is_what_it_was() {
	let mut run = Run::default();
	run.mandatory().tool("lookup", "Lookup", 0);
	let pinned = pinned(None, &["search"]);
	advertised(&mut run, &pinned);
	let snapshot = run.snapshot();

	let none = catalog(&snapshot, &run.specs, &[], None).unwrap();
	let empty = catalog(&snapshot, &run.specs, &[], Some(&PluginTools::default())).unwrap();

	assert_eq!(none, empty);
	assert!(
		none.iter()
			.all(|c| matches!(c.identity, CapabilityIdentity::Registry(_)))
	);
	assert!(!none.iter().any(|c| c.alias == "acme-tools__docs__search"));
}

#[test]
fn a_plugin_alias_that_a_binding_already_has_is_a_conflict() {
	let mut run = Run::default();
	run.mandatory().tool("acme-tools__docs__search", "Taken", 0);
	let pinned = pinned(None, &["search"]);
	advertised(&mut run, &pinned);

	assert_eq!(
		catalog(&run.snapshot(), &run.specs, &[], Some(&pinned)),
		Err(Error::Conflict(
			"duplicate capability alias: acme-tools__docs__search".into()
		))
	);
}
