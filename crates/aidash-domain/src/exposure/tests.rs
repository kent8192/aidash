use super::*;
use crate::{
	context::{Context, ContextUsage},
	registry::bindings::{BindingOrigin, Narrowing, ResolvedBinding, ResolvedDefinition},
	tool::providers::core_descriptor,
};
use rstest::rstest;

const NODE: &str = "aidash://node-a";

fn reference(id: &str) -> QualifiedRef {
	QualifiedRef {
		registry_node: NODE.into(),
		id: id.into(),
		version: "1.0.0".into(),
	}
}
fn entry(identity: &QualifiedRef, kind: &str, description: &str, config: Value) -> Entry {
	serde_json::from_value(json!({
		"id": identity.id,
		"version": identity.version,
		"kind": kind,
		"name": {"en": identity.id},
		"description": {"en": description},
		"config": config,
	}))
	.unwrap()
}
fn budgets(metadata: usize, schema: usize, skill: usize) -> DeferredBudgets {
	DeferredBudgets {
		metadata_bytes: metadata,
		schema_bytes: schema,
		skill_bytes: skill,
	}
}

/// Builds an unvalidated Run snapshot plus the specs dispatch would advertise.
#[derive(Default)]
struct Run {
	agent: Vec<Value>,
	definitions: Vec<ResolvedDefinition>,
	bindings: Vec<ResolvedBinding>,
	specs: BTreeMap<String, ToolSpec>,
}
impl Run {
	fn define(&mut self, definition: Entry) -> (QualifiedRef, String) {
		let identity = QualifiedRef {
			registry_node: NODE.into(),
			id: definition.id.clone(),
			version: definition.version.clone(),
		};
		let digest = rules::digest(&serde_json::to_value(&definition).unwrap());
		self.definitions.push(ResolvedDefinition {
			identity: identity.clone(),
			definition,
			digest: digest.clone(),
		});
		(identity, digest)
	}
	fn bind_tool(
		&mut self,
		identity: QualifiedRef,
		alias: &str,
		descriptor: ToolDescriptor,
		description: &str,
		pad: usize,
		origin: BindingOrigin,
	) -> &mut Self {
		let definition = entry(
			&identity,
			"tool",
			description,
			serde_json::to_value(descriptor).unwrap(),
		);
		let (identity, digest) = self.define(definition.clone());
		self.bindings.push(ResolvedBinding {
			identity,
			definition,
			digest,
			origin,
			alias: Some(alias.into()),
			narrow: Narrowing::default(),
			installation: None,
			provider_contract_digest: Some("contract".into()),
			provider_implementation: Some("implementation".into()),
			excluded_reason: None,
		});
		self.specs.insert(
			alias.into(),
			ToolSpec {
				name: alias.into(),
				description: description.into(),
				parameters: json!({"type":"object","description":"x".repeat(pad)}),
			},
		);
		self
	}
	fn builtin(&mut self, operation: &str, origin: BindingOrigin) -> &mut Self {
		self.bind_tool(
			QualifiedRef::builtin(NODE, operation),
			operation,
			core_descriptor(NODE, operation).unwrap(),
			&format!("Builtin {operation}"),
			0,
			origin,
		)
	}
	fn mandatory(&mut self) -> &mut Self {
		for operation in REQUIRED_TOOLS
			.iter()
			.chain(EXPOSURE_TOOLS)
			.chain(&[SKILL_ASSET_READ])
		{
			self.builtin(operation, BindingOrigin::Required);
		}
		self
	}
	fn tool(&mut self, alias: &str, description: &str, pad: usize) -> &mut Self {
		self.bind_tool(
			reference(alias),
			alias,
			core_descriptor(NODE, "outbound_get").unwrap(),
			description,
			pad,
			BindingOrigin::Explicit,
		)
	}
	fn eager(&mut self, kind: &str, id: &str) -> &mut Self {
		self.agent.push(json!({
			"kind": kind,
			"target": reference(id),
			"exposure": "eager",
		}));
		self
	}
	fn skill(&mut self, id: &str, description: &str, instructions: &str) -> &mut Self {
		let identity = reference(id);
		let definition = entry(
			&identity,
			"skill",
			description,
			json!({"instructions": instructions, "files": [{"path":"notes.md","content":"hello"}]}),
		);
		let (identity, digest) = self.define(definition.clone());
		self.bindings.push(ResolvedBinding {
			identity,
			definition,
			digest,
			origin: BindingOrigin::Explicit,
			alias: None,
			narrow: Narrowing::default(),
			installation: None,
			provider_contract_digest: None,
			provider_implementation: None,
			excluded_reason: None,
		});
		self
	}
	fn snapshot(&mut self) -> BindingSnapshot {
		let agent = reference("agent");
		let definition = entry(
			&agent,
			"agent",
			"",
			json!({
				"schema_version": 1,
				"model": {"id": "model", "version": "1.0.0"},
				"instructions": "Work.",
				"bindings": self.agent,
				"exposure": {"version": "deferred@1"},
			}),
		);
		self.define(definition);
		BindingSnapshot {
			schema_version: 1,
			agent,
			remote: false,
			bindings: self.bindings.clone(),
			definitions: self.definitions.clone(),
			foreign_agents: vec![],
		}
	}
	fn catalog(&mut self) -> Vec<Capability> {
		catalog(&self.snapshot(), &self.specs, &[]).unwrap()
	}
}
fn capability<'a>(catalog: &'a [Capability], alias: &str) -> &'a Capability {
	catalog.iter().find(|c| c.alias == alias).unwrap()
}
fn applied(state: &ExposureState, output: &Value) -> ExposureState {
	let update: ExposureUpdate = serde_json::from_value(output["exposure_update"].clone()).unwrap();
	let mut next = state.clone();
	next.apply(&update);
	next
}
fn mandatory_aliases() -> BTreeSet<String> {
	REQUIRED_TOOLS
		.iter()
		.chain(EXPOSURE_TOOLS)
		.chain(&[SKILL_ASSET_READ])
		.map(|alias| alias.to_string())
		.collect()
}

#[test]
fn over_budget_catalog_keeps_non_mandatory_definitions_out_of_the_request() {
	let mut run = Run::default();
	run.mandatory();
	for index in 0..30 {
		run.tool(&format!("tool_{index:02}"), "Large tool", 2_000);
	}
	let catalog = run.catalog();
	assert_eq!(catalog.len(), 37);
	assert!(catalog.windows(2).all(|pair| pair[0].alias < pair[1].alias));
	let selection = select(
		&DeferredBudgets::default(),
		&catalog,
		&ExposureState::default(),
	)
	.unwrap();
	assert_eq!(selection.tools, mandatory_aliases());
	assert!(selection.skills.is_empty());
	assert!(selection.usage.schema_bytes <= DEFAULT_SCHEMA_BYTES);
	assert_eq!(selection.usage.exposed.len(), mandatory_aliases().len());
	assert!(selection.index.contains("tool_00 [tool]: Large tool\n"));
	assert!(!selection.index.contains("workspace_read [tool]"));
}

#[test]
fn index_is_ordered_by_alias_and_truncated_with_a_footer() {
	let mut run = Run::default();
	run.mandatory();
	for alias in ["zeta", "alpha", "mid", "beta"] {
		run.tool(alias, &format!("{alias} first line\nsecond line"), 0);
	}
	for index in 0..40 {
		run.tool(&format!("filler_{index:02}"), &"d".repeat(400), 0);
	}
	let catalog = run.catalog();
	let roomy = select(
		&budgets(65_536, 16_384, 32_768),
		&catalog,
		&ExposureState::default(),
	)
	.unwrap();
	assert_eq!(roomy.index_omitted, 0);
	assert!(roomy.index.starts_with(INDEX_HEADER));
	assert!(roomy.index.contains("alpha [tool]: alpha first line\n"));
	assert!(!roomy.index.contains("second line"));
	assert!(
		roomy
			.index
			.contains(&format!("filler_00 [tool]: {}\n", "d".repeat(160)))
	);
	let lines = roomy.index[INDEX_HEADER.len()..]
		.lines()
		.map(|line| line.split(' ').next().unwrap().to_owned())
		.collect::<Vec<_>>();
	let mut sorted = lines.clone();
	sorted.sort();
	assert_eq!(lines, sorted);
	assert_eq!(lines.len(), 44);

	let tight = budgets(512, 16_384, 32_768);
	let first = select(&tight, &catalog, &ExposureState::default()).unwrap();
	assert_eq!(
		first,
		select(&tight, &catalog, &ExposureState::default()).unwrap()
	);
	assert!(first.index_omitted > 0);
	assert!(first.index.ends_with(&format!(
		"{} more — use capability_search\n",
		first.index_omitted
	)));
	assert!(first.usage.metadata_bytes <= 512);
	assert_eq!(first.usage.metadata_bytes, escaped_len(&first.index));
	let shown = first.index[INDEX_HEADER.len()..].lines().count() - 1;
	assert_eq!(shown + first.index_omitted, 44);
	assert!(first.index.contains("alpha [tool]"));
}

#[test]
fn search_scores_orders_and_pages() {
	let mut run = Run::default();
	run.mandatory()
		.tool("weather_lookup", "Read a forecast", 0)
		.tool("forecast", "Lookup weather data", 0)
		.tool("calendar", "Plan meetings", 0);
	for index in 0..20 {
		run.tool(&format!("pad_{index:02}"), "Unrelated", 0);
	}
	let catalog = run.catalog();
	let state = ExposureState::default();
	let found = search(&catalog, &state, "Weather LOOKUP!", None).unwrap();
	let aliases = found["results"]
		.as_array()
		.unwrap()
		.iter()
		.map(|item| item["alias"].as_str().unwrap())
		.collect::<Vec<_>>();
	// weather_lookup: alias+name for both tokens (8); forecast: description for both (2).
	assert_eq!(aliases, ["weather_lookup", "forecast"]);
	assert_eq!(found["truncated"], false);
	assert_eq!(found["next_cursor"], Value::Null);
	let item = &found["results"][0];
	assert_eq!(item["kind"], "tool");
	assert_eq!(item["loaded"], false);
	assert_eq!(
		item["digest"],
		capability(&catalog, "weather_lookup").digest
	);
	let mandatory = search(&catalog, &state, "workspace_read", None).unwrap();
	assert_eq!(mandatory["results"][0]["loaded"], true);

	let first = search(&catalog, &state, "", None).unwrap();
	assert_eq!(first["results"].as_array().unwrap().len(), SEARCH_PAGE_SIZE);
	assert_eq!(first["truncated"], true);
	assert_eq!(first["next_cursor"], "16");
	assert_eq!(first["results"][0]["alias"], catalog[0].alias);
	let second = search(&catalog, &state, "", Some("16")).unwrap();
	assert_eq!(
		second["results"].as_array().unwrap().len(),
		catalog.len() - SEARCH_PAGE_SIZE
	);
	assert_eq!(second["truncated"], false);
	assert!(search(&catalog, &state, "", Some("x")).is_err());
	assert!(search(&catalog, &state, "", Some("999")).is_err());
	assert!(
		search(&catalog, &state, "nothing matches", None).unwrap()["results"]
			.as_array()
			.unwrap()
			.is_empty()
	);
}

#[test]
fn load_rejects_unknown_changed_and_over_budget_capabilities_without_eviction() {
	let mut run = Run::default();
	run.mandatory()
		.tool("first", "First", 900)
		.tool("second", "Second", 900);
	let catalog = run.catalog();
	let mandatory_bytes = catalog
		.iter()
		.filter(|c| c.mandatory)
		.map(|c| c.definition_bytes)
		.sum::<usize>();
	let budget = budgets(
		4096,
		mandatory_bytes + capability(&catalog, "first").definition_bytes,
		32_768,
	);
	let state = ExposureState::default();
	assert_eq!(
		load(&budget, &catalog, &state, "missing", "sha256:x", 1),
		Err(Error::Invalid("UNKNOWN_CAPABILITY".into()))
	);
	assert_eq!(
		load(&budget, &catalog, &state, "first", "sha256:stale", 1),
		Err(Error::Invalid("CAPABILITY_CHANGED".into()))
	);
	let digest = capability(&catalog, "first").digest.clone();
	let loaded = load(&budget, &catalog, &state, "first", &digest, 3).unwrap();
	assert_eq!(loaded["status"], "loaded");
	assert_eq!(loaded["alias"], "first");
	assert_eq!(loaded["exposure_update"]["load"]["step"], 3);
	let state = applied(&state, &loaded);
	assert!(
		select(&budget, &catalog, &state)
			.unwrap()
			.tools
			.contains("first")
	);

	let second = capability(&catalog, "second");
	let refused = load(&budget, &catalog, &state, "second", &second.digest, 4).unwrap();
	assert_eq!(refused["error"], "EXPOSURE_BUDGET_EXCEEDED");
	assert_eq!(refused["budget"], budget.schema_bytes);
	assert_eq!(
		refused["required"],
		budget.schema_bytes + second.definition_bytes
	);
	assert!(refused.get("exposure_update").is_none());
	assert!(
		refused["exposed"]
			.as_array()
			.unwrap()
			.iter()
			.any(|item| item["alias"] == "first")
	);
	// No eviction: the earlier load stays exposed.
	assert!(
		select(&budget, &catalog, &state)
			.unwrap()
			.tools
			.contains("first")
	);
}

#[test]
fn already_exposed_capabilities_return_identity_without_body_or_update() {
	let mut run = Run::default();
	run.mandatory()
		.tool("lookup", "Lookup", 0)
		.skill("guide", "A guide", "SECRET BODY TEXT");
	let catalog = run.catalog();
	let budget = DeferredBudgets::default();
	let state = ExposureState::default();
	let required = capability(&catalog, "workspace_read");
	let output = load(
		&budget,
		&catalog,
		&state,
		"workspace_read",
		&required.digest,
		1,
	)
	.unwrap();
	assert_eq!(
		output,
		json!({"status":"already_loaded","identity":required.identity,"digest":required.digest})
	);
	let skill = catalog
		.iter()
		.find(|c| c.kind == CapabilityKind::Skill)
		.unwrap();
	let loaded = load(&budget, &catalog, &state, &skill.alias, &skill.digest, 1).unwrap();
	assert_eq!(loaded["status"], "loaded");
	assert!(!loaded.to_string().contains("SECRET BODY TEXT"));
	let state = applied(&state, &loaded);
	assert_eq!(
		select(&budget, &catalog, &state).unwrap().skills,
		std::slice::from_ref(&skill.alias)
	);
	let again = load(&budget, &catalog, &state, &skill.alias, &skill.digest, 2).unwrap();
	assert_eq!(again["status"], "already_loaded");
	assert!(again.get("exposure_update").is_none());
	assert!(!again.to_string().contains("SECRET BODY TEXT"));
}

#[test]
fn eager_capabilities_can_be_unloaded_and_reloaded() {
	let mut run = Run::default();
	run.mandatory()
		.tool("lookup", "Lookup", 0)
		.eager("tool", "lookup");
	let catalog = run.catalog();
	let budget = DeferredBudgets::default();
	let lookup = capability(&catalog, "lookup");
	assert!(lookup.eager);
	let state = ExposureState::default();
	assert!(
		select(&budget, &catalog, &state)
			.unwrap()
			.tools
			.contains("lookup")
	);
	assert_eq!(
		unload(&catalog, &state, "workspace_read"),
		Err(Error::Invalid("MANDATORY_EXPOSURE".into()))
	);
	assert_eq!(
		unload(&catalog, &state, "missing"),
		Err(Error::Invalid("UNKNOWN_CAPABILITY".into()))
	);
	let output = unload(&catalog, &state, "lookup").unwrap();
	assert_eq!(
		output,
		json!({"status":"unloaded","exposure_update":{"unload":{"alias":"lookup"}}})
	);
	let state = applied(&state, &output);
	assert!(state.unloaded_eager.contains("lookup"));
	let selection = select(&budget, &catalog, &state).unwrap();
	assert!(!selection.tools.contains("lookup"));
	assert!(selection.index.contains("lookup [tool]"));
	assert_eq!(
		unload(&catalog, &state, "lookup").unwrap(),
		json!({"status":"not_loaded"})
	);
	let reloaded = load(&budget, &catalog, &state, "lookup", &lookup.digest, 2).unwrap();
	assert_eq!(reloaded["status"], "loaded");
	let state = applied(&state, &reloaded);
	assert!(!state.unloaded_eager.contains("lookup"));
	let selection = select(&budget, &catalog, &state).unwrap();
	assert!(selection.tools.contains("lookup"));
	assert_eq!(
		selection.usage.schema_bytes,
		catalog
			.iter()
			.filter(|c| c.mandatory || c.alias == "lookup")
			.map(|c| c.definition_bytes)
			.sum::<usize>()
	);
}

#[test]
fn bundle_eagerness_reaches_members_and_records_provenance() {
	let mut run = Run::default();
	run.mandatory().tool("member", "Bundled member", 0);
	let bundle = reference("kit");
	let definition = entry(
		&bundle,
		"bundle",
		"Kit",
		json!({"members":[reference("member")]}),
	);
	run.define(definition);
	run.eager("bundle", "kit");
	let catalog = run.catalog();
	let member = capability(&catalog, "member");
	assert!(member.eager);
	assert_eq!(member.bundle.as_deref(), Some("kit"));
	let found = search(&catalog, &ExposureState::default(), "kit", None).unwrap();
	assert_eq!(found["results"][0]["alias"], "member");
}

#[test]
fn lifecycle_companions_fold_into_their_parent() {
	let mut run = Run::default();
	run.mandatory()
		.builtin("shell", BindingOrigin::Explicit)
		.builtin("shell_poll", BindingOrigin::Companion)
		.builtin("shell_cancel", BindingOrigin::Companion);
	let catalog = run.catalog();
	assert!(
		catalog
			.iter()
			.all(|c| c.alias != "shell_poll" && c.alias != "shell_cancel")
	);
	let shell = capability(&catalog, "shell");
	assert_eq!(shell.companions, ["shell_cancel", "shell_poll"]);
	let spec = |alias: &str| tool_bytes(&run.specs[alias]).unwrap();
	assert!(spec("shell") > serde_json::to_string(&run.specs["shell"]).unwrap().len());
	assert_eq!(
		shell.definition_bytes,
		spec("shell") + spec("shell_poll") + spec("shell_cancel")
	);
	assert_eq!(
		shell.detail,
		serde_json::to_value(&run.specs["shell"]).unwrap()
	);
	let budget = DeferredBudgets::default();
	let state = applied(
		&ExposureState::default(),
		&load(
			&budget,
			&catalog,
			&ExposureState::default(),
			"shell",
			&shell.digest,
			1,
		)
		.unwrap(),
	);
	let tools = select(&budget, &catalog, &state).unwrap().tools;
	for alias in ["shell", "shell_poll", "shell_cancel"] {
		assert!(tools.contains(alias), "{alias}");
	}
}

#[test]
fn shared_lifecycle_companions_are_charged_once() {
	// Arrange: two Python parents share python_poll and python_cancel.
	let mut run = Run::default();
	run.mandatory()
		.builtin("code_interpreter", BindingOrigin::Explicit)
		.builtin("python_install", BindingOrigin::Explicit)
		.builtin("python_poll", BindingOrigin::Companion)
		.builtin("python_cancel", BindingOrigin::Companion);
	let catalog = run.catalog();
	let budgets = DeferredBudgets::default();
	let base = select(&budgets, &catalog, &ExposureState::default())
		.unwrap()
		.usage
		.schema_bytes;
	let mut state = ExposureState::default();
	for alias in ["code_interpreter", "python_install"] {
		let digest = capability(&catalog, alias).digest.clone();
		state = applied(
			&state,
			&load(&budgets, &catalog, &state, alias, &digest, 1).unwrap(),
		);
	}
	// Act
	let selection = select(&budgets, &catalog, &state).unwrap();
	// Assert
	let bytes = |alias: &str| tool_bytes(&run.specs[alias]).unwrap();
	assert_eq!(
		selection.usage.schema_bytes - base,
		bytes("code_interpreter")
			+ bytes("python_install")
			+ bytes("python_poll")
			+ bytes("python_cancel")
	);
}

#[test]
fn restricted_selection_measures_only_the_sent_tools() {
	// Arrange
	let mut run = Run::default();
	run.mandatory();
	let catalog = run.catalog();
	let mut selection = select(
		&DeferredBudgets::default(),
		&catalog,
		&ExposureState::default(),
	)
	.unwrap();
	let sent = [run.specs["workspace_read"].clone()];
	// Act
	selection.restrict_tools(&sent).unwrap();
	// Assert
	assert_eq!(
		selection.tools.iter().collect::<Vec<_>>(),
		["workspace_read"]
	);
	assert_eq!(selection.usage.schema_bytes, tool_bytes(&sent[0]).unwrap());
	assert_eq!(
		selection
			.usage
			.exposed
			.iter()
			.map(|(alias, _)| alias.as_str())
			.collect::<Vec<_>>(),
		["workspace_read"]
	);
}

#[test]
fn skill_aliases_are_deterministic_unique_and_valid() {
	let mut run = Run::default();
	run.mandatory()
		.skill("Data.Tool", "Dotted", "Use dots.")
		.skill("data-tool", "Dashed", "Use dashes.")
		.skill(&format!("a{}", "b".repeat(60)), "Long", "Long id.");
	let direct = DirectSkill {
		metadata: SkillMetadata {
			skill_id: Uuid::from_u128(7),
			name: "Data Tool".into(),
			description: "Mounted".into(),
			origin: "/skills/data".into(),
			digest: "sha256:direct".into(),
			license: None,
		},
		origin: SkillOriginKind::Root,
		body_bytes: 100,
		files: vec![],
	};
	let snapshot = run.snapshot();
	let first = catalog(&snapshot, &run.specs, std::slice::from_ref(&direct)).unwrap();
	let second = catalog(&snapshot, &run.specs, std::slice::from_ref(&direct)).unwrap();
	assert_eq!(first, second);
	let skills = first
		.iter()
		.filter(|c| c.kind == CapabilityKind::Skill)
		.collect::<Vec<_>>();
	assert_eq!(skills.len(), 4);
	assert_eq!(
		skills
			.iter()
			.map(|c| &c.alias)
			.collect::<BTreeSet<_>>()
			.len(),
		4
	);
	for skill in &skills {
		crate::registry::bindings::validate_alias(&skill.alias).unwrap();
		let (stem, hash) = skill.alias.rsplit_once('_').unwrap();
		assert!(stem.starts_with("skill_") && stem.len() <= "skill_".len() + 40);
		assert_eq!(hash.len(), 8);
		assert!(
			skill
				.alias
				.bytes()
				.all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'_'))
		);
	}
	assert_eq!(
		skills
			.iter()
			.filter(|c| c.alias.starts_with("skill_data_tool_"))
			.count(),
		3
	);
	let mounted = skills
		.iter()
		.find(|c| matches!(c.identity, CapabilityIdentity::DirectSkill { .. }))
		.unwrap();
	assert_eq!(
		mounted.definition_bytes,
		escaped_len(&resident_block(mounted, "")) + 100
	);
	let registry = skills.iter().find(|c| c.description == "Dotted").unwrap();
	assert_eq!(
		registry.definition_bytes,
		escaped_len(&resident_block(
			registry,
			&registry_skill_body(&snapshot, registry).unwrap()
		))
	);
	assert_eq!(registry.detail["files"][0]["size"], 5);
	assert!(resident_block(registry, "BODY").starts_with(&format!(
		"\nSkill {} ({NODE}/Data.Tool@1.0.0; digest {}; origin registry):\nBODY\n",
		registry.alias, registry.digest
	)));

	// A tool aliased like a Skill is a conflict, never a silent shadow.
	let alias = registry.alias.clone();
	run.tool(&alias, "Impostor", 0);
	assert!(matches!(
		catalog(&run.snapshot(), &run.specs, &[]),
		Err(Error::Conflict(_))
	));
}

#[test]
fn apply_is_idempotent() {
	let loaded = Loaded {
		alias: "lookup".into(),
		kind: CapabilityKind::Tool,
		identity: CapabilityIdentity::Registry(reference("lookup")),
		digest: "sha256:a".into(),
		step: 2,
	};
	let mut state = ExposureState::default();
	assert!(state.is_empty());
	let load = ExposureUpdate::Load(loaded.clone());
	state.apply(&load);
	let once = state.clone();
	state.apply(&load);
	assert_eq!(state, once);
	assert_eq!(state.loaded, std::slice::from_ref(&loaded));
	let unload = ExposureUpdate::Unload {
		alias: "lookup".into(),
	};
	state.apply(&unload);
	let once = state.clone();
	state.apply(&unload);
	assert_eq!(state, once);
	assert!(state.loaded.is_empty());
	let changed = ExposureUpdate::Load(Loaded {
		digest: "sha256:b".into(),
		..loaded.clone()
	});
	state.apply(&load);
	state.apply(&changed);
	assert_eq!(state.loaded.len(), 1);
	assert_eq!(state.loaded[0].digest, "sha256:b");
	let encoded = serde_json::to_value(&load).unwrap();
	assert_eq!(encoded["load"]["alias"], "lookup");
	assert_eq!(
		serde_json::from_value::<ExposureUpdate>(encoded).unwrap(),
		load
	);
}

#[test]
fn staged_updates_leave_the_current_request_unchanged_until_activation() {
	// Arrange
	let mut run = Run::default();
	run.mandatory().tool("lookup", "Lookup", 0);
	let catalog = run.catalog();
	let budgets = DeferredBudgets::default();
	let mut state = ExposureState::default();
	let lookup = capability(&catalog, "lookup");
	let output = load(&budgets, &catalog, &state, "lookup", &lookup.digest, 1).unwrap();
	// Act
	state.stage(serde_json::from_value(output["exposure_update"].clone()).unwrap());
	// Assert: the response's own request never gains the loaded tool.
	assert!(!state.is_empty());
	assert!(
		!select(&budgets, &catalog, &state)
			.unwrap()
			.tools
			.contains("lookup")
	);
	assert!(
		select(&budgets, &catalog, &state.effective())
			.unwrap()
			.tools
			.contains("lookup")
	);
	assert_eq!(
		load(
			&budgets,
			&catalog,
			&state.effective(),
			"lookup",
			&lookup.digest,
			1
		)
		.unwrap()["status"],
		"already_loaded"
	);
	let encoded: ExposureState =
		serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
	assert_eq!(encoded, state);
	state.activate();
	assert!(state.pending.is_empty());
	assert!(
		select(&budgets, &catalog, &state)
			.unwrap()
			.tools
			.contains("lookup")
	);
	let unloaded = unload(&catalog, &state, "lookup").unwrap();
	state.stage(serde_json::from_value(unloaded["exposure_update"].clone()).unwrap());
	assert!(
		select(&budgets, &catalog, &state)
			.unwrap()
			.tools
			.contains("lookup")
	);
	state.activate();
	assert!(
		!select(&budgets, &catalog, &state)
			.unwrap()
			.tools
			.contains("lookup")
	);
}

#[test]
fn renamed_skill_asset_read_stays_mandatory_and_is_named_in_skill_guidance() {
	// Arrange: Skill support explicitly bound under a custom alias.
	let mut run = Run::default();
	for operation in REQUIRED_TOOLS.iter().chain(EXPOSURE_TOOLS) {
		run.builtin(operation, BindingOrigin::Required);
	}
	run.bind_tool(
		QualifiedRef::builtin(NODE, SKILL_ASSET_READ),
		"read_asset",
		core_descriptor(NODE, SKILL_ASSET_READ).unwrap(),
		"Read Skill assets",
		0,
		BindingOrigin::SkillSupport,
	)
	.skill("packaged", "Packaged", "Follow the notes.");
	let snapshot = run.snapshot();
	// Act
	let catalog = catalog(&snapshot, &run.specs, &[]).unwrap();
	let selection = select(
		&DeferredBudgets::default(),
		&catalog,
		&ExposureState::default(),
	)
	.unwrap();
	let skill = catalog
		.iter()
		.find(|c| c.kind == CapabilityKind::Skill)
		.unwrap();
	let body = registry_skill_body(&snapshot, skill).unwrap();
	// Assert
	assert!(capability(&catalog, "read_asset").mandatory);
	assert!(selection.tools.contains("read_asset"));
	assert!(body.starts_with("Follow the notes.\n\n"));
	assert!(body.contains(&format!(
		"through read_asset with alias {} and digest {}",
		skill.alias, skill.digest
	)));
	assert!(body.ends_with("\n- notes.md\n"));
	assert!(!body.contains("skill_read"));
	assert_eq!(
		skill.definition_bytes,
		escaped_len(&resident_block(skill, &body))
	);
}

#[rstest]
#[case::eager_tool("tool", 20_000)]
#[case::eager_skill("skill", 40_000)]
fn mandatory_and_eager_exposure_over_budget_is_invalid(#[case] kind: &str, #[case] size: usize) {
	let mut run = Run::default();
	run.mandatory();
	if kind == "tool" {
		run.tool("huge", "Huge", size).eager("tool", "huge");
	} else {
		run.skill("huge", "Huge", &"x".repeat(size))
			.eager("skill", "huge");
	}
	let catalog = run.catalog();
	assert!(matches!(
		select(
			&DeferredBudgets::default(),
			&catalog,
			&ExposureState::default()
		),
		Err(Error::Invalid(_))
	));
}

#[test]
fn describe_returns_detail_and_accounting() {
	let mut run = Run::default();
	run.mandatory().tool("lookup", "Lookup", 10);
	let catalog = run.catalog();
	let lookup = capability(&catalog, "lookup");
	let described = describe(&catalog, &DeferredBudgets::default(), "lookup").unwrap();
	assert_eq!(
		described["detail"],
		serde_json::to_value(&run.specs["lookup"]).unwrap()
	);
	assert_eq!(described["bytes"], lookup.definition_bytes);
	assert_eq!(described["budget"], DEFAULT_SCHEMA_BYTES);
	assert_eq!(described["identity"]["registry"]["id"], "lookup");
	assert_eq!(
		describe(&catalog, &DeferredBudgets::default(), "missing"),
		Err(Error::Invalid("UNKNOWN_CAPABILITY".into()))
	);
}

#[test]
fn policy_defaults_bounds_and_unknown_fields() {
	let deferred: ExposurePolicy = serde_json::from_value(json!({"version":"deferred@1"})).unwrap();
	assert_eq!(
		deferred,
		ExposurePolicy::Deferred(DeferredBudgets::default())
	);
	deferred.validate().unwrap();
	let legacy: ExposurePolicy = serde_json::from_value(json!({"version":"legacy@1"})).unwrap();
	assert_eq!(legacy, ExposurePolicy::Legacy);
	assert_eq!(
		serde_json::to_value(legacy).unwrap(),
		json!({"version":"legacy@1"})
	);
	let custom: ExposurePolicy = serde_json::from_value(
		json!({"version":"deferred@1","metadata_bytes":512,"schema_bytes":262144}),
	)
	.unwrap();
	assert_eq!(
		custom,
		ExposurePolicy::Deferred(budgets(512, 262_144, DEFAULT_SKILL_BYTES))
	);
	for invalid in [
		json!({"version":"deferred@1","metadata_bytes":511}),
		json!({"version":"deferred@1","schema_bytes":262145}),
		json!({"version":"deferred@1","skill_bytes":1023}),
	] {
		let policy: ExposurePolicy = serde_json::from_value(invalid).unwrap();
		assert!(matches!(policy.validate(), Err(Error::Invalid(_))));
	}
	for unknown in [
		json!({"version":"deferred@1","tokens":1}),
		json!({"version":"legacy@1","metadata_bytes":512}),
		json!({"version":"deferred@2"}),
	] {
		assert!(
			serde_json::from_value::<ExposurePolicy>(unknown.clone()).is_err(),
			"{unknown}"
		);
	}
}

#[test]
fn legacy_context_and_usage_json_stay_byte_identical() {
	let usage = r#"{"input_tokens":1,"output_tokens":2,"context_window":3,"compactions":4}"#;
	let parsed: ContextUsage = serde_json::from_str(usage).unwrap();
	assert_eq!(parsed.exposure, None);
	assert_eq!(serde_json::to_string(&parsed).unwrap(), usage);
	let context = serde_json::to_string(&Context::default()).unwrap();
	assert!(!context.contains("exposure"));
	let parsed: Context = serde_json::from_str(&context).unwrap();
	assert!(parsed.exposure.is_empty());
	assert_eq!(serde_json::to_string(&parsed).unwrap(), context);
	assert!(
		!serde_json::to_string(&parsed.inspection())
			.unwrap()
			.contains("exposure")
	);

	let mut deferred = Context::default();
	deferred.exposure.apply(&ExposureUpdate::Unload {
		alias: "lookup".into(),
	});
	let encoded = serde_json::to_string(&deferred).unwrap();
	assert!(encoded.contains(r#""exposure":{"loaded":[],"unloaded_eager":["lookup"]}"#));
	assert_eq!(
		serde_json::from_str::<Context>(&encoded).unwrap().exposure,
		deferred.exposure
	);
	assert!(
		serde_json::to_string(&deferred.inspection())
			.unwrap()
			.contains("unloaded_eager")
	);
}
