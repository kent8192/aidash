use super::*;
use crate::ports::catalog_review::{ApprovalSnapshot, InstalledRevision};
use aidash_domain::{marketplace::Installation, nodes::NodeAddress, registry::Projection};
use rstest::rstest;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

struct Fixture {
	node: NodeAddress,
	definitions: BTreeMap<(String, String), Entry>,
	approvals: BTreeMap<(String, String), (bool, i64, Option<CatalogRestriction>)>,
	installed: BTreeMap<(String, String), InstalledRevision>,
	unreadable: BTreeSet<(String, String)>,
	cannot_approve: BTreeSet<(String, String)>,
	provider_missing: BTreeSet<(String, String)>,
	slot_missing: BTreeSet<(String, String)>,
}
fn key(id: &str, version: &str) -> (String, String) {
	(id.into(), version.into())
}
fn node() -> NodeAddress {
	NodeAddress::derived("aidash://test-deployment", "acme")
}
fn entry(kind: &str, id: &str, config: serde_json::Value) -> Entry {
	serde_json::from_value(json!({
		"id": id,
		"version": "1.0.0",
		"kind": kind,
		"name": {"en": id},
		"description": {"en": format!("{id} description")},
		"config": config,
	}))
	.unwrap()
}
fn tool(id: &str, tier: &str) -> Entry {
	entry(
		"tool",
		id,
		json!({
			"registry_deployment": "aidash://test-deployment",
			"provider": "core",
			"operation": if tier == "builtin" {"workspace_read"} else {"workspace_write"},
			"default_alias": id,
			"tier": tier,
		}),
	)
}
fn agent(model: &str, dependencies: &[(&str, &str)]) -> Entry {
	entry(
		"agent",
		"writer",
		json!({
			"schema_version": 1,
			"model": {"id": model, "version": "1.0.0"},
			"instructions": "Write",
			"bindings": dependencies.iter().map(|(kind, id)| json!({
				"kind": kind,
				"target": {"registry_deployment": "aidash://test-deployment", "id": id, "version": "1.0.0"},
				"narrow": {},
			})).collect::<Vec<_>>(),
			"remove_default": [],
		}),
	)
}
impl Fixture {
	fn new(entries: Vec<Entry>) -> Self {
		Self {
			node: node(),
			definitions: entries
				.into_iter()
				.map(|entry| (key(&entry.id, &entry.version), entry))
				.collect(),
			approvals: BTreeMap::new(),
			installed: BTreeMap::new(),
			unreadable: BTreeSet::new(),
			cannot_approve: BTreeSet::new(),
			provider_missing: BTreeSet::new(),
			slot_missing: BTreeSet::new(),
		}
	}
	fn approve(&mut self, id: &str, restriction: Option<CatalogRestriction>) {
		self.approvals
			.insert(key(id, "1.0.0"), (true, 1, restriction));
	}
}
#[async_trait::async_trait]
impl ApprovalReviewScope for Fixture {
	fn tenant(&self) -> &str {
		"acme"
	}
	fn node(&self) -> &NodeAddress {
		&self.node
	}
	async fn definition(&mut self, reference: &EntityRef) -> Result<Option<Entry>> {
		Ok(self
			.definitions
			.get(&key(&reference.id, &reference.version))
			.cloned())
	}
	async fn approval(&mut self, reference: &EntityRef) -> Result<Option<ApprovalSnapshot>> {
		Ok(self
			.approvals
			.get(&key(&reference.id, &reference.version))
			.map(|(enabled, revision, restriction)| ApprovalSnapshot {
				enabled: *enabled,
				revision: *revision,
				restriction: restriction.clone(),
			}))
	}
	async fn installed(&mut self, projection: &Projection) -> Result<Option<InstalledRevision>> {
		let Some(installed) = self.installed.get(&key(&projection.installation, "1.0.0")) else {
			return Ok(None);
		};
		Ok(Some(InstalledRevision {
			installation: installed.installation.clone(),
			revision: installed.revision,
			digest: installed.digest.clone(),
			activation_revision: installed.activation_revision,
			active: installed.active,
		}))
	}
	async fn can_read(&mut self, entry: &Entry) -> Result<bool> {
		Ok(!self.unreadable.contains(&key(&entry.id, &entry.version)))
	}
	async fn require_read(&mut self, entry: &Entry) -> Result<()> {
		if self.unreadable.contains(&key(&entry.id, &entry.version)) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn can_approve(&mut self, entry: &Entry) -> Result<bool> {
		Ok(!self
			.cannot_approve
			.contains(&key(&entry.id, &entry.version)))
	}
	async fn provider_available(&mut self, entry: &Entry) -> Result<bool> {
		Ok(!self
			.provider_missing
			.contains(&key(&entry.id, &entry.version)))
	}
	async fn source_slot_available(&mut self, entry: &Entry) -> Result<bool> {
		Ok(!self.slot_missing.contains(&key(&entry.id, &entry.version)))
	}
}

fn reference(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
}

#[rstest]
#[tokio::test]
async fn the_review_walks_the_transitive_closure_once_and_survives_a_cycle() {
	let bundle = entry(
		"bundle",
		"tools",
		json!({"members": [
			{"registry_deployment": "aidash://test-deployment", "id": "sandbox", "version": "1.0.0"},
			{"registry_deployment": "aidash://test-deployment", "id": "tools", "version": "1.0.0"},
		]}),
	);
	let mut fixture = Fixture::new(vec![
		agent("model", &[("bundle", "tools")]),
		entry("model", "model", json!({})),
		bundle,
		tool("sandbox", "sandboxed"),
	]);
	fixture.approve("model", None);
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	let ids: Vec<_> = review
		.items
		.iter()
		.map(|item| item.reference.id.as_str())
		.collect();
	assert_eq!(ids, ["writer", "model", "sandbox", "tools"]);
	assert_eq!(review.hidden, 0);
}

#[rstest]
#[tokio::test]
async fn builtins_that_need_no_approval_are_satisfied_and_omitted() {
	let mut fixture = Fixture::new(vec![
		agent("model", &[("tool", "native")]),
		entry("model", "model", json!({})),
		tool("native", "builtin"),
	]);
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	assert!(
		review
			.items
			.iter()
			.all(|item| item.reference.id != "native")
	);
	assert!(review.items.iter().any(|item| item.reference.id == "model"));
}

#[rstest]
#[tokio::test]
async fn a_marketplace_copy_needs_activation_and_carries_its_fence() {
	let mut copy = entry("model", "copy", json!({}));
	copy.installation = Some(Projection {
		contract: 1,
		tenant: "acme".into(),
		installation: "package".into(),
		revision: 2,
	});
	let mut fixture = Fixture::new(vec![agent("copy", &[]), copy]);
	fixture.installed.insert(
		key("package", "1.0.0"),
		InstalledRevision {
			installation: Installation {
				id: "package".into(),
				tenant: "acme".into(),
				package_key: "package".into(),
				latest_revision: 2,
				active_revision: None,
				activation_revision: 4,
			},
			revision: 2,
			digest: "digest".into(),
			activation_revision: 4,
			active: false,
		},
	);
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	let copy = review
		.items
		.iter()
		.find(|item| item.reference.id == "copy")
		.unwrap();
	assert_eq!(copy.status, ReviewStatus::NeedsMarketplaceActivation);
	assert_eq!(
		copy.installation.as_ref().unwrap(),
		&ReviewInstallation {
			installation: "package".into(),
			revision: 2,
			digest: "digest".into(),
			expected_activation_revision: 4,
			active_revision: None,
		}
	);
}

#[rstest]
#[tokio::test]
async fn unreadable_dependencies_are_counted_and_never_named() {
	let mut fixture = Fixture::new(vec![
		agent("model", &[("tool", "secret")]),
		entry("model", "model", json!({})),
		tool("secret", "sandboxed"),
	]);
	fixture.unreadable.insert(key("secret", "1.0.0"));
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	assert_eq!(review.hidden, 1);
	assert!(
		review
			.items
			.iter()
			.all(|item| item.reference.id != "secret")
	);
	assert!(!review.ready);
}

#[rstest]
#[tokio::test]
async fn the_closure_digest_changes_with_an_exact_definition() {
	let mut first = Fixture::new(vec![
		agent("model", &[]),
		entry("model", "model", json!({"provider": "openrouter"})),
	]);
	let mut second = Fixture::new(vec![
		agent("model", &[]),
		entry(
			"model",
			"model",
			json!({"provider": "openrouter", "model_id": "other"}),
		),
	]);
	let first = review(&mut first, &reference("writer")).await.unwrap();
	let second = review(&mut second, &reference("writer")).await.unwrap();
	assert_ne!(first.closure_digest, second.closure_digest);
	assert_eq!(first.items[0].reference.id, "writer");
}

#[rstest]
#[tokio::test]
async fn an_agent_root_proposes_its_subject_on_dependencies_but_not_itself() {
	let mut fixture = Fixture::new(vec![
		agent("model", &[]),
		entry("model", "model", json!({})),
	]);
	fixture.approve("model", None);
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	let root = review
		.items
		.iter()
		.find(|item| item.reference.id == "writer")
		.unwrap();
	let model = review
		.items
		.iter()
		.find(|item| item.reference.id == "model")
		.unwrap();
	assert_eq!(root.proposed_restriction, None);
	assert_eq!(
		model.proposed_restriction.as_ref().unwrap().subjects,
		[aidash_domain::qualified_agent(&node(), "writer", "1.0.0")].into()
	);
	assert!(!model.widen);
}

#[rstest]
#[tokio::test]
async fn an_approved_dependency_restricted_to_others_is_marked_widen() {
	let mut fixture = Fixture::new(vec![
		agent("model", &[]),
		entry("model", "model", json!({})),
	]);
	fixture.approve(
		"model",
		Some(CatalogRestriction {
			subjects: ["someone-else".into()].into(),
			groups: BTreeSet::new(),
		}),
	);
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	let model = review
		.items
		.iter()
		.find(|item| item.reference.id == "model")
		.unwrap();
	assert!(model.widen);
	assert_eq!(model.status, ReviewStatus::Approved);
	assert!(!review.ready);
}

#[rstest]
#[tokio::test]
async fn provider_and_slot_blockers_are_reported_before_approval_readiness() {
	let mut fixture = Fixture::new(vec![
		agent("model", &[("tool", "remote"), ("tool", "slot")]),
		tool("remote", "sandboxed"),
		tool("slot", "sandboxed"),
		entry("model", "model", json!({})),
	]);
	fixture.provider_missing.insert(key("remote", "1.0.0"));
	fixture.slot_missing.insert(key("slot", "1.0.0"));
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	let status = |id: &str| {
		review
			.items
			.iter()
			.find(|item| item.reference.id == id)
			.unwrap()
			.status
	};
	assert_eq!(status("remote"), ReviewStatus::ProviderUnavailable);
	assert_eq!(status("slot"), ReviewStatus::SecretOrConnectionMissing);
	assert_eq!(status("model"), ReviewStatus::Approvable);
}

#[rstest]
#[tokio::test]
async fn a_review_ready_requires_every_named_item_approved_without_widening() {
	let mut fixture = Fixture::new(vec![
		agent("model", &[]),
		entry("model", "model", json!({})),
	]);
	fixture.approve("writer", None);
	fixture.approve("model", None);
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	assert!(review.ready);
}

fn submission(review: &ApprovalReview, ids: &[&str]) -> ApprovalReviewSubmission {
	ApprovalReviewSubmission {
		node: review.node.clone(),
		root: review.root.clone(),
		closure_digest: review.closure_digest.clone(),
		items: ids
			.iter()
			.map(|id| {
				let item = review
					.items
					.iter()
					.find(|item| item.reference.id == *id)
					.unwrap();
				ReviewApproval {
					reference: item.reference.clone(),
					digest: item.digest.clone().unwrap(),
					expected_revision: item.expected_revision,
					restriction: item.proposed_restriction.clone(),
				}
			})
			.collect(),
		installations: vec![],
	}
}

#[rstest]
#[tokio::test]
async fn a_stale_digest_or_unapproved_item_rejects_the_whole_plan() {
	let mut fixture = Fixture::new(vec![
		agent("model", &[]),
		entry("model", "model", json!({})),
	]);
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	let mut stale = submission(&review, &["writer"]);
	stale.closure_digest = "sha256:changed".into();
	assert!(matches!(plan(&review, &stale), Err(Error::Conflict(_))));
	let mut changed = submission(&review, &["writer"]);
	changed.items[0].digest = "sha256:changed".into();
	assert!(matches!(plan(&review, &changed), Err(Error::Conflict(_))));
}

#[rstest]
#[tokio::test]
async fn a_plan_accepts_a_partial_subset_and_rejects_an_item_this_principal_cannot_approve() {
	let mut fixture = Fixture::new(vec![
		agent("model", &[]),
		entry("model", "model", json!({})),
		entry("model", "other", json!({})),
	]);
	fixture.cannot_approve.insert(key("writer", "1.0.0"));
	let review_writer = review(&mut fixture, &reference("writer")).await.unwrap();
	let only_writer = submission(&review_writer, &["writer"]);
	assert!(matches!(
		plan(&review_writer, &only_writer),
		Err(Error::Forbidden)
	));
	fixture.cannot_approve.clear();
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	let only_model = submission(&review, &["model"]);
	let plan = plan(&review, &only_model).unwrap();
	assert_eq!(plan.approvals.len(), 1);
	assert_eq!(plan.approvals[0].entry.id, "model");
}

#[rstest]
#[tokio::test]
async fn an_installation_plan_uses_the_review_node_and_restriction() {
	let mut copy = entry("model", "copy", json!({}));
	copy.installation = Some(Projection {
		contract: 1,
		tenant: "acme".into(),
		installation: "package".into(),
		revision: 2,
	});
	let mut fixture = Fixture::new(vec![agent("copy", &[]), copy]);
	fixture.installed.insert(
		key("package", "1.0.0"),
		InstalledRevision {
			installation: Installation {
				id: "package".into(),
				tenant: "acme".into(),
				package_key: "package".into(),
				latest_revision: 2,
				active_revision: None,
				activation_revision: 4,
			},
			revision: 2,
			digest: "revision-digest".into(),
			activation_revision: 4,
			active: false,
		},
	);
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	let mut submission = submission(&review, &[]);
	submission.installations.push(ReviewActivation {
		installation: "package".into(),
		revision: 2,
		digest: "revision-digest".into(),
		expected_activation_revision: 4,
		restriction: None,
	});
	let plan = plan(&review, &submission).unwrap();
	assert_eq!(plan.installations.len(), 1);
	assert!(
		plan.satisfied_by_activation
			.contains(&("copy".into(), "1.0.0".into()))
	);
}
