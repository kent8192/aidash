use super::*;
use aidash_domain::tool::{ToolContract, providers::*};
use async_trait::async_trait;
use serde_json::{Value, json};

const NODE: &str = "aidash://node-a";
fn reference(id: &str) -> QualifiedRef {
	QualifiedRef {
		registry_node: NODE.into(),
		id: id.into(),
		version: "1.0.0".into(),
	}
}
fn entry(id: &str, kind: &str, config: Value) -> Entry {
	Entry {
		binding_normalization: None,
		installation: None,
		id: id.into(),
		version: "1.0.0".into(),
		kind: kind.into(),
		name: BTreeMap::from([("en".into(), id.into())]),
		description: BTreeMap::from([("en".into(), id.into())]),
		capabilities: vec![],
		tags: vec![],
		languages: vec![],
		skills: vec![],
		schema: json!({"type":"object"}),
		config,
	}
}
struct Catalog {
	entries: BTreeMap<QualifiedRef, Entry>,
	reject_installations: bool,
	foreign: BTreeMap<QualifiedRef, ForeignAgentSnapshot>,
}
impl Catalog {
	fn new() -> Self {
		let mut result = Self {
			entries: BTreeMap::new(),
			reject_installations: false,
			foreign: BTreeMap::new(),
		};
		result.insert(entry("model", "model", json!({})));
		for operation in crate::registry::system::operations() {
			result.core(operation);
		}
		result
	}
	fn insert(&mut self, entry: Entry) {
		self.entries.insert(reference(&entry.id), entry);
	}
	fn core(&mut self, operation: &str) -> QualifiedRef {
		let descriptor = core_descriptor(NODE, operation).unwrap();
		let reference = QualifiedRef::builtin(NODE, operation);
		self.insert(entry(
			&reference.id,
			"tool",
			serde_json::to_value(descriptor).unwrap(),
		));
		reference
	}
	fn bundle(&mut self, id: &str, members: Vec<QualifiedRef>) -> QualifiedRef {
		self.insert(entry(id, "bundle", json!({"members":members})));
		reference(id)
	}
}
#[async_trait]
impl BindingCatalog for Catalog {
	async fn foreign_agent(&mut self, reference: &QualifiedRef) -> Result<ForeignAgentSnapshot> {
		self.foreign.get(reference).cloned().ok_or(Error::Forbidden)
	}
	async fn definition(&mut self, reference: &QualifiedRef) -> Result<Entry> {
		self.entries
			.get(reference)
			.cloned()
			.ok_or_else(|| Error::NotFound(reference.id.clone()))
	}
	async fn installation(&mut self, _: &aidash_domain::registry::Projection) -> Result<()> {
		if self.reject_installations {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn source(&mut self, _: &Entry) -> Result<()> {
		Ok(())
	}
}
struct Providers {
	unavailable: Option<String>,
}
impl ProviderCatalog for Providers {
	fn decision_implementation(
		&self,
		config: &aidash_domain::decision::DeciderConfig,
	) -> Result<String> {
		config.validate()?;
		if self.unavailable.as_deref() == Some("decision") {
			return Err(Error::Invalid("DECISION_PROVIDER_UNAVAILABLE".into()));
		}
		Ok("node-a/decision-adapter-v2".into())
	}
	fn contract(
		&self,
		descriptor: &ToolDescriptor,
		identity: &QualifiedRef,
	) -> Result<ToolContract> {
		Ok(descriptor.declared_contract(identity.clone())?)
	}
	fn implementation(&self, descriptor: &ToolDescriptor) -> Result<String> {
		if self.unavailable.as_deref() == Some(&descriptor.operation) {
			return Err(Error::Invalid("PROVIDER_UNAVAILABLE".into()));
		}
		Ok("test-implementation".into())
	}
}
fn agent_config() -> AgentBindings {
	serde_json::from_value(
		json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Test"}),
	)
	.unwrap()
}
fn agent_entry(config: &AgentBindings) -> Entry {
	entry("agent", "agent", serde_json::to_value(config).unwrap())
}
fn providers() -> Providers {
	Providers { unavailable: None }
}
async fn snapshot(
	catalog: &mut Catalog,
	config: &AgentBindings,
	remote: bool,
) -> Result<BindingSnapshot> {
	resolve(
		catalog,
		&providers(),
		reference("agent"),
		&agent_entry(config),
		remote,
	)
	.await
}

#[rstest::rstest]
#[case("reference_attachments", 1, 1, false, true)]
#[case("reference_attachments", 1, 1, true, false)]
#[case("reference_attachments", 5, 4, false, false)]
#[case("skill_attachments", 1, 1, false, true)]
#[case("skill_attachments", 1, 1, true, false)]
#[case("skill_attachments", 9, 8, false, false)]
#[case("skill_roots", 1, 1, false, true)]
#[case("skill_roots", 1, 1, true, false)]
#[case("skill_roots", 5, 4, false, false)]
#[tokio::test]
async fn mounted_source_aggregation_rejects_ambiguous_ids_and_shared_limits(
	#[case] adapter: &str,
	#[case] first_count: usize,
	#[case] second_count: usize,
	#[case] duplicate: bool,
	#[case] accepted: bool,
) {
	fn source(adapter: &str, count: usize, offset: usize) -> Value {
		let ids = (offset..offset + count).map(|id| uuid::Uuid::from_u128(id as u128));
		match adapter {
			"reference_attachments" => {
				json!({"adapter":adapter,"references":ids.map(|id|json!({"reference_id":id,"digest":"a".repeat(64)})).collect::<Vec<_>>()})
			}
			"skill_attachments" => json!({"adapter":adapter,"attachments":ids.map(|id| {
				let mut skill: aidash_domain::capabilities::SkillAttachment = serde_json::from_value(json!({"skill_id":id,"origin":"fixture","digest":"","instructions":"---\nname: test\ndescription: Fixture\n---\nRead this.","files":[]})).unwrap();
				skill.digest = aidash_domain::capabilities::skills::content_digest(&skill);
				skill
			}).collect::<Vec<_>>()}),
			_ => {
				json!({"adapter":adapter,"roots":(offset..offset+count).map(|id|format!("root-{id}/.agents/skills")).collect::<Vec<_>>()})
			}
		}
	}
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	for (id, count, offset) in [
		("first", first_count, 1),
		("second", second_count, if duplicate { 1 } else { 21 }),
	] {
		catalog.insert(entry(
			id,
			"source",
			json!({"schema_version":1,"source":source(adapter,count,offset)}),
		));
		config.bindings.push(Binding {
			kind: BindingKind::Source,
			target: reference(id),
			alias: None,
			narrow: Default::default(),
			members: vec![],
			exposure: None,
		});
	}
	let result = snapshot(&mut catalog, &config, false).await;
	assert_eq!(result.is_ok(), accepted, "{adapter}: {result:?}");
	if let Ok(snapshot) = result {
		snapshot.validate().unwrap();
		aidash_domain::registry::AgentConfig::from_snapshot(&snapshot).unwrap();
	} else {
		assert!(result.unwrap_err().to_string().contains("aggregate"));
	}
}

#[tokio::test]
async fn defaults_are_exact_and_a_saved_snapshot_is_immutable() {
	let mut catalog = Catalog::new();
	let saved = snapshot(&mut catalog, &agent_config(), false)
		.await
		.unwrap();
	let encoded = serde_json::to_value(&saved).unwrap();
	catalog
		.entries
		.get_mut(&QualifiedRef::builtin(NODE, "workspace_read"))
		.unwrap()
		.name
		.insert("en".into(), "changed".into());
	assert_eq!(serde_json::to_value(&saved).unwrap(), encoded);
	assert_eq!(
		saved.bindings.len(),
		REQUIRED_TOOLS.len() + DEFAULT_TOOLS.len()
	);
	assert_eq!(saved.definitions.len(), saved.bindings.len() + 2);
	let recovered: BindingSnapshot = serde_json::from_value(encoded).unwrap();
	recovered.validate().unwrap();
}
#[tokio::test]
async fn missing_required_and_tampered_definitions_fail_closed() {
	let mut catalog = Catalog::new();
	let mut saved = snapshot(&mut catalog, &agent_config(), false)
		.await
		.unwrap();
	saved
		.bindings
		.retain(|binding| binding.alias.as_deref() != Some("workspace_read"));
	assert!(saved.validate().is_err());
	let mut saved = snapshot(&mut catalog, &agent_config(), false)
		.await
		.unwrap();
	saved.definitions[0]
		.definition
		.name
		.insert("en".into(), "changed".into());
	assert!(saved.validate().is_err());
	catalog
		.entries
		.remove(&QualifiedRef::builtin(NODE, "human_request"));
	assert!(
		snapshot(&mut catalog, &agent_config(), false)
			.await
			.is_err()
	);
}
#[tokio::test]
async fn python_start_operations_share_pinned_companions() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	let code = catalog.core("code_interpreter");
	let install = catalog.core("python_install");
	catalog.core("python_poll");
	catalog.core("python_cancel");
	let bundle = catalog.bundle("python", vec![code, install]);
	config.bindings.push(Binding {
		kind: BindingKind::Bundle,
		target: bundle,
		alias: None,
		narrow: Narrowing::default(),
		members: vec![],
		exposure: None,
	});
	let saved = snapshot(&mut catalog, &config, false).await.unwrap();
	for operation in [
		"code_interpreter",
		"python_install",
		"python_poll",
		"python_cancel",
	] {
		assert_eq!(
			saved
				.bindings
				.iter()
				.filter(|binding| binding.alias.as_deref() == Some(operation))
				.count(),
			1
		);
	}
	assert_eq!(
		saved
			.bindings
			.iter()
			.filter(|binding| binding.origin == BindingOrigin::Companion)
			.count(),
		2
	);
}
#[tokio::test]
async fn selecting_async_start_also_pins_companions_but_not_other_starts() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	let code = catalog.core("code_interpreter");
	let install = catalog.core("python_install");
	catalog.core("python_poll");
	catalog.core("python_cancel");
	let bundle = catalog.bundle("python", vec![code.clone(), install]);
	config.bindings.push(Binding {
		kind: BindingKind::Bundle,
		target: bundle,
		alias: None,
		narrow: Narrowing::default(),
		members: vec![code.id],
		exposure: None,
	});
	let saved = snapshot(&mut catalog, &config, false).await.unwrap();
	assert!(
		saved
			.bindings
			.iter()
			.any(|binding| binding.alias.as_deref() == Some("python_poll"))
	);
	assert!(
		!saved
			.bindings
			.iter()
			.any(|binding| binding.alias.as_deref() == Some("python_install"))
	);
}
#[tokio::test]
async fn incompatible_companions_cycles_and_undeclared_members_are_rejected() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	let shell = catalog.core("shell");
	catalog.core("shell_poll");
	catalog.core("shell_cancel");
	let poll = catalog
		.entries
		.get_mut(&QualifiedRef::builtin(NODE, "shell_poll"))
		.unwrap();
	poll.config = serde_json::to_value(core_descriptor(NODE, "python_poll").unwrap()).unwrap();
	config.bindings.push(Binding::tool(shell));
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	let cycle = catalog.bundle("cycle", vec![reference("cycle")]);
	config.bindings.push(Binding {
		kind: BindingKind::Bundle,
		target: cycle,
		alias: None,
		narrow: Narrowing::default(),
		members: vec![],
		exposure: None,
	});
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
	config.bindings[0].members = vec!["absent".into()];
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
}
#[tokio::test]
async fn alias_collisions_are_checked_after_expansion() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	let get = catalog.core("outbound_get");
	let patch = catalog.core("apply_patch");
	let mut a = Binding::tool(get);
	a.alias = Some("lookup".into());
	let mut b = Binding::tool(patch);
	b.alias = Some("lookup".into());
	config.bindings = vec![a, b];
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
	config.bindings.pop();
	config.bindings[0].alias = Some("workspace_read".into());
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
}
#[tokio::test]
async fn remote_defaults_have_durable_exclusion_reasons_and_explicit_inputs_fail() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	let saved = resolve(
		&mut catalog,
		&Providers {
			unavailable: Some("memory_mutate".into()),
		},
		reference("agent"),
		&agent_entry(&config),
		true,
	)
	.await
	.unwrap();
	let memory = saved
		.bindings
		.iter()
		.find(|binding| binding.identity.id == "aidash.memory_mutate")
		.unwrap();
	assert!(memory.excluded_reason.is_some());
	assert!(memory.provider_implementation.is_none());
	assert!(
		saved
			.bindings
			.iter()
			.any(|binding| binding.alias.as_deref() == Some("human_request")
				&& binding.excluded_reason.is_none())
	);
	config
		.bindings
		.push(Binding::tool(QualifiedRef::builtin(NODE, "memory_mutate")));
	assert!(snapshot(&mut catalog, &config, true).await.is_err());
}

#[tokio::test]
async fn recovered_remote_snapshots_preserve_exclusions_and_provider_evidence_at_run_boundaries() {
	let saved = snapshot(&mut Catalog::new(), &agent_config(), true)
		.await
		.unwrap();
	assert!(saved.remote);
	let encoded = serde_json::to_value(&saved).unwrap();
	let recovered: BindingSnapshot = serde_json::from_value(encoded.clone()).unwrap();
	recovered.validate().unwrap();
	let mut missing_placement = encoded;
	missing_placement.as_object_mut().unwrap().remove("remote");
	assert!(serde_json::from_value::<BindingSnapshot>(missing_placement).is_err());

	let mut run = admitted_run().await;
	run.context.binding_snapshot = None;
	run.home_node = "aidash://home".into();
	run.bind(recovered).unwrap();
	let live = Arc::new(Live::new());
	let resolver = execution::PinnedResolver {
		providers: live.clone(),
		authority: live.clone(),
	};
	let tools = resolver.tools(&run).await.unwrap();
	assert!(!tools.contains_key("memory_mutate"));
	assert!(tools.contains_key("workspace_read"));
	assert!(tools.contains_key("human_request"));

	for mutation in 0..6 {
		let mut altered = saved.clone();
		let memory = altered
			.bindings
			.iter_mut()
			.find(|binding| binding.alias.as_deref() == Some("memory_mutate"))
			.unwrap();
		match mutation {
			0 => {
				memory.excluded_reason = None;
				memory.provider_implementation = Some("injected-implementation".into());
			}
			1 => memory.excluded_reason = Some("different reason".into()),
			2 => memory.provider_implementation = Some("injected-implementation".into()),
			3 => memory.provider_contract_digest = Some("changed-contract".into()),
			4 => altered.remote = false,
			5 => {
				let read = altered
					.bindings
					.iter_mut()
					.find(|binding| binding.alias.as_deref() == Some("workspace_read"))
					.unwrap();
				read.provider_implementation = None;
			}
			_ => unreachable!(),
		}
		let altered: BindingSnapshot =
			serde_json::from_value(serde_json::to_value(altered).unwrap()).unwrap();
		assert!(altered.validate().is_err(), "mutation {mutation}");
		run.context.binding_snapshot = None;
		assert!(run.bind(altered.clone()).is_err(), "mutation {mutation}");
		assert!(run.context.binding_snapshot.is_none());
		run.context.binding_snapshot = Some(Box::new(altered));
		assert!(resolver.tools(&run).await.is_err(), "mutation {mutation}");
	}

	let mut local = snapshot(&mut Catalog::new(), &agent_config(), false)
		.await
		.unwrap();
	assert!(!local.remote);
	let memory = local
		.bindings
		.iter_mut()
		.find(|binding| binding.alias.as_deref() == Some("memory_mutate"))
		.unwrap();
	memory.excluded_reason = Some("provider contract is ineligible for remote execution".into());
	memory.provider_implementation = None;
	assert!(local.validate().is_err());
}
#[tokio::test]
async fn installed_exact_revisions_and_current_provider_availability_are_required() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	let get = catalog.core("outbound_get");
	catalog.entries.get_mut(&get).unwrap().installation =
		Some(aidash_domain::registry::Projection {
			contract: 1,
			tenant: "tenant".into(),
			installation: "install".into(),
			revision: 2,
		});
	config.bindings.push(Binding::tool(get));
	catalog.reject_installations = true;
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
	catalog.reject_installations = false;
	assert!(
		resolve(
			&mut catalog,
			&Providers {
				unavailable: Some("outbound_get".into())
			},
			reference("agent"),
			&agent_entry(&config),
			false
		)
		.await
		.is_err()
	);
}
#[tokio::test]
async fn node_identity_cannot_be_replaced_by_a_same_named_local_definition() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	let mut target = catalog.core("outbound_get");
	target.registry_node = "aidash://other".into();
	config.bindings.push(Binding::tool(target));
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
}
#[tokio::test]
async fn coordinator_requirements_follow_operation_semantics() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	catalog.insert(entry(
		"cluster",
		"cluster",
		json!({"coordinator":reference("agent").local()}),
	));
	config.cluster = Some(reference("cluster").local());
	config.remove_default = vec!["task_create".into()];
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
	let mut replacement = Binding::tool(QualifiedRef::builtin(NODE, "task_create"));
	replacement.alias = Some("create_child".into());
	config.bindings.push(replacement);
	snapshot(&mut catalog, &config, false).await.unwrap();
}

#[tokio::test]
async fn coordinator_bundle_selections_reject_undeclared_ids_even_with_complete_defaults() {
	for nested in [false, true] {
		let mut catalog = Catalog::new();
		let get = catalog.core("outbound_get");
		let selected = if nested {
			catalog.bundle("inner", vec![get])
		} else {
			get
		};
		let bundle = catalog.bundle("coordinator-tools", vec![selected.clone()]);
		let mut coordinator = agent_config();
		coordinator.bindings.push(Binding {
			kind: BindingKind::Bundle,
			target: bundle,
			alias: None,
			narrow: Default::default(),
			members: vec![selected.id.clone()],
			exposure: None,
		});
		catalog.insert(entry(
			"cluster",
			"cluster",
			json!({"coordinator":reference("coordinator").local()}),
		));
		let mut config = agent_config();
		config.cluster = Some(reference("cluster").local());
		catalog.insert(entry(
			"coordinator",
			"agent",
			serde_json::to_value(&coordinator).unwrap(),
		));
		snapshot(&mut catalog, &config, false).await.unwrap();
		for members in [
			vec!["absent".into()],
			vec![selected.id.clone(), "absent".into()],
		] {
			coordinator.bindings[0].members = members;
			let coordinator_entry = entry(
				"coordinator",
				"agent",
				serde_json::to_value(&coordinator).unwrap(),
			);
			catalog.insert(coordinator_entry.clone());
			let cluster_error = snapshot(&mut catalog, &config, false).await.unwrap_err();
			assert!(
				matches!(&cluster_error, Error::Invalid(message) if message.contains("undeclared"))
			);
			let root_error = resolve(
				&mut catalog,
				&providers(),
				reference("coordinator"),
				&coordinator_entry,
				false,
			)
			.await
			.unwrap_err();
			assert_eq!(cluster_error.to_string(), root_error.to_string());
		}
	}
}

use crate::ports::{
	bindings::{BindingAuthority, BindingResolver, ProviderSet},
	execution::ExecutionTool,
};

#[tokio::test]
async fn complete_snapshot_retains_delegation_and_cluster_dependencies_and_rejects_wrong_kinds() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	catalog.insert(agent_entry(&config));
	let mut delegated = agent_config();
	delegated.model = reference("delegated-model").local();
	catalog.insert(entry(
		"delegate",
		"agent",
		serde_json::to_value(&delegated).unwrap(),
	));
	catalog.insert(entry("delegated-model", "model", json!({})));
	let descriptor = ToolDescriptor {
		registry_node: NODE.into(),
		provider: "integration.agent@1".into(),
		operation: "invoke".into(),
		default_alias: "delegate_peer".into(),
		tier: ToolTier::Integration,
		narrow: Default::default(),
		lifecycle: None,
		transport: Some(aidash_domain::tool::ToolConfig::Agent {
			node_id: NODE.into(),
			agent: reference("delegate").local(),
		}),
	};
	catalog.insert(entry(
		"delegation",
		"tool",
		serde_json::to_value(descriptor).unwrap(),
	));
	config.bindings.push(Binding::tool(reference("delegation")));
	catalog.insert(entry(
		"cluster",
		"cluster",
		json!({"coordinator":reference("delegate").local()}),
	));
	config.cluster = Some(reference("cluster").local());
	let graph = snapshot(&mut catalog, &config, false).await.unwrap();
	assert!(
		graph
			.definitions
			.iter()
			.any(|r| r.identity == reference("delegate"))
	);
	assert!(
		graph
			.definitions
			.iter()
			.any(|r| r.identity == reference("delegated-model"))
	);
	let mut incomplete = graph.clone();
	incomplete
		.definitions
		.retain(|r| r.identity != reference("delegated-model"));
	assert!(incomplete.validate().is_err());
	catalog
		.entries
		.get_mut(&reference("delegated-model"))
		.unwrap()
		.kind = "skill".into();
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
}

#[tokio::test]
async fn root_installation_and_unselected_recursive_bundle_members_are_checked() {
	let mut catalog = Catalog::new();
	let config = agent_config();
	let mut root = agent_entry(&config);
	root.installation = Some(aidash_domain::registry::Projection {
		contract: 1,
		tenant: "tenant".into(),
		installation: "root".into(),
		revision: 1,
	});
	catalog.reject_installations = true;
	assert!(matches!(
		resolve(&mut catalog, &providers(), reference("agent"), &root, false).await,
		Err(Error::Forbidden)
	));
	catalog.reject_installations = false;
	let first = catalog.bundle("cycle-first", vec![reference("cycle-second")]);
	catalog.bundle("cycle-second", vec![first.clone()]);
	let safe = catalog.core("outbound_get");
	let bundle = catalog.bundle("selected", vec![safe.clone(), first]);
	let mut config = agent_config();
	config.bindings.push(Binding {
		kind: BindingKind::Bundle,
		target: bundle,
		alias: None,
		narrow: Default::default(),
		members: vec![safe.id],
		exposure: None,
	});
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
}

#[tokio::test]
async fn native_context_is_explicit_and_skill_sources_cannot_omit_support() {
	let mut catalog = Catalog::new();
	catalog.insert(entry(
		"memory",
		"memory",
		json!({"schema_version":1,"source":{"adapter":"conversation_memory"}}),
	));
	let mut config = agent_config();
	let initial = snapshot(&mut catalog, &config, false).await.unwrap();
	assert!(
		initial
			.bindings
			.iter()
			.all(|b| b.definition.kind != "memory")
	);
	config.bindings.push(Binding {
		kind: BindingKind::Memory,
		target: reference("memory"),
		alias: None,
		narrow: Default::default(),
		members: vec![],
		exposure: None,
	});
	let bound = snapshot(&mut catalog, &config, false).await.unwrap();
	assert!(
		bound
			.bindings
			.iter()
			.any(|b| b.identity == reference("memory"))
	);
	catalog
		.entries
		.get_mut(&reference("memory"))
		.unwrap()
		.config = json!({"schema_version":1,"source":{"adapter":"future_framework"}});
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
	config.bindings.clear();
	catalog.insert(entry(
		"skills-root",
		"source",
		json!({"schema_version":1,"source":{"adapter":"skill_roots","roots":[".agents/skills"]}}),
	));
	config.bindings.push(Binding {
		kind: BindingKind::Source,
		target: reference("skills-root"),
		alias: None,
		narrow: Default::default(),
		members: vec![],
		exposure: None,
	});
	config.instructions.clear();
	let sources = snapshot(&mut catalog, &config, false).await.unwrap();
	for operation in SKILL_TOOLS {
		assert!(
			sources
				.bindings
				.iter()
				.any(|b| b.identity == QualifiedRef::builtin(NODE, operation)
					&& b.origin == BindingOrigin::SkillSupport)
		);
	}
	assert!(snapshot(&mut catalog, &config, true).await.is_err());
	config.remove_default.push("skill_load".into());
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
}

#[tokio::test]
async fn deferred_skill_sources_require_exposure_support_with_canonical_aliases() {
	let mut catalog = Catalog::new();
	catalog.insert(entry(
		"skills-root",
		"source",
		json!({"schema_version":1,"source":{"adapter":"skill_roots","roots":[".agents/skills"]}}),
	));
	let mut config = agent_config();
	config.exposure = Some(aidash_domain::exposure::ExposurePolicy::Deferred(
		Default::default(),
	));
	config.instructions.clear();
	config.bindings.push(Binding {
		kind: BindingKind::Source,
		target: reference("skills-root"),
		alias: None,
		narrow: Default::default(),
		members: vec![],
		exposure: None,
	});
	let sources = snapshot(&mut catalog, &config, false).await.unwrap();
	sources.validate().unwrap();
	let origin = |operation: &str| {
		sources
			.bindings
			.iter()
			.find(|b| b.identity == QualifiedRef::builtin(NODE, operation))
			.map(|b| (b.origin, b.alias.clone()))
	};
	for operation in EXPOSURE_TOOLS {
		assert_eq!(
			origin(operation),
			Some((BindingOrigin::Required, Some(operation.to_string())))
		);
	}
	assert_eq!(
		origin(SKILL_ASSET_READ),
		Some((BindingOrigin::SkillSupport, Some(SKILL_ASSET_READ.into())))
	);
	for operation in SKILL_TOOLS {
		assert_eq!(origin(operation), None);
	}
	let mut renamed = config.clone();
	renamed.bindings.push(Binding {
		kind: BindingKind::Tool,
		target: QualifiedRef::builtin(NODE, SKILL_ASSET_READ),
		alias: Some("read_asset".into()),
		narrow: Default::default(),
		members: vec![],
		exposure: None,
	});
	assert!(snapshot(&mut catalog, &renamed, false).await.is_err());
	config.remove_default.push(SKILL_ASSET_READ.into());
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
}

#[rstest::rstest]
#[tokio::test]
async fn deferred_agents_reject_explicit_legacy_skill_readers_at_admission_and_recovery(
	#[values("skill_list", "skill_load", "skill_read")] operation: &str,
	#[values("admission", "recovery")] boundary: &str,
) {
	// Arrange: a legacy Agent may bind the reader explicitly.
	let mut catalog = Catalog::new();
	let reader = QualifiedRef::builtin(NODE, operation);
	let mut config = agent_config();
	config.bindings.push(Binding::tool(reader.clone()));
	let legacy = snapshot(&mut catalog, &config, false).await.unwrap();
	legacy.validate().unwrap();
	config.exposure = Some(aidash_domain::exposure::ExposurePolicy::Deferred(
		Default::default(),
	));
	// Act
	let result = if boundary == "admission" {
		snapshot(&mut catalog, &config, false)
			.await
			.map(|_| ())
			.map_err(|error| error.to_string())
	} else {
		// A restored deferred closure rejects it even when its normalization,
		// definitions and digests are internally consistent.
		let mut without = config.clone();
		without.bindings.retain(|binding| binding.target != reader);
		let mut saved = snapshot(&mut catalog, &without, false).await.unwrap();
		saved.validate().unwrap();
		let root = saved
			.definitions
			.iter_mut()
			.find(|definition| definition.identity == saved.agent)
			.unwrap();
		root.definition.config = serde_json::to_value(&config).unwrap();
		root.definition.normalize_agent(NODE).unwrap();
		root.digest = aidash_domain::registry::rules::digest(
			&serde_json::to_value(&root.definition).unwrap(),
		);
		let (binding, definition) = (
			legacy.bindings.iter().find(|b| b.identity == reader),
			legacy.definitions.iter().find(|d| d.identity == reader),
		);
		let mut binding = binding.unwrap().clone();
		binding.origin = BindingOrigin::Explicit;
		saved.bindings.push(binding);
		saved.definitions.push(definition.unwrap().clone());
		saved.validate().map_err(|error| error.to_string())
	};
	// Assert
	let error = result.expect_err("legacy Skill readers bypass deferred exposure");
	assert!(
		error.contains(&format!("{operation} cannot be bound under deferred@1")),
		"{boundary}: {error}"
	);
}

#[rstest::rstest]
#[case::roots("skill_roots")]
#[case::attachments("skill_attachments")]
#[tokio::test]
async fn skill_sources_require_canonical_support_aliases_at_admission_and_recovery(
	#[case] adapter: &str,
	#[values("skill_list", "skill_load", "skill_read")] operation: &str,
	#[values("admission", "recovery")] boundary: &str,
) {
	let mut catalog = Catalog::new();
	let source = if adapter == "skill_roots" {
		json!({"adapter":adapter,"roots":[".agents/skills"]})
	} else {
		let mut skill: aidash_domain::capabilities::SkillAttachment = serde_json::from_value(
			json!({"skill_id":uuid::Uuid::new_v4(),"origin":"fixture","digest":"","instructions":"---\nname: test\ndescription: Fixture\n---\nRead this.","files":[]}),
		)
		.unwrap();
		skill.digest = aidash_domain::capabilities::skills::content_digest(&skill);
		json!({"adapter":adapter,"attachments":[skill]})
	};
	catalog.insert(entry(
		"skills-source",
		"source",
		json!({"schema_version":1,"source":source}),
	));
	let mut config = agent_config();
	config.instructions.clear();
	config.bindings.push(Binding {
		kind: BindingKind::Source,
		target: reference("skills-source"),
		alias: None,
		narrow: Default::default(),
		members: vec![],
		exposure: None,
	});
	let mut support = Binding::tool(QualifiedRef::builtin(NODE, operation));
	support.alias = Some(operation.into());
	config.bindings.push(support);
	let mut saved = snapshot(&mut catalog, &config, false).await.unwrap();
	saved.validate().unwrap();

	config.bindings.last_mut().unwrap().alias = Some("renamed_skill_support".into());
	let result = if boundary == "admission" {
		snapshot(&mut catalog, &config, false)
			.await
			.map(|_| ())
			.map_err(|error| error.to_string())
	} else {
		// A restored closure must reject the same alias even when its Agent
		// normalization and all definition digests are internally consistent.
		let root = saved
			.definitions
			.iter_mut()
			.find(|definition| definition.identity == saved.agent)
			.unwrap();
		root.definition.config = serde_json::to_value(&config).unwrap();
		root.definition.normalize_agent(NODE).unwrap();
		root.digest = aidash_domain::registry::rules::digest(
			&serde_json::to_value(&root.definition).unwrap(),
		);
		saved
			.bindings
			.iter_mut()
			.find(|binding| binding.identity == QualifiedRef::builtin(NODE, operation))
			.unwrap()
			.alias = Some("renamed_skill_support".into());
		saved.validate().map_err(|error| error.to_string())
	};
	let error = result.expect_err("renamed Skill support must be rejected");
	assert!(error.contains("canonical"), "{boundary}: {error}");
}

#[tokio::test]
async fn full_host_bundles_share_explicit_and_generated_poll_cancel_members() {
	let mut catalog = Catalog::new();
	let operations = [
		"shell",
		"shell_poll",
		"shell_cancel",
		"code_interpreter",
		"python_install",
		"python_poll",
		"python_cancel",
	];
	let members = operations
		.into_iter()
		.map(|operation| catalog.core(operation))
		.collect();
	let bundle = catalog.bundle("host", members);
	let mut config = agent_config();
	config.bindings.push(Binding {
		kind: BindingKind::Bundle,
		target: bundle,
		alias: None,
		narrow: Default::default(),
		members: vec![],
		exposure: None,
	});
	let saved = snapshot(&mut catalog, &config, false).await.unwrap();
	for operation in operations {
		let bindings = saved
			.bindings
			.iter()
			.filter(|binding| binding.identity == QualifiedRef::builtin(NODE, operation))
			.collect::<Vec<_>>();
		assert_eq!(bindings.len(), 1);
		assert_eq!(bindings[0].origin, BindingOrigin::Explicit);
	}
}
use aidash_domain::{Run, RunControl, context::Context, provider::ToolSpec};
use std::sync::{
	Arc,
	atomic::{AtomicBool, AtomicUsize, Ordering},
};
struct Live {
	available: AtomicBool,
	authorized: AtomicBool,
	invocations: Arc<AtomicUsize>,
}
impl Live {
	fn new() -> Self {
		Self {
			available: AtomicBool::new(true),
			authorized: AtomicBool::new(true),
			invocations: Arc::new(AtomicUsize::new(0)),
		}
	}
}
impl ProviderCatalog for Live {
	fn contract(
		&self,
		descriptor: &ToolDescriptor,
		identity: &QualifiedRef,
	) -> Result<ToolContract> {
		Ok(descriptor.declared_contract(identity.clone())?)
	}
	fn implementation(&self, _: &ToolDescriptor) -> Result<String> {
		if self.available.load(Ordering::SeqCst) {
			Ok("implementation".into())
		} else {
			Err(Error::Invalid("PROVIDER_UNAVAILABLE".into()))
		}
	}
}
#[async_trait]
impl ProviderSet for Live {
	async fn bind(&self, _: &Run, binding: &ResolvedBinding) -> Result<Arc<dyn ExecutionTool>> {
		Ok(Arc::new(Echo {
			contract: recheck_provider(self, binding)?,
			invocations: self.invocations.clone(),
		}))
	}
}
#[async_trait]
impl BindingAuthority for Live {
	async fn check(&self, _: &Run, _: &ResolvedBinding) -> Result<()> {
		if self.authorized.load(Ordering::SeqCst) {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
}
struct Echo {
	contract: ToolContract,
	invocations: Arc<AtomicUsize>,
}
#[async_trait]
impl ExecutionTool for Echo {
	fn specification(&self) -> ToolSpec {
		ToolSpec {
			name: "unstable-provider-name".into(),
			description: "Changed provider description".into(),
			parameters: json!({"type":"string"}),
		}
	}
	fn contract(&self) -> ToolContract {
		self.contract.clone()
	}
	fn replay_safe(&self) -> bool {
		self.contract.replay_safe()
	}
	async fn invoke(&self, _: &Run, input: Value, _: &str) -> Result<Value> {
		self.invocations.fetch_add(1, Ordering::SeqCst);
		Ok(input)
	}
}
async fn admitted_run() -> Run {
	let mut config = agent_config();
	let mut read = Binding::tool(QualifiedRef::builtin(NODE, "workspace_read"));
	read.narrow.limits.insert("max_chars".into(), 128);
	config.bindings.push(read);
	let saved = snapshot(&mut Catalog::new(), &config, false).await.unwrap();
	let mut run = Run {
		id: uuid::Uuid::new_v4(),
		task_id: uuid::Uuid::new_v4(),
		workspace_id: uuid::Uuid::new_v4(),
		home_node: NODE.into(),
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		state_version: Default::default(),
		state: Default::default(),
		recovery: Default::default(),
		control: RunControl::Active,
		context: Context::default(),
		step: 0,
		revision: 0,
		observed_input_seq: 0,
		ledger_worker_ready: false,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	};
	run.bind(saved).unwrap();
	run
}
#[tokio::test]
async fn saved_dispatch_enforces_narrowing_and_rechecks_revocation_and_provider_loss_before_effects()
 {
	let run = admitted_run().await;
	let live = Arc::new(Live::new());
	let resolver = execution::PinnedResolver {
		providers: live.clone(),
		authority: live.clone(),
	};
	let tools = resolver.tools(&run).await.unwrap();
	let read = &tools["workspace_read"];
	assert_eq!(read.specification().name, "workspace_read");
	assert_eq!(read.specification().parameters, json!({"type":"object"}));
	let input = json!({"kind":"task","id":run.task_id});
	let output = read.invoke(&run, input.clone(), "one").await.unwrap();
	assert_eq!(output["max_chars"], 128);
	assert!(
		read.invoke(&run, json!({"max_chars":129}), "two")
			.await
			.is_err()
	);
	live.authorized.store(false, Ordering::SeqCst);
	assert!(read.invoke(&run, input.clone(), "three").await.is_err());
	live.authorized.store(true, Ordering::SeqCst);
	live.available.store(false, Ordering::SeqCst);
	assert!(read.invoke(&run, input.clone(), "four").await.is_err());
	live.available.store(true, Ordering::SeqCst);
	let mut other = run.clone();
	other.id = uuid::Uuid::new_v4();
	assert!(read.invoke(&other, input, "five").await.is_err());
	assert_eq!(live.invocations.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn activation_cannot_add_or_replace_a_binding_graph_after_execution_starts() {
	let mut run = admitted_run().await;
	let saved = (**run.context.binding_snapshot.as_ref().unwrap()).clone();
	run.bind(saved.clone()).unwrap();
	let mut changed = saved.clone();
	changed.bindings[0].provider_implementation = Some("replacement".into());
	assert!(run.bind(changed).is_err());
	run.context.binding_snapshot = None;
	run.step = 1;
	assert!(run.bind(saved).is_err());
	let resolver = execution::PinnedResolver {
		providers: Arc::new(Live::new()),
		authority: Arc::new(Live::new()),
	};
	assert!(resolver.tools(&run).await.is_err());
}

#[tokio::test]
async fn another_nodes_valid_snapshot_cannot_be_admitted_or_dispatch_for_the_same_agent() {
	let foreign = "aidash://node-b";
	let mut catalog = Catalog::new();
	catalog.entries = catalog
		.entries
		.into_iter()
		.map(|(mut identity, mut entry)| {
			identity.registry_node = foreign.into();
			if entry.kind == "tool" {
				let descriptor: ToolDescriptor = serde_json::from_value(entry.config).unwrap();
				entry.config =
					serde_json::to_value(core_descriptor(foreign, &descriptor.operation).unwrap())
						.unwrap();
			}
			(identity, entry)
		})
		.collect();
	let config = agent_config();
	let saved = resolve(
		&mut catalog,
		&providers(),
		QualifiedRef {
			registry_node: foreign.into(),
			..reference("agent")
		},
		&agent_entry(&config),
		false,
	)
	.await
	.unwrap();
	saved.validate().unwrap();
	let mut run = admitted_run().await;
	run.context.binding_snapshot = None;
	assert!(matches!(
		run.bind(saved.clone()),
		Err(aidash_domain::Error::Invalid(_))
	));
	assert!(run.context.binding_snapshot.is_none());

	// Persisted or adapter-supplied context must pass the same qualified identity check.
	run.context.binding_snapshot = Some(Box::new(saved));
	let live = Arc::new(Live::new());
	let resolver = execution::PinnedResolver {
		providers: live.clone(),
		authority: live.clone(),
	};
	assert!(matches!(
		resolver.tools(&run).await,
		Err(Error::Conflict(_))
	));
	assert_eq!(live.invocations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn recovered_snapshots_must_match_the_agent_binding_closure_before_run_admission() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	config.remove_default.push("memory_mutate".into());
	let mut get = Binding::tool(catalog.core("outbound_get"));
	get.alias = Some("lookup".into());
	get.narrow.allowed_hosts = Some(BTreeSet::from(["example.com".into()]));
	config.bindings.push(get);
	for (id, kind, source) in [
		("skill", BindingKind::Skill, json!({"instructions":"Work"})),
		(
			"skills-root",
			BindingKind::Source,
			json!({"schema_version":1,"source":{"adapter":"skill_roots","roots":[".agents/skills"]}}),
		),
	] {
		catalog.insert(entry(
			id,
			if kind == BindingKind::Skill {
				"skill"
			} else {
				"source"
			},
			source,
		));
		config.bindings.push(Binding {
			kind,
			target: reference(id),
			alias: None,
			narrow: Default::default(),
			members: vec![],
			exposure: None,
		});
	}
	let code = catalog.core("code_interpreter");
	let install = catalog.core("python_install");
	catalog.core("python_poll");
	catalog.core("python_cancel");
	let bundle = catalog.bundle("python", vec![code.clone(), install.clone()]);
	config.bindings.push(Binding {
		kind: BindingKind::Bundle,
		target: bundle,
		alias: None,
		narrow: Default::default(),
		members: vec![code.id.clone()],
		exposure: None,
	});
	let saved = snapshot(&mut catalog, &config, false).await.unwrap();
	let recovered: BindingSnapshot =
		serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap();
	recovered.validate().unwrap();

	let mut altered = vec![];
	for id in [
		"aidash.outbound_get",
		"skill",
		"skills-root",
		"aidash.code_interpreter",
		"aidash.python_poll",
		"aidash.file_read",
	] {
		let mut missing = recovered.clone();
		missing.bindings.retain(|binding| binding.identity.id != id);
		altered.push(missing);
	}
	for field in ["alias", "origin", "narrow"] {
		let mut changed = recovered.clone();
		let get = changed
			.bindings
			.iter_mut()
			.find(|binding| binding.identity.id == "aidash.outbound_get")
			.unwrap();
		match field {
			"alias" => get.alias = Some("different".into()),
			"origin" => get.origin = BindingOrigin::Default,
			_ => get.narrow = Default::default(),
		}
		altered.push(changed);
	}
	let mut duplicate = recovered.clone();
	duplicate.bindings.push(duplicate.bindings[0].clone());
	altered.push(duplicate);
	let mut expanded_config = config.clone();
	expanded_config.bindings.last_mut().unwrap().members.clear();
	expanded_config
		.bindings
		.push(Binding::tool(catalog.core("apply_patch")));
	let expanded = snapshot(&mut catalog, &expanded_config, false)
		.await
		.unwrap();
	for identity in [install, QualifiedRef::builtin(NODE, "apply_patch")] {
		let mut added = recovered.clone();
		added.bindings.push(
			expanded
				.bindings
				.iter()
				.find(|b| b.identity == identity)
				.unwrap()
				.clone(),
		);
		if !added.definitions.iter().any(|d| d.identity == identity) {
			added.definitions.push(
				expanded
					.definitions
					.iter()
					.find(|d| d.identity == identity)
					.unwrap()
					.clone(),
			);
		}
		altered.push(added);
	}
	let mut run = admitted_run().await;
	run.context.binding_snapshot = None;
	for changed in altered {
		assert!(changed.validate().is_err());
		assert!(run.bind(changed).is_err());
		assert!(run.context.binding_snapshot.is_none());
	}
	run.bind(recovered).unwrap();
}

#[tokio::test]
async fn bundle_selection_rejects_same_id_on_different_nodes_or_versions() {
	for (node, version) in [(NODE, "2.0.0"), ("aidash://node-b", "1.0.0")] {
		let mut catalog = Catalog::new();
		let member = catalog.core("outbound_get");
		let mut other = member.clone();
		other.registry_node = node.into();
		other.version = version.into();
		let bundle = catalog.bundle("ambiguous", vec![member.clone(), other]);
		let mut config = agent_config();
		config.bindings.push(Binding {
			kind: BindingKind::Bundle,
			target: bundle,
			alias: None,
			narrow: Default::default(),
			members: vec![member.id],
			exposure: None,
		});
		assert!(snapshot(&mut catalog, &config, false).await.is_err());
	}
}

#[tokio::test]
async fn remote_registry_skill_reader_does_not_require_a_native_working_area() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	catalog.insert(entry(
		"memory",
		"memory",
		json!({"schema_version":1,"source":{"adapter":"semantic_memory"}}),
	));
	config.bindings.push(Binding {
		kind: BindingKind::Memory,
		target: reference("memory"),
		alias: None,
		narrow: Default::default(),
		members: vec![],
		exposure: None,
	});
	let admitted = snapshot(&mut catalog, &config, true).await.unwrap();
	assert!(admitted.operation("skill_read").is_ok());
	let settings = aidash_domain::registry::AgentConfig::from_snapshot(&admitted).unwrap();
	assert!(settings.semantic_memory);
	assert!(!settings.needs_context_authority());
	assert!(!settings.core_capabilities.files);
	assert!(!settings.core_capabilities.skills);
}

async fn public_foreign_snapshot() -> ForeignAgentSnapshot {
	let node = "aidash://node-b";
	let mut catalog = Catalog::new();
	catalog.entries = catalog
		.entries
		.into_iter()
		.map(|(mut identity, mut entry)| {
			identity.registry_node = node.into();
			if entry.kind == "tool" {
				let descriptor: ToolDescriptor = serde_json::from_value(entry.config).unwrap();
				entry.config =
					serde_json::to_value(core_descriptor(node, &descriptor.operation).unwrap())
						.unwrap();
			}
			(identity, entry)
		})
		.collect();
	let saved = resolve(
		&mut catalog,
		&providers(),
		QualifiedRef {
			registry_node: node.into(),
			..reference("agent")
		},
		&agent_entry(&agent_config()),
		true,
	)
	.await
	.unwrap();
	ForeignAgentSnapshot::from_snapshot(saved).unwrap()
}

#[tokio::test]
async fn foreign_agent_transports_pin_receiver_closures_without_flattening_child_tools() {
	let child = public_foreign_snapshot().await;
	let mut catalog = Catalog::new();
	catalog.foreign.insert(child.agent.clone(), child.clone());
	let descriptor = ToolDescriptor {
		registry_node: NODE.into(),
		provider: "integration.agent@1".into(),
		operation: "invoke".into(),
		default_alias: "remote_child".into(),
		tier: ToolTier::Integration,
		narrow: Default::default(),
		lifecycle: None,
		transport: Some(aidash_domain::tool::ToolConfig::Agent {
			node_id: child.agent.registry_node.clone(),
			agent: child.agent.local(),
		}),
	};
	catalog.insert(entry(
		"foreign-transport",
		"tool",
		serde_json::to_value(descriptor).unwrap(),
	));
	let mut config = agent_config();
	config
		.bindings
		.push(Binding::tool(reference("foreign-transport")));
	let graph = snapshot(&mut catalog, &config, false).await.unwrap();
	assert_eq!(
		graph.foreign_agents.as_slice(),
		std::slice::from_ref(&child)
	);
	assert!(
		graph
			.definitions
			.iter()
			.any(|definition| definition.identity == child.agent)
	);
	assert!(
		graph
			.bindings
			.iter()
			.all(|binding| binding.identity.registry_node == NODE)
	);
	assert_eq!(
		graph
			.bindings
			.iter()
			.filter(|binding| binding.alias.as_deref() == Some("remote_child"))
			.count(),
		1
	);
	let mut recovered: BindingSnapshot =
		serde_json::from_slice(&serde_json::to_vec(&graph).unwrap()).unwrap();
	recovered.validate().unwrap();
	recovered.foreign_agents.clear();
	assert!(recovered.validate().is_err());
	let mut forged = graph.clone();
	forged.foreign_agents[0].agent.registry_node = NODE.into();
	assert!(forged.validate().is_err());
	let mut forged = graph.clone();
	forged.foreign_agents[0].definitions[0].digest = "forged".into();
	assert!(forged.validate().is_err());
	let mut duplicated = graph;
	duplicated.foreign_agents.push(child.clone());
	assert!(duplicated.validate().is_err());
	catalog.foreign.clear();
	assert!(matches!(
		snapshot(&mut catalog, &config, false).await,
		Err(Error::Forbidden)
	));
}

#[tokio::test]
async fn foreign_export_rejects_private_installed_and_cross_node_definitions() {
	let saved = public_foreign_snapshot().await;
	for kind in ["memory", "source", "skill"] {
		let mut changed = saved.clone();
		changed.definitions.push(
			ResolvedDefinition::new(
				QualifiedRef {
					registry_node: saved.agent.registry_node.clone(),
					..reference("private")
				},
				entry("private", kind, json!({})),
			)
			.unwrap(),
		);
		assert!(changed.validate().is_err(), "{kind}");
	}
	let mut installed = saved.clone();
	installed.definitions[0].definition.installation = Some(aidash_domain::registry::Projection {
		contract: 1,
		tenant: "private".into(),
		installation: "installed".into(),
		revision: 1,
	});
	assert!(installed.validate().is_err());
	let mut changed = saved.clone();
	changed.definitions[0].identity.registry_node = NODE.into();
	assert!(changed.validate().is_err());
	let mut local = saved.snapshot();
	local.remote = false;
	assert!(ForeignAgentSnapshot::from_snapshot(local).is_err());
	// Additional pinned roots cannot turn this protocol into a multi-hop export.
	let mut recursive = saved.snapshot();
	recursive.foreign_agents.push(saved);
	assert!(ForeignAgentSnapshot::from_snapshot(recursive).is_err());
}

#[tokio::test]
async fn standalone_host_operations_derive_only_their_resource_authority() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	config.remove_default.push("task_delegate".into());
	config
		.bindings
		.push(Binding::tool(catalog.core("outbound_get")));
	config
		.bindings
		.push(Binding::tool(catalog.core("task_assign")));
	let admitted = snapshot(&mut catalog, &config, false).await.unwrap();
	let settings = aidash_domain::registry::AgentConfig::from_snapshot(&admitted).unwrap();
	assert!(aidash_domain::tool::CorePermission::Outbound.permitted(&settings.core_capabilities));
	assert!(aidash_domain::tool::AgentFlag::Delegation.permitted(&settings));
	assert!(!settings.core_capabilities.shell);
	assert!(!settings.core_capabilities.python);
	assert!(admitted.operation("shell").is_err());
	assert!(admitted.operation("task_delegate").is_err());
}

#[tokio::test]
async fn aggregate_reference_mounts_require_files_and_unique_identities() {
	let mut catalog = Catalog::new();
	let mut config = agent_config();
	let reference_id = uuid::Uuid::new_v4();
	for name in ["first", "second"] {
		catalog.insert(entry(name, "source", json!({"schema_version":1,"source":{"adapter":"reference_attachments","references":[{"reference_id":reference_id,"digest":"a".repeat(64)}]}})));
		config.bindings.push(Binding {
			kind: BindingKind::Source,
			target: reference(name),
			alias: None,
			narrow: Default::default(),
			members: vec![],
			exposure: None,
		});
	}
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
	config.bindings.pop();
	assert!(snapshot(&mut catalog, &config, false).await.is_ok());
	config
		.remove_default
		.extend(["file_search".into(), "file_read".into()]);
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
	config.bindings.clear();
	assert!(snapshot(&mut catalog, &config, false).await.is_ok());
}

#[test]
fn native_memory_binding_preserves_removed_read_operations() {
	let mut root = crate::test_support::agent("native-read-disabled");
	root.config["bindings"] = json!([crate::test_support::binding("memory", NODE, "native")]);
	root.config["remove_default"] = json!(["memory_recall", "memory_reflect"]);
	let admitted = crate::test_support::resolve(
		NODE,
		&root,
		true,
		crate::test_support::native_memory_entries(),
	);
	let settings = aidash_domain::registry::AgentConfig::from_snapshot(&admitted).unwrap();
	assert_eq!(settings.memory.unwrap().id, "native");
	assert_eq!(settings.allow_cross_conversation_memory, Some(false));
	assert!(admitted.operation("memory_recall").is_err());
	assert!(admitted.operation("memory_reflect").is_err());
}

fn decider_config() -> Value {
	json!({"hook":"compaction","answer_type":"noul","description":"Preserve needed history",
        "provider_contract":"typesafe.jev/1","endpoint":"https://example.test/systemone","model":"jev-1.13.0",
        "credential_env":"AIDASH_SECRET_JEV","builder":"aidash.compaction/1","option_source":"history_event/1",
        "rule":"compaction.keep/1","keep_threshold":"3fe0000000000000","mode":"enforce"})
}
fn decider_binding(id: &str) -> Binding {
	serde_json::from_value(json!({"kind":"decider","target":reference(id),"narrow":{"decision":{"keep_threshold":"3fd0000000000000","preserve_recent":8}}})).unwrap()
}
#[tokio::test]
async fn explicit_decider_snapshot_pins_provider_builder_model_and_narrowing() {
	let mut catalog = Catalog::new();
	catalog.insert(entry("decider", "decider", decider_config()));
	let mut config = agent_config();
	config.bindings.push(decider_binding("decider"));
	let saved = snapshot(&mut catalog, &config, false).await.unwrap();
	saved.validate().unwrap();
	let implementation = saved
		.bindings
		.iter()
		.find(|b| b.definition.kind == "decider")
		.unwrap()
		.provider_implementation
		.as_deref()
		.unwrap();
	assert_eq!(implementation, "node-a/decision-adapter-v2");
	assert_ne!(implementation, aidash_domain::decision::PROVIDER);
	let recovered: BindingSnapshot =
		serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap();
	recovered.validate().unwrap();
	assert_eq!(recovered, saved);
	for implementation in [None, Some(""), Some(" \t ")] {
		let mut corrupt = saved.clone();
		corrupt
			.bindings
			.iter_mut()
			.find(|b| b.definition.kind == "decider")
			.unwrap()
			.provider_implementation = implementation.map(str::to_owned);
		assert!(corrupt.validate().is_err());
	}
	let bound = recovered
		.decider(aidash_domain::decision::Hook::Compaction)
		.unwrap();
	assert_eq!(bound.pin.identity, reference("decider"));
	assert_eq!(
		serde_json::to_value(&bound.pin).unwrap()["provider_implementation"],
		implementation
	);
	assert_eq!(bound.config.model, "jev-1.13.0");
	assert_eq!(bound.restrictions.preserve_recent, 8);
	assert_eq!(bound.restrictions.keep_threshold.value(), 0.25);
	catalog
		.entries
		.get_mut(&reference("decider"))
		.unwrap()
		.config["model"] = json!("jev-1.14.0");
	assert_eq!(
		saved
			.decider(aidash_domain::decision::Hook::Compaction)
			.unwrap()
			.config
			.model,
		"jev-1.13.0"
	);
	let mut corrupt = saved.clone();
	corrupt
		.bindings
		.iter_mut()
		.find(|b| b.definition.kind == "decider")
		.unwrap()
		.provider_contract_digest = Some("a".repeat(64));
	assert!(corrupt.validate().is_err());
	corrupt = saved;
	corrupt
		.bindings
		.iter_mut()
		.find(|b| b.definition.kind == "decider")
		.unwrap()
		.narrow
		.decision
		.as_mut()
		.unwrap()
		.preserve_recent = 6;
	assert!(corrupt.validate().is_err());
}
#[tokio::test]
async fn duplicate_decider_hooks_and_unavailable_node_implementations_fail_admission() {
	let mut catalog = Catalog::new();
	catalog.insert(entry("decider", "decider", decider_config()));
	catalog.insert(entry("other", "decider", decider_config()));
	let mut config = agent_config();
	config.bindings.push(decider_binding("decider"));
	config.bindings.push(decider_binding("other"));
	assert!(snapshot(&mut catalog, &config, false).await.is_err());
	config.bindings.pop();
	let unavailable = Providers {
		unavailable: Some("decision".into()),
	};
	assert!(
		resolve(
			&mut catalog,
			&unavailable,
			reference("agent"),
			&agent_entry(&config),
			false
		)
		.await
		.is_err()
	);
	let saved = snapshot(&mut catalog, &config, true).await.unwrap();
	assert_eq!(
		saved
			.decider(aidash_domain::decision::Hook::Compaction)
			.unwrap()
			.pin
			.identity
			.registry_node,
		NODE
	);
}
#[test]
fn decider_bindings_reject_deletion_widening_and_tool_only_fields() {
	for narrow in [
		json!({"decision":{"keep_threshold":"3fe3333333333333"}}),
		json!({"decision":{"preserve_recent":5}}),
		json!({"allowed_hosts":["example.test"]}),
		json!({"limits":{"max_calls":10}}),
	] {
		let binding: Binding = serde_json::from_value(
			json!({"kind":"decider","target":reference("decider"),"narrow":narrow}),
		)
		.unwrap();
		assert!(binding.validate().is_err());
	}
	let mut tool = Binding::tool(reference("tool"));
	tool.narrow.decision = Some(Default::default());
	assert!(tool.validate().is_err());
}
