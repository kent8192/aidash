use super::*;
use serde_json::json;

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
			.filter(|b| b.origin == BindingOrigin::Required)
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
			registry_node: "aidash://node-a".into(),
			id: "native.memory".into(),
			version: "1.0.0".into(),
		},
		alias: None,
		narrow: Narrowing::default(),
		members: vec![],
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
fn bound_skills_require_support_tools() {
	let mut agent = agent();
	let mut binding = Binding::tool(QualifiedRef {
		registry_node: "aidash://node-a".into(),
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
			registry_node: "aidash://node-b".into(),
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
