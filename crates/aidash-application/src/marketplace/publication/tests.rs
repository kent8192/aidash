use super::*;
use crate::ports::{
	marketplace::{DefinitionScope, DistributionScope},
	registry::CoreToolCatalog,
};
use aidash_domain::{
	policy::Resource,
	registry::{Entry, Projection},
};
use async_trait::async_trait;
use rstest::rstest;
use std::{collections::BTreeMap, sync::Arc};

struct CoreContracts;
impl CoreToolCatalog for CoreContracts {
	fn specifications(
		&self,
		_: &aidash_domain::capabilities::CoreCapabilities,
	) -> BTreeMap<String, aidash_domain::provider::ToolSpec> {
		BTreeMap::new()
	}
}
fn validation() -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(CoreContracts))
}
fn entry(id: &str) -> Entry {
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":"skill","name":{"en":id},"description":{"en":"Fixture"},"config":{"instructions":"Frozen instructions"}})).unwrap()
}
fn command() -> PublishCommand {
	PublishCommand {
		source: aidash_domain::marketplace::definitions::reference(&entry("source")),
		package_id: "package".into(),
		author: "Fixture".into(),
		permissions: vec![],
		dependencies: vec![],
		idempotency_key: Uuid::nil(),
	}
}
#[derive(Default)]
struct Fixture {
	entries: BTreeMap<String, Entry>,
	versions: BTreeMap<String, Version>,
	audiences: BTreeMap<String, Audience>,
	grants: BTreeMap<String, Audience>,
	revisions: BTreeMap<String, Revision>,
	lineage: BTreeSet<ConsentEdge>,
	replay: Option<Replay>,
	kind: Option<String>,
	calls: Vec<String>,
	authority: Vec<(String, i64)>,
	denied: Option<String>,
}
impl Fixture {
	fn new() -> Self {
		Self {
			entries: BTreeMap::from([("source".into(), entry("source"))]),
			..Default::default()
		}
	}
	fn record(&mut self, action: &str) -> Result<()> {
		self.calls.push(action.into());
		if self.denied.as_deref() == Some(action) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	fn resource(&self, kind: &str, id: &str) -> Resource {
		Resource {
			tenant: "owner".into(),
			kind: kind.into(),
			id: id.into(),
			attributes: json!({}),
		}
	}
	fn writes(&self) -> Vec<&str> {
		self.calls
			.iter()
			.map(String::as_str)
			.filter(|c| {
				c.starts_with("insert")
					|| c.starts_with("event:")
					|| c.starts_with("remember")
					|| c.starts_with("save_")
			})
			.collect()
	}
}
#[async_trait]
impl DefinitionScope for Fixture {
	fn tenant(&self) -> &str {
		"owner"
	}
	async fn raw(&mut self, reference: &EntityRef) -> Result<Entry> {
		self.record("raw")?;
		self.entries
			.get(&reference.id)
			.cloned()
			.ok_or(Error::Forbidden)
	}
	async fn installation(&mut self, _: &str) -> Result<Option<Installation>> {
		Ok(None)
	}
	async fn revision(&mut self, id: &str, _: i64) -> Result<Revision> {
		self.revisions.get(id).cloned().ok_or(Error::Forbidden)
	}
	async fn require_installation_read(&mut self, _: &Installation, _: i64) -> Result<()> {
		self.record("installation.read")
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		self.record(action)?;
		self.entries
			.get(&reference.id)
			.cloned()
			.ok_or(Error::Forbidden)
	}
	async fn require_export(&mut self, _: &Entry) -> Result<()> {
		self.record("registry.export")
	}
	async fn matching_publication(&mut self, _: &EntityRef, _: &str) -> Result<Option<Version>> {
		Ok(None)
	}
	async fn require_reference_read(&mut self, _: Uuid) -> Result<()> {
		self.record("reference.read")
	}
}
#[async_trait]
impl DistributionScope for Fixture {
	async fn version(&mut self, key: &str) -> Result<Option<Version>> {
		self.record("version")?;
		Ok(self.versions.get(key).cloned())
	}
	async fn audience(&mut self, key: &str) -> Result<Option<Audience>> {
		Ok(self.audiences.get(key).cloned())
	}
	async fn consent(&mut self, key: &str) -> Result<Option<Audience>> {
		Ok(self.grants.get(key).cloned())
	}
	fn authority(&mut self, name: String, revision: i64) {
		self.authority.push((name, revision));
	}
	fn operation(&mut self, name: &str, _: Value, _: Option<Uuid>) {
		self.calls.push(format!("operation:{name}"));
	}
	fn package_resource(&self, version: &Version) -> Resource {
		self.resource("package", &version.key)
	}
	fn registry_resource(&self, kind: &str, id: &str) -> Resource {
		self.resource("registry", &format!("{kind}/{id}"))
	}
	fn installation_resource(&self, installation: &Installation, _: Option<i64>) -> Resource {
		self.resource("installation", &installation.id)
	}
	async fn require(&mut self, _: &Resource, action: &str) -> Result<()> {
		self.record(action)
	}
	async fn decide(&mut self, _: &Resource, action: &str) -> Result<bool> {
		self.record(action)?;
		Ok(true)
	}
}
#[async_trait]
impl DistributionWriter for Fixture {
	async fn distribution(
		&mut self,
		package: &str,
		redistributor: Option<&str>,
	) -> Result<Option<Audience>> {
		Ok(match redistributor {
			Some(tenant) => self.grants.get(&key(&(package, tenant))),
			None => self.audiences.get(package),
		}
		.cloned())
	}
	async fn save_distribution(
		&mut self,
		package: &str,
		redistributor: Option<&str>,
		audience: &Audience,
	) -> Result<()> {
		self.record("save_distribution")?;
		match redistributor {
			Some(tenant) => {
				self.grants
					.insert(key(&(package, tenant)), audience.clone());
			}
			None => {
				self.audiences.insert(package.into(), audience.clone());
			}
		}
		Ok(())
	}
	async fn event(&mut self, kind: &str, _: Value) -> Result<()> {
		self.record(&format!("event:{kind}"))
	}
}
#[async_trait]
impl PublicationScope for Fixture {
	fn actor(&self) -> &str {
		"publisher"
	}
	fn audit_resource(&mut self, _: Value) {
		self.calls.push("audit_resource".into());
	}
	async fn provenance(&mut self, _: &Entry) -> Result<BTreeSet<ConsentEdge>> {
		Ok(self.lineage.clone())
	}
	async fn publication_replay(&mut self, _: Uuid) -> Result<Option<Replay>> {
		Ok(self.replay.clone())
	}
	async fn remember_publication(
		&mut self,
		_: Uuid,
		fingerprint: String,
		result: Value,
	) -> Result<()> {
		self.record("remember_publication")?;
		self.replay = Some(Replay {
			fingerprint,
			result,
		});
		Ok(())
	}
	async fn package_kind(&mut self, _: &str, _: &str) -> Result<Option<String>> {
		Ok(self.kind.clone())
	}
	async fn insert_version(&mut self, version: &Version, audience: &Audience) -> Result<()> {
		self.record("insert_version")?;
		self.versions.insert(version.key.clone(), version.clone());
		self.audiences.insert(version.key.clone(), audience.clone());
		Ok(())
	}
}

#[rstest]
#[tokio::test]
async fn publication_keeps_immutable_bytes_and_orders_version_event_and_replay() {
	// Arrange
	let mut scope = Fixture::new();
	let command = command();
	// Act
	let result = publish(&mut scope, &validation(), &command, "node")
		.await
		.unwrap();
	// Assert
	let version = &scope.versions[result["key"].as_str().unwrap()];
	assert_eq!(version.key, key(&("node", "owner", "package", "1.0.0")));
	assert_eq!(version.publisher, "publisher");
	assert_eq!(manifest(version).unwrap().entity, entry("source"));
	assert_eq!(
		scope.audiences[&version.key].tenants,
		BTreeSet::from(["owner".into()])
	);
	assert_eq!(
		scope.writes(),
		vec![
			"insert_version",
			"event:marketplace.published",
			"remember_publication"
		]
	);
	assert_eq!(scope.replay.as_ref().unwrap().fingerprint, key(&command));
	assert_eq!(scope.replay.as_ref().unwrap().result, result);
}

#[rstest]
#[case::direct("direct", true)]
#[case::nested("nested", true)]
#[case::extra("extra", false)]
#[tokio::test]
async fn bundle_publication_names_builtin_members_but_never_ships_one(
	#[case] path: &str,
	#[case] published: bool,
) {
	use aidash_domain::{registry::bindings::QualifiedRef, tool::providers::core_descriptor};
	let node = "aidash://publisher";
	let mut scope = Fixture::new();
	let mut builtin = entry("aidash.workspace_read");
	builtin.kind = "tool".into();
	builtin.config =
		serde_json::to_value(core_descriptor(node, "workspace_read").unwrap()).unwrap();
	scope.entries.insert(builtin.id.clone(), builtin);
	let mut host = entry("host-get");
	host.kind = "tool".into();
	host.config = serde_json::to_value(core_descriptor(node, "outbound_get").unwrap()).unwrap();
	scope.entries.insert(host.id.clone(), host);
	let qualified = |id: &str| QualifiedRef {
		registry_deployment: node.into(),
		id: id.into(),
		version: "1.0.0".into(),
	};
	let member = match path {
		"nested" => {
			let mut inner = entry("inner");
			inner.kind = "bundle".into();
			inner.config = json!({"members":[qualified("aidash.workspace_read")]});
			scope.entries.insert(inner.id.clone(), inner);
			qualified("inner")
		}
		"extra" => qualified("host-get"),
		_ => qualified("aidash.workspace_read"),
	};
	let root = scope.entries.get_mut("source").unwrap();
	root.kind = "bundle".into();
	root.config = json!({"members":[member]});
	let mut command = command();
	if path == "extra" {
		command.dependencies.push(EntityRef {
			id: "aidash.workspace_read".into(),
			version: "1.0.0".into(),
		});
	}
	let result = publish(&mut scope, &validation(), &command, node).await;
	if published {
		assert!(result.is_ok(), "{:?}", result.err());
		let version = scope.versions.values().next().unwrap();
		assert!(
			version
				.dependencies
				.iter()
				.all(|dependency| !dependency.reference.id.starts_with("aidash."))
		);
	} else {
		assert!(matches!(
			result,
			Err(Error::Invalid(message)) if message.contains("system builtin")
		));
		assert!(scope.writes().is_empty());
		assert!(scope.versions.is_empty());
		assert!(scope.replay.is_none());
	}
}

#[tokio::test]
async fn bundle_publication_accepts_portable_host_descriptors() {
	let node = "aidash://publisher";
	let mut scope = Fixture::new();
	let mut host = entry("host-get");
	host.kind = "tool".into();
	host.config = serde_json::to_value(
		aidash_domain::tool::providers::core_descriptor(node, "outbound_get").unwrap(),
	)
	.unwrap();
	scope.entries.insert(host.id.clone(), host);
	let root = scope.entries.get_mut("source").unwrap();
	root.kind = "bundle".into();
	root.config =
		json!({"members":[{"registry_deployment":node,"id":"host-get","version":"1.0.0"}]});
	let result = publish(&mut scope, &validation(), &command(), node)
		.await
		.unwrap();
	let version = &scope.versions[result["key"].as_str().unwrap()];
	assert_eq!(version.kind, "bundle");
	assert_eq!(version.dependencies.len(), 1);
	assert_eq!(version.dependencies[0].reference.id, "host-get");
}
#[rstest]
#[tokio::test]
async fn publication_event_failure_never_records_a_successful_replay() {
	// Arrange
	let mut scope = Fixture::new();
	scope.denied = Some("event:marketplace.published".into());
	// Act
	let result = publish(&mut scope, &validation(), &command(), "node").await;
	// Assert: the caller's transaction handles rollback of the staged version.
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(
		scope.writes(),
		vec!["insert_version", "event:marketplace.published"]
	);
	assert!(scope.replay.is_none());
}
#[rstest]
#[tokio::test]
async fn immutable_kind_collision_is_rejected_before_any_write() {
	// Arrange
	let mut scope = Fixture::new();
	scope.kind = Some("agent".into());
	// Act
	let result = publish(&mut scope, &validation(), &command(), "node").await;
	// Assert
	assert!(
		matches!(result, Err(Error::Conflict(ref message)) if message == "revision or immutable content changed")
	);
	assert!(scope.writes().is_empty());
}
#[rstest]
#[case::visible(true)]
#[case::revoked(false)]
#[tokio::test]
async fn conflicting_replay_rechecks_disclosure_before_returning_its_diagnostic(
	#[case] visible: bool,
) {
	// Arrange
	let mut scope = Fixture::new();
	let mut command = command();
	let original = publish(&mut scope, &validation(), &command, "node")
		.await
		.unwrap();
	scope.calls.clear();
	command.author = "Changed publisher metadata".into();
	if !visible {
		scope
			.audiences
			.get_mut(original["key"].as_str().unwrap())
			.unwrap()
			.tenants
			.clear();
	}
	// Act
	let result = publish(&mut scope, &validation(), &command, "node").await;
	// Assert
	if visible {
		assert!(
			matches!(result, Err(Error::Conflict(ref message)) if message == "idempotency key has different input")
		);
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
	assert!(scope.calls.iter().any(|c| c == "marketplace.read"));
	assert!(scope.writes().is_empty());
}
#[rstest]
#[tokio::test]
async fn an_unchanged_replay_is_authorized_and_never_duplicates_the_publication_event() {
	// Arrange
	let mut scope = Fixture::new();
	let command = command();
	let original = publish(&mut scope, &validation(), &command, "node")
		.await
		.unwrap();
	scope.calls.clear();
	// Act
	let retry = publish(&mut scope, &validation(), &command, "node")
		.await
		.unwrap();
	// Assert
	assert_eq!(retry, original);
	assert!(scope.writes().is_empty());
	assert!(scope.calls.iter().any(|c| c == "marketplace.publish"));
	assert!(scope.calls.iter().any(|c| c == "marketplace.read"));
}
#[rstest]
#[tokio::test]
async fn installed_republication_retains_the_original_graph_instead_of_local_configuration() {
	// Arrange
	let mut scope = Fixture::new();
	let command = command();
	let result = publish(&mut scope, &validation(), &command, "node")
		.await
		.unwrap();
	let source = scope.versions[result["key"].as_str().unwrap()].clone();
	let mut local = entry("local-copy");
	local.config["instructions"] = json!("Private local override");
	local.installation = Some(Projection {
		installation: "installed".into(),
		tenant: "owner".into(),
		revision: 1,
		contract: 1,
	});
	scope.entries.insert(local.id.clone(), local.clone());
	scope.revisions.insert(
		"installed".into(),
		Revision {
			installation: "installed".into(),
			tenant: "owner".into(),
			revision: 1,
			entry: local.clone(),
			digest: source.digest.clone(),
			config: json!({}),
			dependencies: vec![],
			bindings: vec![],
			source: source.clone(),
		},
	);
	let mut second = command;
	second.source = aidash_domain::marketplace::definitions::reference(&local);
	second.package_id = "second".into();
	// Act
	let prepared = prepare(&mut scope, &validation(), &second, "node")
		.await
		.unwrap();
	// Assert
	assert_eq!(
		manifest(&prepared).unwrap().entity,
		manifest(&source).unwrap().entity
	);
	assert_ne!(manifest(&prepared).unwrap().entity.config, local.config);
	assert!(prepared.lineage.contains(&ConsentEdge {
		source: source.key,
		redistributor: "owner".into(),
		grant: SourceGrant::Consent
	}));
}
#[rstest]
#[case::audience(None, true)]
#[case::consent(Some("redistributor"), false)]
#[tokio::test]
async fn only_the_package_audience_retains_the_owner_during_withdrawal(
	#[case] redistributor: Option<&str>,
	#[case] keep_owner: bool,
) {
	// Arrange
	let mut scope = Fixture::new();
	let result = publish(&mut scope, &validation(), &command(), "node")
		.await
		.unwrap();
	scope.calls.clear();
	let revision = if redistributor.is_none() { 1 } else { 0 };
	// Act
	let result = share(
		&mut scope,
		result["key"].as_str().unwrap(),
		redistributor,
		AudienceChange {
			expected_revision: revision,
			tenants: BTreeSet::new(),
		},
	)
	.await
	.unwrap();
	// Assert
	assert_eq!(result.audience.tenants.contains("owner"), keep_owner);
	assert_eq!(result.audience.revision, revision + 1);
	assert_eq!(
		scope.writes(),
		vec![
			"save_distribution",
			"event:marketplace.distribution_changed"
		]
	);
}
#[rstest]
#[tokio::test]
async fn a_joining_audience_tenant_is_owed_events_that_the_owner_does_not_write() {
	// Arrange
	let mut scope = Fixture::new();
	let result = publish(&mut scope, &validation(), &command(), "node")
		.await
		.unwrap();
	scope.calls.clear();
	// Act
	let shared = share(
		&mut scope,
		result["key"].as_str().unwrap(),
		None,
		AudienceChange {
			expected_revision: 1,
			tenants: BTreeSet::from(["recipient".to_owned()]),
		},
	)
	.await
	.unwrap();
	// Assert: the owner's own transaction writes one event; the rest wait for delivery.
	assert_eq!(
		scope.writes(),
		vec![
			"save_distribution",
			"event:marketplace.distribution_changed"
		]
	);
	let owed = shared
		.announcements
		.iter()
		.map(|a| {
			(
				a.kind,
				a.tenants.iter().map(String::as_str).collect::<Vec<_>>(),
			)
		})
		.collect::<Vec<_>>();
	assert_eq!(
		owed,
		vec![
			(events::PUBLISHED, vec!["recipient"]),
			(events::DISTRIBUTION_CHANGED, vec!["recipient"]),
		]
	);
}
#[rstest]
#[tokio::test]
async fn a_consent_change_owes_nothing_to_the_audience() {
	// Arrange
	let mut scope = Fixture::new();
	let result = publish(&mut scope, &validation(), &command(), "node")
		.await
		.unwrap();
	// Act
	let shared = share(
		&mut scope,
		result["key"].as_str().unwrap(),
		Some("redistributor"),
		AudienceChange {
			expected_revision: 0,
			tenants: BTreeSet::from(["onward".to_owned()]),
		},
	)
	.await
	.unwrap();
	// Assert
	assert!(shared.announcements.is_empty());
}
#[rstest]
#[tokio::test]
async fn audience_owner_insertion_cannot_exceed_the_recipient_limit() {
	// Arrange
	let mut scope = Fixture::new();
	let result = publish(&mut scope, &validation(), &command(), "node")
		.await
		.unwrap();
	scope.calls.clear();
	let tenants = (0..128).map(|i| format!("recipient-{i}")).collect();
	// Act
	let result = share(
		&mut scope,
		result["key"].as_str().unwrap(),
		None,
		AudienceChange {
			expected_revision: 1,
			tenants,
		},
	)
	.await;
	// Assert
	assert!(
		matches!(result, Err(Error::Invalid(ref message)) if message == "audience exceeds 128 tenants")
	);
	assert!(scope.writes().is_empty());
}

#[rstest]
#[case::memory("memory", json!({"adapter":"conversation_memory"}))]
#[case::source("source", json!({"adapter":"private_references","digest":"a".repeat(64)}))]
#[tokio::test]
async fn native_context_descriptors_publish_without_private_content(
	#[case] kind: &str,
	#[case] adapter: Value,
) {
	let mut scope = Fixture::new();
	let definition = scope.entries.get_mut("source").unwrap();
	definition.kind = kind.into();
	definition.config = json!({"schema_version":1,"source":adapter});
	let result = publish(&mut scope, &validation(), &command(), "aidash://publisher")
		.await
		.unwrap();
	let frozen = &scope.versions[result["key"].as_str().unwrap()];
	assert_eq!(frozen.kind, kind);
	assert_eq!(
		manifest(frozen).unwrap().entity.config["source"]["adapter"],
		adapter["adapter"]
	);
	assert!(!frozen.manifest_source.contains("reference_documents"));
}

mod duty_templates {
	//! Duty templates share the publication tail and the read checks of
	//! every package (#1011 D4).
	use super::*;
	use crate::marketplace::duty_templates::{PublishTemplate, installable, publish};
	use aidash_domain::duties::{
		Definition,
		scripts::{Language, Script},
		templates::{KIND, Slot, Template, TriggerNote},
	};

	fn now() -> chrono::DateTime<chrono::Utc> {
		chrono::Utc::now()
	}

	fn template() -> Template {
		let duty = Definition {
			title: "GitHub CI duty".into(),
			mission: "Fix failing CI runs.".into(),
			procedure: "1. Read the failed job.".into(),
			goal: None,
			agent: EntityRef {
				id: "ci-fixer".into(),
				version: "1.0.0".into(),
			},
			trigger: None,
			approval_mode: None,
			max_lateness_seconds: 3600,
			pause_after_failures: 3,
			router: Some(Script {
				language: Language::Python,
				source: "print('{\"action\":\"task\"}')".into(),
			}),
			watcher: None,
		};
		Template::from_duty(
			&duty,
			&[aidash_domain::duties::events::TriggerKind::Webhook],
			&[TriggerNote {
				kind: Slot::Webhook,
				instruction: "Add the URL as a workflow_run webhook.".into(),
			}],
			true,
		)
		.unwrap()
	}

	fn input() -> PublishTemplate {
		PublishTemplate {
			package_id: "github-ci-duty".into(),
			version: "1.0.0".into(),
			author: "Acme".into(),
			description: String::new(),
			idempotency_key: Uuid::nil(),
		}
	}

	#[rstest]
	#[tokio::test]
	async fn a_published_template_reads_back_for_installing() {
		// Arrange
		let mut scope = Fixture::new();
		// Act
		let result = publish(&mut scope, &template(), &input(), "node", now())
			.await
			.unwrap();
		let key = result["key"].as_str().unwrap().to_owned();
		let digest = result["digest"].as_str().unwrap().to_owned();
		let (version, read) = installable(&mut scope, &key, &digest, "node", now())
			.await
			.unwrap();
		// Assert: an owner-only package of its own kind, with no Duty id.
		assert_eq!(version.kind, KIND);
		assert_eq!(version.source.id, "github-ci-duty");
		assert!(version.dependencies.is_empty() && version.lineage.is_empty());
		assert_eq!(
			scope.audiences[&key].tenants,
			BTreeSet::from(["owner".into()])
		);
		assert_eq!(read, template());
		assert!(scope.calls.iter().any(|call| call == "marketplace.publish"));
		assert!(scope.calls.iter().any(|call| call == "marketplace.install"));
		assert_eq!(
			scope.writes(),
			vec![
				"insert_version",
				"event:marketplace.published",
				"remember_publication"
			]
		);
	}

	#[rstest]
	#[tokio::test]
	async fn publishing_needs_marketplace_publish_and_writes_nothing_without_it() {
		let mut scope = Fixture::new();
		scope.denied = Some("marketplace.publish".into());
		let result = publish(&mut scope, &template(), &input(), "node", now()).await;
		assert!(matches!(result, Err(Error::Forbidden)), "{result:?}");
		assert!(scope.writes().is_empty());
	}

	#[rstest]
	#[tokio::test]
	async fn a_template_with_a_credential_is_never_published() {
		let mut scope = Fixture::new();
		let mut template = template();
		template.procedure = "export TOKEN=ghp_0123456789abcdefABCDEF0123456789abcd".into();
		let result = publish(&mut scope, &template, &input(), "node", now()).await;
		assert!(
			result
				.as_ref()
				.is_err_and(|error| error.to_string().contains("carries no secrets")),
			"{result:?}"
		);
		assert!(scope.writes().is_empty());
	}

	#[rstest]
	#[tokio::test]
	async fn a_package_id_used_by_another_kind_conflicts() {
		let mut scope = Fixture::new();
		scope.kind = Some("skill".into());
		let result = publish(&mut scope, &template(), &input(), "node", now()).await;
		assert!(matches!(result, Err(Error::Conflict(_))), "{result:?}");
	}

	#[rstest]
	#[tokio::test]
	async fn only_a_reviewed_duty_template_installs_as_a_duty() {
		// Arrange: a template, and a skill package.
		let mut scope = Fixture::new();
		let result = publish(&mut scope, &template(), &input(), "node", now())
			.await
			.unwrap();
		let key = result["key"].as_str().unwrap().to_owned();
		// The fixture replays any key; the skill is a request of its own.
		scope.replay = None;
		let skill = super::publish(&mut scope, &validation(), &command(), "node")
			.await
			.unwrap();
		// Act + Assert: another digest conflicts.
		let changed = installable(&mut scope, &key, "sha256:other", "node", now())
			.await
			.map(|_| ());
		assert!(matches!(changed, Err(Error::Conflict(_))), "{changed:?}");
		// Act + Assert: a skill is not a Duty template.
		let read = installable(
			&mut scope,
			skill["key"].as_str().unwrap(),
			skill["digest"].as_str().unwrap(),
			"node",
			now(),
		)
		.await
		.map(|_| ());
		assert!(matches!(read, Err(Error::Invalid(_))), "{read:?}");
		// Act + Assert: no marketplace.install, no template.
		scope.denied = Some("marketplace.install".into());
		let digest = result["digest"].as_str().unwrap();
		let denied = installable(&mut scope, &key, digest, "node", now())
			.await
			.map(|_| ());
		assert!(matches!(denied, Err(Error::Forbidden)), "{denied:?}");
	}

	#[rstest]
	#[tokio::test]
	async fn an_installed_manifest_with_a_secret_field_is_refused() {
		// Arrange: a version whose template names a webhook secret, as another
		// Tenant's tampered package could.
		let mut scope = Fixture::new();
		let mut package = template()
			.package(aidash_domain::duties::templates::Listing {
				package_id: "github-ci-duty",
				version: "1.0.0",
				author: "Acme",
				description: "",
			})
			.unwrap();
		package.entity.config["webhook_secret"] = json!("s3cr3t");
		let manifest_source = serde_json::to_string(&package).unwrap();
		let digest = format!(
			"sha256:{:x}",
			<sha2::Sha256 as sha2::Digest>::digest(manifest_source.as_bytes())
		);
		let version = Version {
			key: "tampered".into(),
			repository: "node".into(),
			owner_tenant: "owner".into(),
			package_id: "github-ci-duty".into(),
			version: "1.0.0".into(),
			kind: KIND.into(),
			publisher: "publisher".into(),
			source: EntityRef {
				id: "github-ci-duty".into(),
				version: "1.0.0".into(),
			},
			manifest_source,
			digest: digest.clone(),
			dependencies: vec![],
			lineage: BTreeSet::new(),
		};
		scope.versions.insert("tampered".into(), version);
		scope.audiences.insert(
			"tampered".into(),
			Audience {
				revision: 1,
				tenants: BTreeSet::from(["owner".into()]),
			},
		);
		// Act
		let result = installable(&mut scope, "tampered", &digest, "node", now())
			.await
			.map(|_| ());
		// Assert
		assert!(
			result
				.as_ref()
				.is_err_and(|error| error.to_string().contains("not a valid Duty template")),
			"{result:?}"
		);
	}
}
