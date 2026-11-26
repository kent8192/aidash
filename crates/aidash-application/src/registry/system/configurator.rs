//! Release-pinned declarations distributed to every Tenant.
use aidash_domain::registry::{Entry, bindings::QualifiedRef};
use serde_json::json;

pub use aidash_domain::identity::tenant_administration::CONFIGURATOR_ID as ID;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GUIDE: &str = "aidash.tenant-configuration-guide";

pub fn definitions(node: &str) -> crate::Result<[Entry; 2]> {
	let guide = json!({"id":GUIDE,"kind":"skill","version":VERSION,
		"name":{"en":"Tenant configuration guide"},"description":{"en":"Configure a Tenant for its administrator."},
		"tags":["system"],"languages":[],"capabilities":[],"schema":{},
		"config":{"instructions":include_str!("configuration-guide.md")}});
	let mut bindings = vec![
		json!({"kind":"skill","target":{"registry_deployment":node,"id":GUIDE,"version":VERSION}}),
	];
	for &operation in ["node_create", "setting_request"]
		.iter()
		.chain(aidash_domain::tool::configurator::TOOLS)
	{
		bindings.push(
			json!({"kind":"tool","target":QualifiedRef::builtin(node,operation),"alias":operation}),
		);
	}
	let agent = json!({"id":ID,"kind":"agent","version":VERSION,
		"name":{"en":"Tenant Configurator","ja":"テナント設定エージェント"},
		"description":{"en":"Configure this Tenant at its administrator's request."},
		"tags":["system"],"languages":[],"capabilities":[],"schema":{},
		// #1154 supplies the Task Model when this definition is admitted.
		"config":{"schema_version":1,"instructions":"Follow the Tenant configuration guide.",
			"bindings":bindings}});
	Ok([
		serde_json::from_value(guide)?,
		serde_json::from_value(agent)?,
	])
}

/// Install this release's subject when a Node first receives it. A policy
/// administrator's later narrowing of an existing subject survives restarts.
pub fn install_subject(
	bundle: &mut aidash_domain::policy::PolicyBundle,
	node: &aidash_domain::nodes::NodeAddress,
) -> bool {
	let id = aidash_domain::qualified_agent(node, ID, VERSION);
	if bundle.subjects.contains_key(&id) {
		return false;
	}
	let groups = bundle
		.groups
		.contains_key(aidash_domain::identity::tenancy::ADMINISTRATORS)
		.then(|| aidash_domain::identity::tenancy::ADMINISTRATORS.to_owned())
		.into_iter()
		.collect();
	bundle.subjects.insert(
		id,
		aidash_domain::policy::Subject {
			kind: aidash_domain::policy::SubjectKind::Agent,
			node: Some(node.node.clone()),
			groups,
			roles: Default::default(),
			enabled: true,
			delegated_by: None,
			attributes: json!({}),
		},
	);
	true
}
