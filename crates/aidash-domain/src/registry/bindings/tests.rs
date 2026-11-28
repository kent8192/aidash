use super::*;
use serde_json::{Value, json};

#[test]
fn reference_embedding_is_an_immutable_agent_definition_dependency() {
	let entry: Entry = serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"Agent"},"config":{"model":{"id":"model","version":"1.0.0"},"instructions":"Work","reference_embedding":{"id":"image-embedding","version":"2.0.0"}}})).unwrap();
	let root = QualifiedRef {
		registry_deployment: "aidash://node-a".into(),
		id: "agent".into(),
		version: "1.0.0".into(),
	};
	let references = definition_references(&root, &entry).unwrap();
	assert!(
		references
			.iter()
			.any(|(reference, kind)| reference.id == "image-embedding"
				&& reference.version == "2.0.0"
				&& kind == "embedding")
	);
	let mut config = entry.config;
	assert!(
		crate::registry::rules::overlay_config(&mut config, &json!({"reference_embedding":null}))
			.is_err()
	);
}

mod agent_plugins;

fn agent() -> AgentBindings {
	serde_json::from_value(
		json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Work on the Task."}),
	)
	.unwrap()
}

#[test]
fn built_in_memory_is_default_and_a_custom_provider_is_refused() {
	let node = "aidash://node-a";
	let mut config = agent();
	let defaults = config.normalize(node).unwrap();
	let memories: Vec<_> = defaults
		.iter()
		.filter(|binding| binding.binding.kind == BindingKind::Memory)
		.collect();
	assert_eq!(memories.len(), 1);
	assert_eq!(memories[0].binding.target.id, "aidash.memory");
	assert_eq!(memories[0].binding.target.version, "1.0.0");
	assert_eq!(memories[0].origin, BindingOrigin::Default);
	config.bindings.push(Binding {
		kind: BindingKind::Memory,
		target: QualifiedRef {
			registry_deployment: node.into(),
			id: "custom-memory".into(),
			version: "1.0.0".into(),
		},
		alias: None,
		narrow: Narrowing::default(),
		members: vec![],
		exposure: None,
	});
	assert!(matches!(
		config.normalize(node),
		Err(crate::Error::Invalid(_))
	));
}

#[test]
fn builtin_memory_algorithms_are_internal_but_custom_provider_roles_are_dependencies() {
	let entry = memory_defaults::definitions()
		.unwrap()
		.into_iter()
		.find(|entry| entry.kind == "memory")
		.unwrap();
	let identity = QualifiedRef::builtin("aidash://node-a", "memory");
	let references = definition_references(&identity, &entry).unwrap();
	assert!(
		!references
			.iter()
			.any(|(_, kind)| kind == "reranker" || kind == "tokenizer")
	);
	let mut custom = entry.clone();
	custom.id = "tenant.memory".into();
	let references = definition_references(&identity, &custom).unwrap();
	assert!(
		references
			.iter()
			.any(|(target, kind)| target.id == memory_defaults::RERANKER_ID && kind == "reranker")
	);
	assert!(
		references
			.iter()
			.any(|(target, kind)| target.id == memory_defaults::TOKENIZER_ID && kind == "tokenizer")
	);
}

#[test]
fn agent_model_roles_do_not_add_registry_dependencies() {
	let mut policy: Value = serde_json::from_str(include_str!(
		"../../../../../tests/fixtures/native_memory_policy_baseline.json"
	))
	.unwrap();
	for role in ["extraction", "derivation", "reflection"] {
		policy[role] = json!("agent_model");
	}
	policy["embedding"] = Value::Null;
	policy["reranker"] = json!({"id":"rrf","version":"1.0.0"});
	policy["tokenizer"] = json!({"id":"utf8","version":"1.0.0"});
	let entry: Entry = serde_json::from_value(json!({"id":"memory","version":"1.0.0","kind":"memory","name":{"en":"Memory"},"description":{"en":"Memory"},"config":{"engine":"hindsight_rust","policy":policy}})).unwrap();
	let identity = QualifiedRef {
		registry_deployment: "aidash://node-a".into(),
		id: "memory".into(),
		version: "1.0.0".into(),
	};
	let refs = definition_references(&identity, &entry).unwrap();
	assert_eq!(
		refs.iter()
			.map(|(reference, kind)| (reference.id.as_str(), kind.as_str()))
			.collect::<Vec<_>>(),
		vec![("rrf", "reranker"), ("utf8", "tokenizer")]
	);
}
#[rstest::rstest]
fn agent_config_rejects_removed_cluster_field() {
	let input = json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Work on the Task.","cluster":null});
	let error = serde_json::from_value::<AgentBindings>(input).unwrap_err();
	assert!(error.to_string().contains("unknown field `cluster`"));
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
			json!({"model":{"id":"model","version":"1.0.0"},"instructions":"old","exposure":{"version":"legacy@1"}})
		)
		.is_err()
	);
}
#[test]
fn obsolete_compaction_default_is_rejected() {
	let mut definition = agent();
	definition.remove_default.push("compaction".into());
	assert!(definition.validate().is_err());
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

/// A saved `workspace_wait` 1.0.0 resolves as 1.1.0, which only adds inputs.
fn with_current_wait(mut bindings: Vec<NormalizedBinding>) -> Vec<NormalizedBinding> {
	for item in &mut bindings {
		if item.binding.target.id == "aidash.workspace_wait" {
			item.binding.target.version = "1.1.0".into();
		}
	}
	bindings
}

#[test]
fn existing_agent_normalization_keeps_its_original_system_bindings() {
	let config = agent();
	let old = config.normalize_mode("aidash://node-a", 1, true).unwrap();
	let mut entry: Entry = serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"Agent"},"config":config})).unwrap();
	entry.binding_normalization = Some(AgentNormalization {
		schema_version: BINDING_SCHEMA,
		registry_deployment: "aidash://node-a".into(),
		bindings: old.clone(),
	});
	assert_eq!(
		entry.normalized_bindings("aidash://node-a").unwrap(),
		with_current_wait(old)
	);
	entry.version = "1.1.0".into();
	entry.normalize_agent("aidash://node-a").unwrap();
	// A new version binds today's defaults: the Thread tools (#1008).
	let current = entry.normalized_bindings("aidash://node-a").unwrap();
	assert!(
		current
			.iter()
			.any(|b| b.binding.target.id == "aidash.thread_search")
	);
	assert!(current.iter().any(
		|b| b.binding.target.id == "aidash.thread_read" && b.binding.target.version == "1.1.0"
	));
}

#[test]
fn historical_agent_normalization_remains_valid_and_tampering_is_rejected() {
	let node = "aidash://node-a";
	let config = agent();
	let old = config.normalize_mode(node, 1, true).unwrap();
	let mut entry: Entry = serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"Agent"},"schema":{},"config":config})).unwrap();
	entry.binding_normalization = Some(AgentNormalization {
		schema_version: BINDING_SCHEMA,
		registry_deployment: node.into(),
		bindings: old.clone(),
	});
	assert!(!entry.supports_thread_attachment_selection(node).unwrap());
	let mut current = entry.clone();
	current.binding_normalization = None;
	current.normalize_agent(node).unwrap();
	assert!(current.supports_thread_attachment_selection(node).unwrap());
	assert_eq!(
		entry.normalized_bindings(node).unwrap(),
		with_current_wait(old.clone())
	);
	assert!(
		!old.iter()
			.any(|binding| binding.binding.target.id == "aidash.attachment_copy")
	);
	assert!(
		config
			.normalize(node)
			.unwrap()
			.iter()
			.any(|binding| binding.binding.target.id == "aidash.attachment_copy")
	);
	entry.binding_normalization.as_mut().unwrap().bindings.pop();
	assert!(entry.normalized_bindings(node).is_err());
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
fn an_explicit_source_does_not_restore_removed_memory_tools() {
	let node = "aidash://node-a";
	let mut config = agent();
	config.bindings.push(Binding {
		kind: BindingKind::Source,
		target: QualifiedRef {
			registry_deployment: node.into(),
			id: "retrieval".into(),
			version: "1.0.0".into(),
		},
		alias: None,
		narrow: Narrowing::default(),
		members: vec![],
		exposure: None,
	});
	config
		.remove_default
		.extend(MEMORY_TOOLS.iter().map(|tool| (*tool).to_owned()));
	let normalized = config.normalize(node).unwrap();
	assert!(
		normalized
			.iter()
			.any(|binding| binding.binding.target.id == "retrieval")
	);
	for tool in MEMORY_TOOLS {
		assert!(
			!normalized
				.iter()
				.any(|binding| binding.binding.target.id == format!("aidash.{tool}")),
			"{tool}"
		);
	}
}

#[test]
fn memory_builtins_stay_default_bindings_and_may_be_explicit() {
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
	// They are offered with the built-in provider by default.
	let mut agent = agent();
	assert_eq!(memory_tools(&agent), vec![BindingOrigin::Default; 3]);
	let explicit = Binding::tool(QualifiedRef::builtin("aidash://node-a", "memory_recall"));
	agent.bindings.push(explicit.clone());
	agent.validate().unwrap();
	assert!(memory_tools(&agent).contains(&BindingOrigin::Explicit));
	agent.bindings.clear();
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
		"model": {"id": "model", "version": "1.0.0"},
		"instructions": "Work on the Task.",
		"bindings": [{
			"kind": "tool",
			"target": {"registry_deployment": "aidash://node-a", "id": "aidash.file_read", "version": "1.0.0"},
			"alias": "read",
			"narrow": {}
		}],
		"remove_default": ["memory_mutate"]
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
	input["projection_version"] = json!("native");
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

#[test]
fn omitted_rendering_pins_keep_the_definition_bytes() {
	// Arrange: no definition, digest or Binding snapshot written before #531
	// names a Harness Contract.
	let input = serde_json::to_value(agent()).unwrap();
	assert!(input.get("harness_contract").is_none());
	// Act
	let parsed: AgentBindings = serde_json::from_value(input.clone()).unwrap();
	let config = crate::registry::AgentConfig::from_definition(parsed);
	// Assert
	assert_eq!(config.projection_version, None);
	assert_eq!(config.harness_contract, None);
	assert_eq!(
		config.harness_contract.unwrap_or_default(),
		crate::harness_contract::HarnessContract::Structured4
	);
	assert_eq!(serde_json::to_value(config).unwrap(), input);
}

#[test]
fn unknown_harness_contract_versions_are_rejected() {
	let mut input = serde_json::to_value(agent()).unwrap();
	// structured@1 and structured@2 were removed by #532; nothing accepts them.
	for contract in ["legacy@1", "structured@1", "structured@2", "structured@5"] {
		input["harness_contract"] = json!(contract);
		assert!(serde_json::from_value::<AgentBindings>(input.clone()).is_err());
	}
}

#[test]
fn a_workbench_test_excludes_only_the_schedule_tools() {
	// Arrange: one Schedule tool and one other default tool.
	let bound = |operation: &str| {
		let descriptor =
			crate::tool::providers::core_descriptor("aidash://node-a", operation).unwrap();
		let definition: Entry = serde_json::from_value(json!({"id":operation,"version":"1.0.0","kind":"tool","name":{"en":operation},"description":{"en":operation},"config":descriptor})).unwrap();
		ResolvedBinding {
			identity: QualifiedRef {
				registry_deployment: "aidash://node-a".into(),
				id: operation.into(),
				version: "1.0.0".into(),
			},
			definition,
			digest: String::new(),
			origin: BindingOrigin::Default,
			alias: Some(operation.into()),
			narrow: Default::default(),
			installation: None,
			provider_contract_digest: None,
			provider_implementation: None,
			excluded_reason: None,
		}
	};
	let snapshot = BindingSnapshot {
		schema_version: 1,
		agent: QualifiedRef {
			registry_deployment: "aidash://node-a".into(),
			id: "agent".into(),
			version: "1.0.0".into(),
		},
		remote: false,
		bindings: vec![bound("duty_create"), bound("task_create")],
		definitions: vec![],
		agent_plugins: vec![],
	};
	// Act
	let tested = snapshot.for_workbench_test();
	// Assert
	assert_eq!(
		tested.bindings[0].excluded_reason.as_deref(),
		Some(WORKBENCH_SCHEDULE_EXCLUSION)
	);
	assert_eq!(tested.bindings[1].excluded_reason, None);
}

#[test]
fn an_agent_normalized_before_the_schedule_tools_loads_without_them() {
	// Arrange: today's defaults as stored before #557 added the four
	// Schedule tools, and a #640-era (mode 2) normalization.
	let node = "aidash://node-a";
	let config = agent();
	let schedule_tool = |binding: &NormalizedBinding| {
		SCHEDULE_TOOLS
			.iter()
			.any(|tool| binding.binding.target == QualifiedRef::builtin(node, tool))
	};
	// Before #745 too, so its file tools are at 1.0.0 (set 6).
	let mut saved = config.normalize_mode(node, 6, true).unwrap();
	assert_eq!(saved.iter().filter(|b| schedule_tool(b)).count(), 4);
	saved.retain(|binding| !schedule_tool(binding));
	let mut entry: Entry = serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"Agent"},"config":config})).unwrap();
	entry.binding_normalization = Some(AgentNormalization {
		schema_version: BINDING_SCHEMA,
		registry_deployment: node.into(),
		bindings: saved,
	});

	// Act
	let resolved = entry.normalized_bindings(node).unwrap();
	let historical = config.normalize_mode(node, 2, true).unwrap();

	// Assert: what it pinned stands; no historical set has the new tools.
	assert!(!resolved.iter().any(schedule_tool));
	assert!(!historical.iter().any(schedule_tool));
}

#[test]
fn an_agent_normalized_before_the_execution_pod_keeps_its_1_0_0_file_tools() {
	// Arrange: the defaults before #1008 (set 7), schedule tools included, as
	// stored before #745 moved file_read, file_search and attachment_copy to
	// 1.0.1.
	let node = "aidash://node-a";
	let config = agent();
	let today = config.normalize_mode(node, 7, true).unwrap();
	let mut saved = today.clone();
	for item in &mut saved {
		if POD_FILE_TOOLS
			.iter()
			.any(|tool| item.binding.target.id == QualifiedRef::builtin(node, tool).id)
		{
			item.binding.target.version = "1.0.0".into();
		}
	}
	assert_ne!(saved, today);
	let mut entry: Entry = serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"Agent"},"config":config})).unwrap();
	entry.binding_normalization = Some(AgentNormalization {
		schema_version: BINDING_SCHEMA,
		registry_deployment: node.into(),
		bindings: saved.clone(),
	});

	// Act
	let resolved = entry.normalized_bindings(node).unwrap();

	// Assert: the pinned 1.0.0 file tools and the later 1.0.1 set still resolve.
	assert_eq!(resolved, saved);
	let file_read = |bindings: &[NormalizedBinding]| {
		bindings
			.iter()
			.find(|b| b.binding.target.id == "aidash.file_read")
			.unwrap()
			.binding
			.target
			.version
			.clone()
	};
	assert_eq!(file_read(&resolved), "1.0.0");
	assert_eq!(file_read(&today), "1.1.0");
}

#[test]
fn an_agent_normalized_before_attachment_image_search_binds_the_1_1_0_tools() {
	// Arrange: a normalization saved between image retrieval (#639) and
	// Thread Attachment image search (#622 part 2), as it was stored then
	// (the defaults before #1008, set 7).
	let node = "aidash://node-a";
	let config = agent();
	let mut saved = config.normalize_mode(node, 7, true).unwrap();
	for item in &mut saved {
		match item.binding.target.id.as_str() {
			"aidash.workspace_message" => item.binding.target.version = "1.0.1".into(),
			"aidash.workspace_search" => item.binding.target.version = "1.0.0".into(),
			_ => {}
		}
	}
	let version = |bindings: &[NormalizedBinding], id: &str| {
		bindings
			.iter()
			.find(|binding| binding.binding.target.id == id)
			.map(|binding| binding.binding.target.version.clone())
	};
	let mut entry: Entry = serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"Agent"},"config":config})).unwrap();
	entry.binding_normalization = Some(AgentNormalization {
		schema_version: BINDING_SCHEMA,
		registry_deployment: node.into(),
		bindings: saved.clone(),
	});

	// Act
	let resolved = entry.normalized_bindings(node).unwrap();

	// Assert: the two tools move to 1.1.0; workspace_read keeps its pin.
	assert_eq!(
		version(&resolved, "aidash.workspace_message").as_deref(),
		Some("1.1.0")
	);
	assert_eq!(
		version(&resolved, "aidash.workspace_search").as_deref(),
		Some("1.1.0")
	);
	assert_eq!(
		version(&resolved, "aidash.workspace_read"),
		version(&saved, "aidash.workspace_read")
	);
	entry.binding_normalization.as_mut().unwrap().bindings.pop();
	assert!(entry.normalized_bindings(node).is_err());
}

#[test]
fn an_agent_from_before_image_retrieval_binds_thread_message_1_1_0() {
	// Arrange: the #640-era defaults (mode 2) with workspace_message 1.0.1.
	let node = "aidash://node-a";
	let config = agent();
	let mut saved = config.normalize_mode(node, 2, true).unwrap();
	for item in &mut saved {
		if item.binding.target.id == "aidash.workspace_message" {
			item.binding.target.version = "1.0.1".into();
		}
	}
	let mut entry: Entry = serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"Agent"},"config":config})).unwrap();
	entry.binding_normalization = Some(AgentNormalization {
		schema_version: BINDING_SCHEMA,
		registry_deployment: node.into(),
		bindings: saved,
	});

	// Act
	let resolved = entry.normalized_bindings(node).unwrap();

	// Assert
	let message = resolved
		.iter()
		.find(|item| item.binding.target.id == "aidash.workspace_message")
		.unwrap();
	assert_eq!(message.binding.target.version, "1.1.0");
	let read = resolved
		.iter()
		.find(|item| item.binding.target.id == "aidash.workspace_read")
		.unwrap();
	assert_eq!(read.binding.target.version, "1.0.1");
	assert!(
		!resolved
			.iter()
			.any(|item| item.binding.target.id == "aidash.workspace_search")
	);
}
