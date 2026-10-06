use super::*;
use crate::marketplace::*;
use rstest::rstest;
use serde_json::json;
use std::collections::BTreeSet;

fn entry() -> Entry {
	serde_json::from_value(json!({"id":"source","version":"1.0.0","kind":"tool","name":{"en":"Source"},"description":{"en":"Fixture"},"config":{"transport":"agent","node_id":"local","agent":{"id":"agent","version":"1.0.0"}}})).unwrap()
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
	local.config["node_id"] = json!("other");
	assert_ne!(content(&original), content(&local));
}
#[rstest]
fn executable_reference_rewrite_preserves_other_configuration() {
	let mut entity = entry();
	let original_node = entity.config["node_id"].clone();
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
	)
	.unwrap();
	assert_eq!(
		entity.config["agent"],
		json!({"id":"installed-agent","version":"4.0.0"})
	);
	assert_eq!(entity.config["node_id"], original_node);
}
