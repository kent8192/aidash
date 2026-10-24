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
fn only_a_descriptors_read_only_declaration_lifts_the_tool_call_approval() {
	for (replay, effect, approval) in [
		("read_only", ToolEffect::ReadOnly, false),
		("idempotent", ToolEffect::Unsafe, true),
		("unsafe", ToolEffect::Unsafe, true),
	] {
		let descriptor = ToolDescriptor {
			registry_deployment: "aidash://node-a".into(),
			provider: "integration.http@1".into(),
			operation: "invoke".into(),
			default_alias: "lookup".into(),
			tier: ToolTier::Integration,
			narrow: Narrowing::default(),
			lifecycle: None,
			transport: Some(ToolConfig::Http {
				endpoint: "https://example.invalid".into(),
				replay: replay.into(),
				secret: None,
			}),
		};
		let contract = descriptor
			.declared_contract(QualifiedRef {
				registry_deployment: "aidash://node-a".into(),
				id: "lookup".into(),
				version: "1.0.0".into(),
			})
			.unwrap();
		assert_eq!(contract.behavior.effect, effect, "{replay}");
		assert_eq!(contract.behavior.tool_call_approval, approval, "{replay}");
		assert_eq!(contract.replay_safe(), !approval, "{replay}");
	}
}
#[test]
fn descriptors_can_lower_but_never_raise_provider_concurrency() {
	use crate::tool::Concurrency;
	let declared = |operation: &str, claim| {
		let mut descriptor = core_descriptor("aidash://node-a", operation).unwrap();
		descriptor.narrow.concurrency = claim;
		descriptor.declared_contract(QualifiedRef::builtin("aidash://node-a", operation))
	};
	assert_eq!(
		declared("file_read", Some(Concurrency::SharedRead))
			.unwrap()
			.behavior
			.concurrency,
		Concurrency::SharedRead
	);
	assert!(declared("file_read", Some(Concurrency::Sequential)).is_ok());
	for operation in ["apply_patch", "skill_read", "shell", "outbound_get"] {
		assert!(declared(operation, Some(Concurrency::Sequential)).is_ok());
		assert!(declared(operation, Some(Concurrency::SharedRead)).is_err());
	}
	let mut http = serde_json::to_value(ToolDescriptor {
		registry_deployment: "aidash://node-a".into(),
		provider: "integration.http@1".into(),
		operation: "invoke".into(),
		default_alias: "lookup".into(),
		tier: ToolTier::Integration,
		narrow: Narrowing::default(),
		lifecycle: None,
		transport: Some(ToolConfig::Http {
			endpoint: "https://example.invalid".into(),
			replay: "read_only".into(),
			secret: None,
		}),
	})
	.unwrap();
	http["narrow"]["concurrency"] = json!("shared_read");
	let http: ToolDescriptor = serde_json::from_value(http).unwrap();
	assert!(
		http.declared_contract(QualifiedRef {
			registry_deployment: "aidash://node-a".into(),
			id: "lookup".into(),
			version: "1.0.0".into(),
		})
		.is_err()
	);
}
#[test]
fn only_implicit_defaults_can_be_excluded_remotely() {
	let descriptor = core_descriptor("aidash://node-a", "memory_mutate").unwrap();
	let contract = descriptor
		.declared_contract(QualifiedRef::builtin("aidash://node-a", "memory_mutate"))
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
fn stored_tool_transports_that_name_a_deployment_secret_no_longer_decode() {
	// Dispatch decodes the stored descriptor first, so such a Tool is refused
	// before anything could resolve the secret (#296).
	for transport in [
		json!({"transport":"http","endpoint":"https://example.invalid","credential_env":"AIDASH_SECRET_BRAVE_SEARCH","replay":"read_only"}),
		json!({"transport":"mcp","endpoint":"https://example.invalid","credential_env":null,"tool_name":"lookup","replay":"read_only"}),
	] {
		assert!(serde_json::from_value::<ToolConfig>(transport.clone()).is_err());
		let descriptor = json!({"registry_deployment":"aidash://node-a","provider":"integration.http@1","operation":"invoke","default_alias":"lookup","tier":"integration","transport":transport});
		assert!(serde_json::from_value::<ToolDescriptor>(descriptor).is_err());
	}
}

#[test]
fn legacy_transport_tags_do_not_accept_descriptor_fields_or_convert_descriptors() {
	let legacy =
		json!({"transport":"http","endpoint":"https://example.invalid","replay":"read_only"});
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

#[test]
fn exposure_and_skill_asset_operations_are_builtin_core_operations() {
	for (operation, provider) in [
		("capability_search", "core.exposure@1"),
		("capability_describe", "core.exposure@1"),
		("capability_load", "core.exposure@1"),
		("capability_unload", "core.exposure@1"),
		("skill_asset_read", "core.skills@1"),
	] {
		assert_eq!(core_provider(operation), Some(provider));
		assert_eq!(tier(operation), Some(ToolTier::Builtin));
		assert!(reserved_aliases().contains(operation));
		let descriptor = core_descriptor("aidash://node-a", operation).unwrap();
		assert!(descriptor.lifecycle.is_none());
		let contract = descriptor
			.declared_contract(QualifiedRef::builtin("aidash://node-a", operation))
			.unwrap();
		assert!(contract.remote_exposure);
		assert!(
			remote_exclusion(BindingOrigin::Required, &contract)
				.unwrap()
				.is_none()
		);
		assert_eq!(
			contract.behavior.exposure_update,
			matches!(operation, "capability_load" | "capability_unload")
		);
	}
}
