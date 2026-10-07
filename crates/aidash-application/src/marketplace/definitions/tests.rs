use super::*;
use aidash_domain::marketplace::{Installation, Revision};
use aidash_domain::registry::Projection;
use async_trait::async_trait;
use rstest::rstest;
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;

fn r(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
}
fn entry(id: &str, kind: &str, config: Value) -> Entry {
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Fixture"},"config":config})).unwrap()
}
fn agent(id: &str) -> Entry {
	entry(
		id,
		"agent",
		json!({"schema_version":1,"model":r("model"),"instructions":"Fixture","bindings":[crate::test_support::binding("tool","aidash://local","tool")],"remove_default":aidash_domain::registry::bindings::DEFAULT_TOOLS,"cluster":null}),
	)
}
#[derive(Default)]
struct Fixture {
	entries: BTreeMap<String, Entry>,
	calls: Vec<String>,
	denied: Option<String>,
	revision: Option<Revision>,
}
impl Fixture {
	fn record(&mut self, name: String) -> Result<()> {
		self.calls.push(name.clone());
		if self.denied.as_ref() == Some(&name) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
}
#[async_trait]
impl DefinitionScope for Fixture {
	fn tenant(&self) -> &str {
		"tenant"
	}
	async fn raw(&mut self, reference: &EntityRef) -> Result<Entry> {
		self.record(format!("raw:{}", reference.id))?;
		self.entries
			.get(&reference.id)
			.cloned()
			.ok_or(Error::Forbidden)
	}
	async fn installation(&mut self, id: &str) -> Result<Option<Installation>> {
		self.record(format!("installation:{id}"))?;
		Ok(self.revision.as_ref().map(|_| Installation {
			id: id.into(),
			tenant: "tenant".into(),
			package_key: "source".into(),
			latest_revision: 1,
			active_revision: None,
			activation_revision: 0,
		}))
	}
	async fn revision(&mut self, id: &str, _: i64) -> Result<Revision> {
		self.record(format!("revision:{id}"))?;
		self.revision.clone().ok_or(Error::Forbidden)
	}
	async fn require_installation_read(
		&mut self,
		installation: &Installation,
		_: i64,
	) -> Result<()> {
		self.record(format!("installation.read:{}", installation.id))
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		self.record(format!("{action}:{}", reference.id))?;
		self.entries
			.get(&reference.id)
			.cloned()
			.ok_or(Error::Forbidden)
	}
	async fn require_export(&mut self, entry: &Entry) -> Result<()> {
		self.record(format!("registry.export:{}", entry.id))
	}
	async fn matching_publication(
		&mut self,
		reference: &EntityRef,
		_: &str,
	) -> Result<Option<Version>> {
		self.record(format!("publication:{}", reference.id))?;
		Ok(None)
	}
	async fn require_reference_read(&mut self, id: Uuid) -> Result<()> {
		self.record(format!("reference.read:{id}"))
	}
}
fn graph() -> Fixture {
	Fixture {
		entries: BTreeMap::from([
			("agent".into(), agent("agent")),
			(
				"tool".into(),
				entry(
					"tool",
					"tool",
					json!({"registry_node":"aidash://local","provider":"integration.agent@1","operation":"invoke","default_alias":"delegate","tier":"integration","transport":{"transport":"agent","node_id":"aidash://local","agent":r("agent")}}),
				),
			),
			("model".into(), entry("model", "model", json!({}))),
			(
				"aidash.workspace_read".into(),
				crate::test_support::builtin_entries("aidash://local")
					.into_iter()
					.find(|e| e.id == "aidash.workspace_read")
					.unwrap(),
			),
			(
				"aidash.human_request".into(),
				crate::test_support::builtin_entries("aidash://local")
					.into_iter()
					.find(|e| e.id == "aidash.human_request")
					.unwrap(),
			),
		]),
		..Default::default()
	}
}
#[rstest]
#[tokio::test]
async fn cycle_traversal_rechecks_authority_but_returns_each_definition_once() {
	let mut scope = graph();
	let entries = local_graph(
		&mut scope,
		vec![(r("agent"), "agent".into())],
		"aidash://local",
	)
	.await
	.unwrap();
	assert_eq!(
		entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
		vec!["agent", "tool", "model"]
	);
	assert_eq!(
		scope.calls,
		vec![
			"raw:agent",
			"registry.read:agent",
			"raw:tool",
			"registry.read:tool",
			"raw:agent",
			"registry.read:agent",
			"raw:model",
			"registry.read:model"
		]
	);
}
#[rstest]
#[tokio::test]
async fn hidden_transitive_dependency_prevents_a_partial_graph() {
	let mut scope = graph();
	scope.denied = Some("registry.read:tool".into());
	assert!(matches!(
		local_graph(
			&mut scope,
			vec![(r("agent"), "agent".into())],
			"aidash://local"
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope.calls,
		vec![
			"raw:agent",
			"registry.read:agent",
			"raw:tool",
			"registry.read:tool"
		]
	);
}
#[rstest]
#[tokio::test]
async fn publication_checks_export_before_disclosing_dependency_content() {
	let mut scope = graph();
	scope.denied = Some("registry.export:tool".into());
	let root = entry("root", "cluster", json!({"coordinator":r("agent")}));
	assert!(matches!(
		publication_graph(&mut scope, &root, &[], "aidash://local").await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope.calls,
		vec![
			"registry.read:agent",
			"registry.export:agent",
			"publication:agent",
			"registry.read:tool",
			"registry.export:tool"
		]
	);
}
#[rstest]
#[tokio::test]
async fn foreign_tenant_projection_is_denied_before_installation_lookup() {
	let mut scope = graph();
	let entry = scope.entries.get_mut("agent").unwrap();
	entry.installation = Some(Projection {
		contract: 1,
		tenant: "foreign".into(),
		installation: "private".into(),
		revision: 1,
	});
	assert!(matches!(
		local(&mut scope, &r("agent")).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["raw:agent"]);
}
#[rstest]
fn remote_agent_tool_is_not_resolved_from_same_named_local_content() {
	let tool = entry(
		"remote-tool",
		"tool",
		json!({"registry_node":"aidash://remote","provider":"integration.agent@1","operation":"invoke","default_alias":"delegate","tier":"integration","transport":{"transport":"agent","node_id":"remote","agent":r("agent")}}),
	);
	assert!(matches!(refs(&tool, "local"), Err(Error::Forbidden)));
}

#[tokio::test]
async fn cross_node_bundle_installation_uses_verified_receiving_node_dependencies() {
	use aidash_domain::registry::Package;
	use sha2::{Digest, Sha256};
	let receiving = "aidash://receiver";
	let publishing = "aidash://publisher";
	let original = r("published-tool");
	let mut installed = entry(
		"installed-tool",
		"tool",
		json!({"transport":"http","endpoint":"https://example.com/tool","replay":"read_only"}),
	);
	installed.version = "4.0.0".into();
	let target = reference(&installed);
	let bundle = entry(
		"bundle",
		"bundle",
		json!({"members":[{"registry_node":publishing,"id":original.id,"version":original.version}]}),
	);
	let manifest_source = serde_json::to_string(&Package {
		entity: bundle,
		author: "fixture".into(),
		permissions: vec![],
		dependencies: vec![original.clone()],
	})
	.unwrap();
	let source = Version {
		key: "bundle-package".into(),
		repository: publishing.into(),
		owner_tenant: "publisher".into(),
		package_id: "bundle".into(),
		version: "1.0.0".into(),
		kind: "bundle".into(),
		publisher: "fixture".into(),
		source: r("bundle"),
		digest: format!("sha256:{:x}", Sha256::digest(manifest_source.as_bytes())),
		manifest_source,
		dependencies: vec![Dependency {
			reference: original.clone(),
			kind: "tool".into(),
			digest: content(&installed),
			package: None,
		}],
		lineage: BTreeSet::new(),
	};
	let submitted = vec![DependencyBinding {
		source: original,
		target: target.clone(),
	}];
	let mut scope = Fixture {
		entries: BTreeMap::from([(installed.id.clone(), installed)]),
		..Default::default()
	};
	let validation = crate::marketplace::installations::tests::validation();
	let (resolved, dependencies, bindings) = resolve(
		&mut scope,
		&validation,
		&source,
		&json!({}),
		&submitted,
		receiving,
	)
	.await
	.unwrap();
	assert_eq!(
		resolved.config["members"],
		json!([{"registry_node":receiving,"id":target.id,"version":target.version}])
	);
	assert_eq!(dependencies, vec![target]);
	assert_eq!(
		serde_json::to_value(bindings).unwrap(),
		serde_json::to_value(&submitted).unwrap()
	);
	assert!(
		scope
			.calls
			.iter()
			.any(|call| call == "registry.read:installed-tool")
	);
	assert_eq!(
		manifest(&source).unwrap().entity.config["members"][0]["registry_node"],
		publishing
	);

	scope.denied = Some("registry.read:installed-tool".into());
	assert!(matches!(
		resolve(
			&mut scope,
			&validation,
			&source,
			&json!({}),
			&submitted,
			receiving
		)
		.await,
		Err(Error::Forbidden)
	));
	scope.denied = None;
	let mut changed = source.clone();
	changed.dependencies[0].digest = "changed".into();
	assert!(matches!(
		resolve(
			&mut scope,
			&validation,
			&changed,
			&json!({}),
			&submitted,
			receiving
		)
		.await,
		Err(Error::Forbidden)
	));
	let mut undeclared = source;
	undeclared.dependencies.clear();
	assert!(matches!(
		resolve(
			&mut scope,
			&validation,
			&undeclared,
			&json!({}),
			&submitted,
			receiving
		)
		.await,
		Err(Error::Forbidden)
	));
}
