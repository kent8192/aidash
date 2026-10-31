use super::*;
use serde_json::{Value, json};

mod agent_plugins;

fn agent() -> AgentBindings {
	serde_json::from_value(json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Work on the Task."})).unwrap()
}
#[test]
fn legacy_and_mixed_schemas_are_rejected() {
	for field in [
		"core_capabilities",
		"allow_task_creation",
		"allow_workspace_retrieval",
		"allow_cross_conversation_memory",
		"tools",
		"skills",
		"knowledge_digest",
	] {
		let mut input = serde_json::to_value(agent()).unwrap();
		input[field] = json!(false);
		assert!(
			serde_json::from_value::<AgentBindings>(input).is_err(),
			"{field}"
		);
	}
	assert!(
		serde_json::from_value::<AgentBindings>(
			json!({"model":{"id":"model","version":"1.0.0"},"instructions":"old"})
		)
		.is_err()
	);
}
#[test]
fn normalization_records_trusted_origins_and_keeps_required_tools() {
	let mut agent = agent();
	agent.bindings.push(Binding::tool(QualifiedRef::builtin(
		"aidash://node-a",
		"file_read",
	)));
	agent.remove_default.push("memory_mutate".into());
	let normalized = agent.normalize("aidash://node-a").unwrap();
	assert_eq!(
		normalized
			.iter()
			.filter(|b| b.origin == BindingOrigin::Required && b.binding.kind == BindingKind::Tool)
			.count(),
		2
	);
	assert_eq!(
		normalized
			.iter()
			.find(|b| b.binding.target.id == "aidash.file_read")
			.unwrap()
			.origin,
		BindingOrigin::Explicit
	);
	assert!(
		!normalized
			.iter()
			.any(|b| b.binding.target.id == "aidash.memory_mutate")
	);
	agent.remove_default.push("workspace_read".into());
	assert!(agent.normalize("aidash://node-a").is_err());
}
#[test]
fn narrowing_intersects_and_enforces_bounds_without_changing_facts() {
	let a: Narrowing = serde_json::from_value(json!({"allowed_hosts":["a.example","b.example"],"scope":{"recipient":["a","b"]},"limits":{"maximum_bytes":100}})).unwrap();
	let b: Narrowing = serde_json::from_value(
		json!({"allowed_hosts":["a.example"],"scope":{"recipient":["b"]},"limits":{"maximum_bytes":20}}),
	)
	.unwrap();
	let effective = a.intersect(&b).unwrap();
	let mut input = json!({"url":"https://a.example/path","recipient":"b"});
	effective.apply(&mut input).unwrap();
	assert_eq!(input["maximum_bytes"], 20);
	input["maximum_bytes"] = json!(21);
	assert!(effective.apply(&mut input).is_err());
	input["maximum_bytes"] = json!(20);
	input["url"] = json!("https://b.example/path");
	assert!(effective.apply(&mut input).is_err());
	assert!(serde_json::from_value::<Narrowing>(json!({"effect":"read_only"})).is_err());
}
#[test]
fn explicit_source_does_not_implicitly_grant_memory_read_or_write() {
	let mut agent = agent();
	agent.bindings.push(Binding {
		kind: BindingKind::Memory,
		target: QualifiedRef {
			registry_deployment: "aidash://node-a".into(),
			id: "native.memory".into(),
			version: "1.0.0".into(),
		},
		alias: None,
		narrow: Narrowing::default(),
		members: vec![],
		exposure: None,
	});
	let normalized = agent.normalize("aidash://node-a").unwrap();
	assert_eq!(
		normalized
			.iter()
			.filter(|b| b.binding.kind == BindingKind::Memory)
			.count(),
		1
	);
	agent.remove_default.push("memory_mutate".into());
	assert!(
		agent
			.normalize("aidash://node-a")
			.unwrap()
			.iter()
			.any(|b| b.binding.kind == BindingKind::Memory)
	);
}
#[test]
fn memory_builtins_stay_default_bindings_and_explicit_ones_need_a_memory() {
	let memory_tools = |agent: &AgentBindings| -> Vec<BindingOrigin> {
		agent
			.normalize("aidash://node-a")
			.unwrap()
			.into_iter()
			.filter(|b| {
				MEMORY_TOOLS
					.iter()
					.any(|tool| b.binding.target.id == format!("aidash.{tool}"))
			})
			.map(|b| b.origin)
			.collect()
	};
	// The snapshot records them so it can say why they are excluded.
	let mut agent = agent();
	assert_eq!(memory_tools(&agent), vec![BindingOrigin::Default; 3]);
	let explicit = Binding::tool(QualifiedRef::builtin("aidash://node-a", "memory_recall"));
	agent.bindings.push(explicit.clone());
	assert!(agent.validate().is_err());
	agent.bindings.push(Binding {
		kind: BindingKind::Memory,
		target: QualifiedRef {
			registry_deployment: "aidash://node-a".into(),
			id: "native.memory".into(),
			version: "1.0.0".into(),
		},
		alias: None,
		narrow: Narrowing::default(),
		members: vec![],
		exposure: None,
	});
	agent.validate().unwrap();
	agent.bindings.retain(|b| b != &explicit);
	agent.remove_default.push("memory_recall".into());
	assert_eq!(memory_tools(&agent).len(), 2);
}
#[test]
fn only_a_memory_definition_without_schema_version_is_a_memory_provider() {
	use super::memory_provider;
	assert!(memory_provider(
		"memory",
		&json!({"engine":"hindsight_rust"})
	));
	for adapter in ["semantic_memory", "conversation_memory"] {
		assert!(!memory_provider(
			"memory",
			&json!({"schema_version":1,"source":{"adapter":adapter}})
		));
	}
	assert!(!memory_provider(
		"source",
		&json!({"memory":{"id":"m","version":"1.0.0"}})
	));
}
#[test]
fn memory_exclusion_applies_only_to_memory_tools_without_a_memory() {
	use crate::tool::providers::memory_exclusion;
	assert!(
		memory_exclusion(BindingOrigin::Default, "memory_recall", false)
			.unwrap()
			.is_some()
	);
	assert!(memory_exclusion(BindingOrigin::Explicit, "memory_mutate", false).is_err());
	assert_eq!(
		memory_exclusion(BindingOrigin::Default, "memory_reflect", true).unwrap(),
		None
	);
	assert_eq!(
		memory_exclusion(BindingOrigin::Default, "file_read", false).unwrap(),
		None
	);
}
#[test]
fn bound_skills_require_support_tools() {
	let mut agent = agent();
	let mut binding = Binding::tool(QualifiedRef {
		registry_deployment: "aidash://node-a".into(),
		id: "skill".into(),
		version: "1.0.0".into(),
	});
	binding.kind = BindingKind::Skill;
	agent.bindings.push(binding);
	assert_eq!(
		agent
			.normalize("aidash://node-a")
			.unwrap()
			.iter()
			.filter(|b| b.origin == BindingOrigin::SkillSupport)
			.count(),
		3
	);
	agent.remove_default.push("skill_read".into());
	assert!(agent.normalize("aidash://node-a").is_err());
}

#[test]
fn bundle_member_ids_are_unique_across_versions_and_nodes() {
	let member = QualifiedRef::builtin("aidash://node-a", "file_read");
	for duplicate in [
		QualifiedRef {
			version: "2.0.0".into(),
			..member.clone()
		},
		QualifiedRef {
			registry_deployment: "aidash://node-b".into(),
			..member.clone()
		},
		member.clone(),
	] {
		assert!(
			BundleConfig {
				members: vec![member.clone(), duplicate]
			}
			.validate()
			.is_err()
		);
	}
	BundleConfig {
		members: vec![
			member,
			QualifiedRef::builtin("aidash://node-b", "file_search"),
		],
	}
	.validate()
	.unwrap();
}

#[rstest::rstest]
#[case::tool(BindingKind::Tool, true)]
#[case::bundle(BindingKind::Bundle, true)]
#[case::skill(BindingKind::Skill, false)]
#[case::memory(BindingKind::Memory, false)]
#[case::source(BindingKind::Source, false)]
#[case::decider(BindingKind::Decider, false)]
fn concurrency_narrowing_requires_a_tool_or_bundle_binding(
	#[case] kind: BindingKind,
	#[case] accepted: bool,
) {
	// Arrange: a Binding of each kind, valid without the restriction.
	let mut binding = Binding::tool(QualifiedRef {
		registry_deployment: "aidash://node-a".into(),
		id: "target".into(),
		version: "1.0.0".into(),
	});
	binding.kind = kind;
	binding.validate().unwrap();
	// Act
	binding.narrow = serde_json::from_value(json!({"concurrency":"sequential"})).unwrap();
	// Assert: only Bindings that bind Tools may lower their concurrency.
	assert_eq!(binding.validate().is_ok(), accepted);
}

fn deferred() -> AgentBindings {
	let mut agent = agent();
	agent.exposure = Some(ExposurePolicy::Deferred(Default::default()));
	agent
}
fn skill_binding() -> Binding {
	Binding {
		kind: BindingKind::Skill,
		..Binding::tool(QualifiedRef {
			registry_deployment: "aidash://node-a".into(),
			id: "skill".into(),
			version: "1.0.0".into(),
		})
	}
}
fn targets(normalized: &[NormalizedBinding]) -> BTreeSet<String> {
	normalized
		.iter()
		.map(|b| b.binding.target.id.trim_start_matches("aidash.").to_owned())
		.collect()
}

#[test]
fn deferred_normalization_swaps_skill_tools_for_exposure_tools() {
	let legacy = agent().normalize("aidash://node-a").unwrap();
	let mut legacy_policy = agent();
	legacy_policy.exposure = Some(ExposurePolicy::Legacy);
	assert_eq!(legacy_policy.normalize("aidash://node-a").unwrap(), legacy);
	let normalized = deferred().normalize("aidash://node-a").unwrap();
	let (legacy_targets, deferred_targets) = (targets(&legacy), targets(&normalized));
	assert_eq!(
		deferred_targets
			.difference(&legacy_targets)
			.map(String::as_str)
			.collect::<BTreeSet<_>>(),
		EXPOSURE_TOOLS
			.iter()
			.copied()
			.chain([SKILL_ASSET_READ])
			.collect()
	);
	assert_eq!(
		legacy_targets
			.difference(&deferred_targets)
			.map(String::as_str)
			.collect::<BTreeSet<_>>(),
		SKILL_TOOLS.iter().copied().collect()
	);
	for binding in &normalized {
		let operation = binding.binding.target.id.trim_start_matches("aidash.");
		let required = REQUIRED_TOOLS.contains(&operation)
			|| EXPOSURE_TOOLS.contains(&operation)
			|| binding.binding.target.id == source_defaults::WORKSPACE_OBSERVATION_ID;
		assert_eq!(
			binding.origin == BindingOrigin::Required,
			required,
			"{operation}"
		);
	}
	let mut removed = deferred();
	removed.remove_default.push(EXPOSURE_TOOLS[0].into());
	assert!(removed.normalize("aidash://node-a").is_err());
	let mut removed = deferred();
	removed.remove_default.push("skill_read".into());
	assert!(removed.validate().is_err());
	let mut removed = deferred();
	removed.remove_default.push(SKILL_ASSET_READ.into());
	assert!(!targets(&removed.normalize("aidash://node-a").unwrap()).contains(SKILL_ASSET_READ));
	let mut removed = agent();
	removed.remove_default.push(SKILL_ASSET_READ.into());
	assert!(removed.validate().is_err());
}

#[test]
fn deferred_skills_require_skill_asset_read_as_support() {
	let mut agent = deferred();
	agent.bindings.push(skill_binding());
	let support = agent
		.normalize("aidash://node-a")
		.unwrap()
		.into_iter()
		.filter(|b| b.origin == BindingOrigin::SkillSupport)
		.map(|b| b.binding.target.id)
		.collect::<Vec<_>>();
	assert_eq!(support, [format!("aidash.{SKILL_ASSET_READ}")]);
	agent.remove_default.push(SKILL_ASSET_READ.into());
	assert!(agent.normalize("aidash://node-a").is_err());
}

#[test]
fn binding_exposure_requires_deferred_policy_and_exposable_kind() {
	let mut eager = skill_binding();
	eager.exposure = Some(BindingExposure::Eager);
	let mut agent = deferred();
	agent.bindings.push(eager.clone());
	agent.validate().unwrap();
	agent.exposure = Some(ExposurePolicy::Legacy);
	assert!(agent.validate().is_err());
	agent.exposure = None;
	assert!(agent.validate().is_err());
	let mut agent = deferred();
	agent.bindings.push(Binding {
		kind: BindingKind::Memory,
		..eager
	});
	assert!(agent.validate().is_err());
	let mut agent = deferred();
	agent.exposure = Some(ExposurePolicy::Deferred(crate::exposure::DeferredBudgets {
		metadata_bytes: 100,
		..Default::default()
	}));
	assert!(agent.validate().is_err());
}

#[test]
fn legacy_agent_json_round_trips_byte_identically() {
	let input = json!({
		"schema_version": 1,
		"model": {"id": "model", "version": "1.0.0"},
		"instructions": "Work on the Task.",
		"bindings": [{
			"kind": "tool",
			"target": {"registry_deployment": "aidash://node-a", "id": "aidash.file_read", "version": "1.0.0"},
			"alias": "read",
			"narrow": {}
		}],
		"remove_default": ["memory_mutate"],
		"cluster": null,
		"max_steps": 9
	});
	let parsed: AgentBindings = serde_json::from_value(input.clone()).unwrap();
	assert_eq!(parsed.exposure, None);
	let encoded = serde_json::to_string(&parsed).unwrap();
	assert!(!encoded.contains("exposure"));
	let reparsed: AgentBindings = serde_json::from_str(&encoded).unwrap();
	assert_eq!(serde_json::to_string(&reparsed).unwrap(), encoded);
	let normalized = serde_json::to_string(&parsed.normalize("aidash://node-a").unwrap()).unwrap();
	assert!(!normalized.contains("exposure"));
	let config: crate::registry::AgentConfig = serde_json::from_value(input.clone()).unwrap();
	assert_eq!(serde_json::to_string(&config).unwrap(), encoded);
	assert_eq!(config.exposure_policy(), ExposurePolicy::Legacy);

	let mut deferred = input;
	deferred["exposure"] = json!({"version": "deferred@1", "skill_bytes": 2048});
	let config: crate::registry::AgentConfig = serde_json::from_value(deferred).unwrap();
	assert_eq!(
		config.exposure_policy().budgets().unwrap().skill_bytes,
		2048
	);
	assert!(config.permits_builtin("capability_load"));
	assert!(config.permits_builtin(SKILL_ASSET_READ));
	assert!(!config.permits_builtin("skill_read"));
}

#[test]
fn declared_prompt_cache_survives_the_agent_settings_view() {
	// Arrange
	let mut input = serde_json::to_value(agent()).unwrap();
	assert!(input.get("prompt_cache").is_none());
	input["projection_version"] = json!("ordered");
	input["prompt_cache"] = json!("explicit");
	// Act
	let decoded: AgentBindings = serde_json::from_value(input.clone()).unwrap();
	let pinned = crate::registry::AgentConfig::from_definition(decoded).definition();
	// Assert
	assert_eq!(
		pinned.prompt_cache,
		Some(crate::projection::PromptCache::Explicit)
	);
	assert_eq!(serde_json::to_value(&pinned).unwrap(), input);
	input["prompt_cache"] = json!("automatic");
	assert!(serde_json::from_value::<AgentBindings>(input).is_err());
}
