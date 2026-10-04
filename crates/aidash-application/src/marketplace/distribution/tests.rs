use super::*;
use crate::ports::marketplace::DefinitionScope;
use aidash_domain::{
	policy::Resource,
	registry::{EntityRef, Entry, Package},
};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;

fn entry(id: &str) -> Entry {
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":"skill","name":{"en":id},"description":{"en":"Fixture"},"config":{}})).unwrap()
}
fn version(id: &str, owner: &str) -> Version {
	let entity = entry(id);
	let package = Package {
		entity: entity.clone(),
		author: "Fixture".into(),
		permissions: vec![],
		dependencies: vec![],
	};
	let manifest_source = serde_json::to_string(&package).unwrap();
	Version {
		key: id.into(),
		repository: "local".into(),
		owner_tenant: owner.into(),
		package_id: id.into(),
		version: "1.0.0".into(),
		kind: "skill".into(),
		publisher: "Fixture".into(),
		source: aidash_domain::marketplace::definitions::reference(&entity),
		digest: format!("sha256:{:x}", Sha256::digest(manifest_source.as_bytes())),
		manifest_source,
		dependencies: vec![],
		lineage: BTreeSet::new(),
	}
}
#[derive(Default)]
struct Fixture {
	versions: BTreeMap<String, Version>,
	audiences: BTreeMap<String, Audience>,
	consents: BTreeMap<String, Audience>,
	entries: BTreeMap<String, Entry>,
	revisions: Vec<(String, i64)>,
	decisions: Vec<(String, Resource)>,
	operations: Vec<String>,
	context: Value,
}
impl Fixture {
	fn new(root: Version) -> Self {
		Self {
			audiences: BTreeMap::from([(
				root.key.clone(),
				Audience {
					revision: 9,
					tenants: BTreeSet::from([
						"recipient".into(),
						"other".into(),
						root.owner_tenant.clone(),
					]),
				},
			)]),
			versions: BTreeMap::from([(root.key.clone(), root)]),
			context: json!({"scope":"root"}),
			..Default::default()
		}
	}
	fn resource(&self, kind: &str, id: &str) -> Resource {
		Resource {
			tenant: "recipient".into(),
			kind: kind.into(),
			id: id.into(),
			attributes: self.context.clone(),
		}
	}
}
#[async_trait]
impl DefinitionScope for Fixture {
	fn tenant(&self) -> &str {
		"recipient"
	}
	async fn raw(&mut self, r: &EntityRef) -> Result<Entry> {
		self.context = json!({"scope":"dependency"});
		self.entries.get(&r.id).cloned().ok_or(Error::Forbidden)
	}
	async fn installation(&mut self, _: &str) -> Result<Option<Installation>> {
		Ok(None)
	}
	async fn revision(&mut self, _: &str, _: i64) -> Result<Revision> {
		Err(Error::Forbidden)
	}
	async fn require_installation_read(&mut self, _: &Installation, _: i64) -> Result<()> {
		Err(Error::Forbidden)
	}
	async fn catalog(&mut self, r: &EntityRef, _: &str) -> Result<Entry> {
		self.entries.get(&r.id).cloned().ok_or(Error::Forbidden)
	}
	async fn require_export(&mut self, _: &Entry) -> Result<()> {
		Err(Error::Forbidden)
	}
	async fn matching_publication(&mut self, _: &EntityRef, _: &str) -> Result<Option<Version>> {
		Ok(None)
	}
	async fn require_reference_read(&mut self, _: Uuid) -> Result<()> {
		Err(Error::Forbidden)
	}
}
#[async_trait]
impl DistributionScope for Fixture {
	async fn version(&mut self, key: &str) -> Result<Option<Version>> {
		Ok(self.versions.get(key).cloned())
	}
	async fn audience(&mut self, key: &str) -> Result<Option<Audience>> {
		Ok(self.audiences.get(key).cloned())
	}
	async fn consent(&mut self, key: &str) -> Result<Option<Audience>> {
		Ok(self.consents.get(key).cloned())
	}
	fn authority(&mut self, name: String, revision: i64) {
		self.revisions.push((name, revision));
	}
	fn operation(&mut self, name: &str, _: Value, _: Option<Uuid>) {
		self.operations.push(name.into());
	}
	fn package_resource(&self, version: &Version) -> Resource {
		self.resource("package", &version.key)
	}
	fn installation_resource(&self, installation: &Installation, _: Option<i64>) -> Resource {
		self.resource("installation", &installation.id)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.decisions.push((action.into(), resource.clone()));
		Ok(())
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.decisions.push((action.into(), resource.clone()));
		Ok(true)
	}
}
#[rstest]
#[tokio::test]
async fn withdrawn_audience_blocks_an_otherwise_authorized_package_read() {
	// Arrange
	let root = version("root", "owner");
	let mut scope = Fixture::new(root);
	scope
		.audiences
		.get_mut("root")
		.unwrap()
		.tenants
		.remove("recipient");
	// Act
	let result = load(&mut scope, "root", "marketplace.read").await;
	// Assert: the live audience revision remains in the authority evidence.
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.operations, vec!["marketplace.read"]);
	assert_eq!(scope.decisions[0].0, "marketplace.read");
	assert_eq!(scope.revisions, vec![("audience:root".into(), 9)]);
}
#[rstest]
#[case::allowed(vec!["recipient"],true)]
#[case::outside_grant(vec!["recipient","foreign"],false)]
#[tokio::test]
async fn every_recipient_must_remain_inside_the_ancestor_consent(
	#[case] recipients: Vec<&str>,
	#[case] allowed: bool,
) {
	// Arrange
	let root = version("root", "owner");
	let mut scope = Fixture::new(root);
	scope
		.versions
		.insert("ancestor".into(), version("ancestor", "ancestor-owner"));
	scope.consents.insert(
		key(&("ancestor", "redistributor")),
		Audience {
			revision: 4,
			tenants: BTreeSet::from(["recipient".into()]),
		},
	);
	let edges = BTreeSet::from([ConsentEdge {
		source: "ancestor".into(),
		redistributor: "redistributor".into(),
		grant: SourceGrant::Consent,
	}]);
	let recipients = recipients.into_iter().map(str::to_owned).collect();
	// Act
	let result = consent(&mut scope, &edges, &recipients).await;
	// Assert
	if allowed {
		result.unwrap();
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
	assert_eq!(
		scope.revisions,
		vec![("consent:ancestor:redistributor".into(), 4)]
	);
}
#[rstest]
#[tokio::test]
async fn nested_definition_reads_cannot_replace_the_package_policy_context() {
	// Arrange
	let mut root = version("root", "recipient");
	let child = entry("child");
	root.dependencies.push(Dependency {
		reference: aidash_domain::marketplace::definitions::reference(&child),
		kind: "skill".into(),
		digest: aidash_domain::marketplace::definitions::content(&child),
		package: None,
	});
	let mut scope = Fixture::new(root.clone());
	scope.entries.insert(child.id.clone(), child);
	// Act
	let result = summary(&mut scope, &root, "local").await.unwrap();
	// Assert: package decisions reuse the resource captured before nested reads.
	assert_eq!(result.actions, vec!["read", "install", "share", "consent"]);
	assert_eq!(scope.context, json!({"scope":"dependency"}));
	let package_decisions: Vec<_> = scope
		.decisions
		.iter()
		.filter(|(_, resource)| resource.kind == "package")
		.collect();
	assert_eq!(package_decisions.len(), 4);
	assert!(
		package_decisions
			.iter()
			.all(|(_, resource)| resource.attributes == json!({"scope":"root"}))
	);
	let installation_decisions: Vec<_> = scope
		.decisions
		.iter()
		.filter(|(_, resource)| resource.kind == "installation")
		.collect();
	assert_eq!(installation_decisions.len(), 2);
	assert!(
		installation_decisions
			.iter()
			.all(|(_, resource)| resource.attributes == json!({"scope":"dependency"}))
	);
}
#[rstest]
#[case::reader("owner", 1)]
#[case::owner("recipient", 2)]
#[tokio::test]
async fn detail_discloses_other_recipients_only_with_owner_sharing_authority(
	#[case] owner: &str,
	#[case] visible: usize,
) {
	// Arrange
	let mut scope = Fixture::new(version("root", owner));
	// Act
	let result = detail(&mut scope, "root", "local").await.unwrap();
	// Assert
	assert_eq!(result.audience.revision, 9);
	assert_eq!(result.audience.tenants.len(), visible);
	assert!(result.audience.tenants.contains("recipient"));
	assert_eq!(
		result
			.summary
			.actions
			.iter()
			.any(|action| action == "share"),
		owner == "recipient"
	);
}
#[rstest]
#[tokio::test]
async fn hidden_dependency_removes_read_and_install_without_hiding_owner_management() {
	// Arrange
	let mut root = version("root", "recipient");
	root.dependencies.push(Dependency {
		reference: aidash_domain::marketplace::definitions::reference(&entry("hidden")),
		kind: "skill".into(),
		digest: "frozen".into(),
		package: None,
	});
	let mut scope = Fixture::new(root.clone());
	// Act
	let result = summary(&mut scope, &root, "local").await.unwrap();
	// Assert
	assert_eq!(result.actions, vec!["share", "consent"]);
	assert!(
		!scope
			.decisions
			.iter()
			.any(|(action, _)| action == "marketplace.install")
	);
}
