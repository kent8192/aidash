use super::*;
use crate::{registry::bindings::BindingOrigin, tool::ToolEffect};
use serde_json::json;

#[test]
fn provider_versions_lifecycle_and_facts_cannot_be_relabelled() {
	let mut descriptor = core_descriptor("aidash://node-a", "shell").unwrap();
	let identity = QualifiedRef::builtin("aidash://node-a", "shell");
	let contract = descriptor.declared_contract(identity.clone()).unwrap();
	descriptor.provider = "core.sandbox@2".into();
	assert!(descriptor.declared_contract(identity.clone()).is_err());
	descriptor.provider = "core.sandbox@1".into();
	descriptor.lifecycle.as_mut().unwrap().poll =
		QualifiedRef::builtin("aidash://node-a", "python_poll");
	// Companion IDs change at installation. Their operation is checked when
	// the complete closure is resolved, rather than inferred from an ID.
	assert!(descriptor.declared_contract(identity).is_ok());
	assert!(contract.authorization.core.is_none());
	let mut value =
		serde_json::to_value(core_descriptor("aidash://node-a", "shell").unwrap()).unwrap();
	value["effect"] = json!("read_only");
	assert!(serde_json::from_value::<ToolDescriptor>(value).is_err());
}
#[test]
fn http_and_mcp_publisher_claims_do_not_manufacture_replay_guarantees() {
	for replay in ["read_only", "idempotent", "unsafe"] {
		let descriptor = ToolDescriptor {
			registry_node: "aidash://node-a".into(),
			provider: "integration.http@1".into(),
			operation: "invoke".into(),
			default_alias: "lookup".into(),
			tier: ToolTier::Integration,
			narrow: Narrowing::default(),
			lifecycle: None,
			transport: Some(ToolConfig::Http {
				endpoint: "https://example.invalid".into(),
				credential_env: None,
				replay: replay.into(),
			}),
		};
		let contract = descriptor
			.declared_contract(QualifiedRef {
				registry_node: "aidash://node-a".into(),
				id: "lookup".into(),
				version: "1.0.0".into(),
			})
			.unwrap();
		assert_eq!(contract.behavior.effect, ToolEffect::Unsafe);
		assert!(contract.behavior.workbench_approval);
		assert!(!contract.replay_safe());
	}
}
#[test]
fn only_implicit_defaults_can_be_excluded_remotely() {
	let descriptor = core_descriptor("aidash://node-a", "memory_write").unwrap();
	let contract = descriptor
		.declared_contract(QualifiedRef::builtin("aidash://node-a", "memory_write"))
		.unwrap();
	assert!(
		remote_exclusion(BindingOrigin::Default, &contract)
			.unwrap()
			.is_some()
	);
	for origin in [
		BindingOrigin::Required,
		BindingOrigin::Explicit,
		BindingOrigin::SkillSupport,
		BindingOrigin::Companion,
	] {
		assert!(remote_exclusion(origin, &contract).is_err());
	}
	let human = core_descriptor("aidash://node-a", "human_request")
		.unwrap()
		.declared_contract(QualifiedRef::builtin("aidash://node-a", "human_request"))
		.unwrap();
	assert!(
		remote_exclusion(BindingOrigin::Required, &human)
			.unwrap()
			.is_none()
	);
}

#[test]
fn legacy_transport_tags_do_not_accept_descriptor_fields_or_convert_descriptors() {
	let legacy = json!({"transport":"http","endpoint":"https://example.invalid","credential_env":null,"replay":"read_only"});
	assert!(matches!(
		crate::tool::legacy_config(&legacy).unwrap(),
		Some(ToolConfig::Http { .. })
	));
	let mut mixed = legacy;
	mixed["provider"] = json!("core.workspace@1");
	assert!(crate::tool::legacy_config(&mixed).is_err());
	let descriptor =
		serde_json::to_value(core_descriptor("aidash://node-a", "workspace_read").unwrap())
			.unwrap();
	assert!(crate::tool::legacy_config(&descriptor).unwrap().is_none());
}
