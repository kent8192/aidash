use super::*;
use crate::marketplace::*;
use rstest::rstest;
use serde_json::json;
use std::collections::BTreeSet;

fn entry() -> Entry {
	serde_json::from_value(json!({"id":"source","version":"1.0.0","kind":"tool","name":{"en":"Source"},"description":{"en":"Fixture"},"config":{"registry_node":"aidash://local","provider":"integration.agent@1","operation":"invoke","default_alias":"delegate","tier":"integration","transport":{"transport":"agent","node_id":"aidash://local","agent":{"id":"agent","version":"1.0.0"}}}})).unwrap()
}
fn version() -> Version {
	let package = Package {
		entity: entry(),
		author: "fixture".into(),
		permissions: vec![],
		dependencies: vec![],
	};
	let manifest_source = serde_json::to_string(&package).unwrap();
	Version {
		key: "key".into(),
		repository: "local".into(),
		owner_tenant: "tenant".into(),
		package_id: "source".into(),
		version: "1.0.0".into(),
		kind: "tool".into(),
		publisher: "fixture".into(),
		source: reference(&package.entity),
		digest: format!("sha256:{:x}", Sha256::digest(manifest_source.as_bytes())),
		manifest_source,
		dependencies: vec![],
		lineage: BTreeSet::new(),
	}
}
#[rstest]
fn frozen_manifest_rejects_content_tampering_and_kind_mismatch() {
	let valid = version();
	assert_eq!(manifest(&valid).unwrap().entity, entry());
	let mut tampered = valid.clone();
	tampered.manifest_source.push(' ');
	assert!(
		matches!(manifest(&tampered),Err(Error::Conflict(ref v)) if v=="revision or immutable content changed")
	);
	let mut wrong_kind = valid;
	wrong_kind.kind = "agent".into();
	assert!(matches!(manifest(&wrong_kind), Err(Error::Conflict(_))));
}
#[rstest]
fn content_identity_excludes_local_alias_and_installation_projection() {
	let original = entry();
	let mut local = original.clone();
	local.id = "alias".into();
	local.version = "2.0.0".into();
	local.installation = Some(crate::registry::Projection {
		contract: 1,
		tenant: "tenant".into(),
		installation: "installed".into(),
		revision: 1,
	});
	assert_eq!(content(&original), content(&local));
	local.config["transport"]["node_id"] = json!("other");
	assert_ne!(content(&original), content(&local));
}
#[rstest]
fn executable_reference_rewrite_preserves_other_configuration() {
	let mut entity = entry();
	let original_node = entity.config["transport"]["node_id"].clone();
	rewrite(
		&mut entity,
		&[DependencyBinding {
			source: EntityRef {
				id: "agent".into(),
				version: "1.0.0".into(),
			},
			target: EntityRef {
				id: "installed-agent".into(),
				version: "4.0.0".into(),
			},
		}],
		"aidash://local",
	)
	.unwrap();
	assert_eq!(
		entity.config["transport"]["agent"],
		json!({"id":"installed-agent","version":"4.0.0"})
	);
	assert_eq!(entity.config["transport"]["node_id"], original_node);
}

#[test]
fn bundle_substitutions_requalify_only_verified_members_to_the_receiving_node() {
	let mut bundle = entry();
	bundle.kind = "bundle".into();
	bundle.config = json!({"members":[
		{"registry_node":"aidash://publisher","id":"shell","version":"1.0.0"},
		{"registry_node":"aidash://publisher","id":"unbound","version":"1.0.0"}
	]});
	rewrite(
		&mut bundle,
		&[DependencyBinding {
			source: EntityRef {
				id: "shell".into(),
				version: "1.0.0".into(),
			},
			target: EntityRef {
				id: "installed-shell".into(),
				version: "2.0.0".into(),
			},
		}],
		"aidash://receiver",
	)
	.unwrap();
	assert_eq!(
		bundle.config["members"],
		json!([
			{"registry_node":"aidash://receiver","id":"installed-shell","version":"2.0.0"},
			{"registry_node":"aidash://publisher","id":"unbound","version":"1.0.0"}
		])
	);
}

#[rstest]
#[case("outbound_get")]
#[case("shell")]
fn installed_descriptors_and_bound_lifecycle_match_the_receiving_node(#[case] operation: &str) {
	use crate::{registry::bindings::QualifiedRef, tool::providers::*};
	let publishing = "aidash://publisher";
	let receiving = "aidash://receiver";
	let descriptor = core_descriptor(publishing, operation).unwrap();
	let bindings: Vec<_> = descriptor
		.lifecycle
		.as_ref()
		.map_or_else(Vec::new, |lifecycle| {
			[&lifecycle.poll, &lifecycle.cancel]
				.into_iter()
				.map(|reference| DependencyBinding {
					source: reference.local(),
					target: EntityRef {
						id: format!("mkt-{}", reference.id),
						version: reference.version.clone(),
					},
				})
				.collect()
		});
	let mut installed = entry();
	installed.id = format!("mkt-{operation}");
	installed.config = serde_json::to_value(descriptor).unwrap();
	rewrite(&mut installed, &bindings, receiving).unwrap();
	let rewritten: ToolDescriptor = serde_json::from_value(installed.config).unwrap();
	let identity = QualifiedRef {
		registry_node: receiving.into(),
		id: installed.id,
		version: installed.version,
	};
	assert!(rewritten.declared_contract(identity.clone()).is_ok());
	assert!(
		rewritten
			.declared_contract(QualifiedRef {
				registry_node: publishing.into(),
				..identity
			})
			.is_err()
	);
	if let Some(lifecycle) = rewritten.lifecycle {
		for (reference, binding) in [&lifecycle.poll, &lifecycle.cancel]
			.into_iter()
			.zip(bindings)
		{
			assert_eq!(reference.registry_node, receiving);
			assert_eq!(reference.local(), binding.target);
		}
	}
}
