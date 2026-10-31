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
	groups: BTreeMap<String, BTreeSet<String>>,
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
			"remove_default": ["compaction"],
		}),
	)
}
/// An Agent that keeps the Node's default compaction Binding.
fn agent_with_default_compaction() -> Entry {
	let mut agent = agent("model", &[]);
	agent.config["remove_default"] = json!([]);
	agent
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
			groups: BTreeMap::new(),
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
	async fn subject_groups(&mut self, subject: &str) -> Result<BTreeSet<String>> {
		Ok(self
			.groups
			.get(subject)
			.cloned()
			.unwrap_or_else(|| BTreeSet::from(["members".into()])))
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
async fn missing_references_from_unreadable_definitions_are_anonymous() {
	let bundle = entry(
		"bundle",
		"secret",
		json!({"members": [
		 {"registry_deployment": "aidash://test-deployment", "id": "absent", "version": "1.0.0"}
		]}),
	);
	let mut fixture = Fixture::new(vec![
		agent("model", &[("bundle", "secret")]),
		entry("model", "model", json!({})),
		bundle,
	]);
	fixture.unreadable.insert(key("secret", "1.0.0"));
	let hidden = review(&mut fixture, &reference("writer")).await.unwrap();
	assert_eq!(hidden.hidden, 2);
	assert!(
		hidden
			.items
			.iter()
			.all(|item| item.reference.id != "absent")
	);
	fixture.unreadable.clear();
	let visible = review(&mut fixture, &reference("writer")).await.unwrap();
	assert_eq!(visible.closure_digest, hidden.closure_digest);
	assert!(
		visible
			.items
			.iter()
			.any(|item| item.reference.id == "absent" && item.status == ReviewStatus::Missing)
	);
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
	// Widening is still work for an approver, so the request stays pending.
	assert!(model.awaits_approver());
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
		request: None,
		root_groups: None,
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
async fn root_agent_groups_are_carried_only_to_its_approval() {
	let mut fixture = Fixture::new(vec![
		agent("model", &[]),
		entry("model", "model", json!({})),
	]);
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	let mut value = serde_json::to_value(submission(&review, &["writer", "model"])).unwrap();
	value["root_groups"] = json!(["operators"]);
	let input: ApprovalReviewSubmission = serde_json::from_value(value).unwrap();
	let plan = plan(&review, &input).unwrap();
	let root = plan
		.approvals
		.iter()
		.find(|a| a.entry.id == "writer")
		.unwrap();
	let model = plan
		.approvals
		.iter()
		.find(|a| a.entry.id == "model")
		.unwrap();
	assert_eq!(root.groups, Some(BTreeSet::from(["operators".into()])));
	assert_eq!(model.groups, None);
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

#[rstest]
#[tokio::test]
async fn widening_the_active_revision_skips_activation_while_a_newer_one_is_staged() {
	// Arrange: revision 1 is approved and active for another agent only;
	// revision 2 is staged.
	let mut copy = entry("model", "copy", json!({}));
	copy.installation = Some(Projection {
		contract: 1,
		tenant: "acme".into(),
		installation: "package".into(),
		revision: 1,
	});
	let mut fixture = Fixture::new(vec![agent("copy", &[]), copy]);
	fixture.approve(
		"copy",
		Some(CatalogRestriction {
			subjects: ["other-agent".into()].into(),
			groups: BTreeSet::new(),
		}),
	);
	fixture.installed.insert(
		key("package", "1.0.0"),
		InstalledRevision {
			installation: Installation {
				id: "package".into(),
				tenant: "acme".into(),
				package_key: "package".into(),
				latest_revision: 2,
				active_revision: Some(1),
				activation_revision: 4,
			},
			revision: 1,
			digest: "revision-digest".into(),
			activation_revision: 4,
			active: true,
		},
	);
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	let item = review
		.items
		.iter()
		.find(|item| item.reference.id == "copy")
		.unwrap();
	assert_eq!(item.status, ReviewStatus::Approved);
	assert!(item.widen);
	// Act: widen it together with an unrelated local approval of the root.
	let mut submission = submission(&review, &["writer"]);
	submission.installations.push(ReviewActivation {
		installation: "package".into(),
		revision: 1,
		digest: "revision-digest".into(),
		expected_activation_revision: 4,
		restriction: item.proposed_restriction.clone(),
	});
	let plan = plan(&review, &submission).unwrap();
	let set = restriction_set("acme", &review.node, &review, &plan).unwrap();
	// Assert
	assert_eq!(plan.approvals.len(), 1);
	assert!(plan.installations.is_empty());
	assert!(plan.satisfied_by_activation.is_empty());
	assert_eq!(set.installations.len(), 1);
	let selection = &set.installations[0];
	assert_eq!(selection.revision, 1);
	assert_eq!(selection.expected_activation_revision, 4);
	assert_eq!(selection.expected_catalog_revision, 1);
	assert_eq!(
		Some(selection.restriction.clone()),
		item.proposed_restriction
	);
}

#[rstest]
#[tokio::test]
async fn the_review_includes_the_compaction_decider_the_node_binds_by_default() {
	// Arrange
	let mut fixture = Fixture::new(vec![
		agent_with_default_compaction(),
		entry("model", "model", json!({})),
		crate::registry::system::compaction_decider().unwrap(),
	]);
	fixture.approve("writer", None);
	fixture.approve("model", None);
	// Act
	let pending = review(&mut fixture, &reference("writer")).await.unwrap();
	fixture.approve("aidash.compaction-decider", None);
	let approved = review(&mut fixture, &reference("writer")).await.unwrap();
	// Assert
	let decider = pending
		.items
		.iter()
		.find(|item| item.reference.id == "aidash.compaction-decider")
		.expect("the default compaction Decider is a review item");
	assert!(!decider.approved && !pending.ready);
	assert!(approved.ready);
}

#[rstest]
#[tokio::test]
async fn the_review_omits_the_compaction_decider_when_the_default_is_removed() {
	// Arrange
	let mut fixture = Fixture::new(vec![
		agent("model", &[]),
		entry("model", "model", json!({})),
	]);
	// Act
	let review = review(&mut fixture, &reference("writer")).await.unwrap();
	// Assert
	assert!(
		review
			.items
			.iter()
			.all(|item| item.reference.id != "aidash.compaction-decider")
	);
}

fn named_agent(id: &str, model: &str, dependencies: &[(&str, &str)]) -> Entry {
	let mut value = agent(model, dependencies);
	value.id = id.into();
	value
}
fn delegate(id: &str, target: &str) -> Entry {
	entry(
		"tool",
		id,
		json!({"registry_deployment":"aidash://test-deployment", "provider":"integration.agent@1", "operation":"invoke", "default_alias":id, "tier":"integration", "transport":{"transport":"agent", "node_id":"aidash://test-deployment", "agent":{"id":target,"version":"1.0.0"}}}),
	)
}
fn subject(id: &str) -> String {
	aidash_domain::qualified_agent(&node(), id, "1.0.0")
}
fn item<'a>(review: &'a ApprovalReview, id: &str) -> &'a ReviewItem {
	review
		.items
		.iter()
		.find(|item| item.reference.id == id)
		.unwrap()
}
fn delegated(shared: bool) -> Fixture {
	Fixture::new(vec![
		named_agent("a", if shared { "m" } else { "a-model" }, &[("tool", "t")]),
		delegate("t", "b"),
		named_agent("b", "m", &[]),
		entry("model", "m", json!({})),
		entry("model", "a-model", json!({})),
	])
}

#[tokio::test]
async fn delegated_dependencies_use_the_nearest_agent_and_plan_accepts_proposals() {
	let mut fixture = delegated(false);
	let review = review(&mut fixture, &reference("a")).await.unwrap();
	for (id, consumer) in [("t", "a"), ("b", "a"), ("m", "b")] {
		let reviewed = item(&review, id);
		assert_eq!(reviewed.consumers, vec![subject(consumer)]);
		assert_eq!(
			reviewed.proposed_restriction.as_ref().unwrap().subjects,
			BTreeSet::from([subject(consumer)])
		);
	}
	let plan = plan(
		&review,
		&submission(&review, &["a", "t", "b", "m", "a-model"]),
	)
	.unwrap();
	assert_eq!(plan.approvals.len(), 5);
}

#[tokio::test]
async fn shared_model_unions_consumers_and_widens_existing_restriction() {
	let mut fixture = delegated(true);
	let first = review(&mut fixture, &reference("a")).await.unwrap();
	assert_eq!(
		item(&first, "m")
			.proposed_restriction
			.as_ref()
			.unwrap()
			.subjects,
		BTreeSet::from([subject("a"), subject("b")])
	);
	for id in ["a", "t", "b"] {
		fixture.approve(id, None);
	}
	fixture.approve(
		"m",
		Some(CatalogRestriction {
			subjects: BTreeSet::from([subject("a")]),
			groups: BTreeSet::new(),
		}),
	);
	let second = review(&mut fixture, &reference("a")).await.unwrap();
	assert!(item(&second, "m").widen);
	assert!(!second.ready);
	assert_eq!(
		item(&second, "m")
			.proposed_restriction
			.as_ref()
			.unwrap()
			.subjects,
		BTreeSet::from([subject("a"), subject("b")])
	);
}

#[tokio::test]
async fn non_agent_root_still_discovers_delegated_agent_consumers() {
	let mut fixture = delegated(false);
	let result = review(&mut fixture, &reference("t")).await.unwrap();
	assert_eq!(item(&result, "t").proposed_restriction, None);
	assert_eq!(item(&result, "b").proposed_restriction, None);
	assert_eq!(item(&result, "m").consumers, vec![subject("b")]);
}

#[tokio::test]
async fn cycle_to_root_widens_restricted_root_but_leaves_unrestricted_root_open() {
	let mut fixture = delegated(false);
	fixture
		.definitions
		.get_mut(&key("b", "1.0.0"))
		.unwrap()
		.config["bindings"] = json!([{ "kind":"tool", "target":{"registry_deployment":"aidash://test-deployment", "id":"back", "version":"1.0.0"}, "narrow":{} }]);
	fixture
		.definitions
		.insert(key("back", "1.0.0"), delegate("back", "a"));
	let unrestricted = review(&mut fixture, &reference("a")).await.unwrap();
	assert_eq!(item(&unrestricted, "a").proposed_restriction, None);
	fixture.approve(
		"a",
		Some(CatalogRestriction {
			subjects: BTreeSet::from(["alice".into()]),
			groups: BTreeSet::new(),
		}),
	);
	let restricted = review(&mut fixture, &reference("a")).await.unwrap();
	assert!(item(&restricted, "a").widen);
	assert!(
		item(&restricted, "a")
			.proposed_restriction
			.as_ref()
			.unwrap()
			.subjects
			.contains(&subject("b"))
	);
	assert!(!restricted.ready);
}

#[tokio::test]
async fn hidden_consumer_is_not_disclosed_and_blocks_its_dependency() {
	let mut fixture = delegated(true);
	fixture.unreadable.insert(key("b", "1.0.0"));
	fixture.approve(
		"m",
		Some(CatalogRestriction {
			subjects: BTreeSet::from([subject("b")]),
			groups: BTreeSet::new(),
		}),
	);
	let result = review(&mut fixture, &reference("a")).await.unwrap();
	let model = item(&result, "m");
	assert_eq!(model.status, ReviewStatus::NeedsOtherApprover);
	assert_eq!(model.proposed_restriction, None);
	assert_eq!(model.restriction, None);
	assert_eq!(model.consumers, vec![subject("a")]);
	assert!(!result.ready);
	assert!(
		!serde_json::to_string(&result)
			.unwrap()
			.contains(&subject("b"))
	);
	assert!(matches!(
		plan(&result, &submission(&result, &["m"])),
		Err(Error::Forbidden)
	));
}

#[tokio::test]
async fn group_restrictions_are_checked_for_each_consumer() {
	let mut fixture = delegated(true);
	fixture
		.groups
		.insert(subject("a"), BTreeSet::from(["writers".into()]));
	// B has no subject yet and receives the default members group.

	fixture.approve(
		"m",
		Some(CatalogRestriction {
			subjects: BTreeSet::new(),
			groups: BTreeSet::from(["writers".into(), "members".into()]),
		}),
	);
	let result = review(&mut fixture, &reference("a")).await.unwrap();
	assert!(!item(&result, "m").widen);
	fixture.groups.insert(subject("b"), BTreeSet::new());
	let result = review(&mut fixture, &reference("a")).await.unwrap();
	assert!(item(&result, "m").widen);
	assert_eq!(
		item(&result, "m")
			.proposed_restriction
			.as_ref()
			.unwrap()
			.subjects,
		BTreeSet::from([subject("b")])
	);
}

#[tokio::test]
async fn consumers_reach_children_of_an_already_expanded_shared_bundle() {
	let mut root = named_agent("a", "a-model", &[("bundle", "aa"), ("tool", "t")]);
	root.config["instructions"] = json!("root");
	let nested = named_agent("b", "m", &[("bundle", "aa")]);
	let bundle = entry(
		"bundle",
		"aa",
		json!({"members": [{"registry_deployment":"aidash://test-deployment", "id":"shared-tool", "version":"1.0.0"}]}),
	);
	let mut fixture = Fixture::new(vec![
		root,
		delegate("t", "b"),
		nested,
		bundle,
		tool("shared-tool", "sandboxed"),
		entry("model", "m", json!({})),
		entry("model", "a-model", json!({})),
	]);
	let result = review(&mut fixture, &reference("a")).await.unwrap();
	assert_eq!(
		item(&result, "shared-tool").consumers,
		vec![subject("a"), subject("b")]
	);
	assert_eq!(
		item(&result, "shared-tool")
			.proposed_restriction
			.as_ref()
			.unwrap()
			.subjects,
		BTreeSet::from([subject("a"), subject("b")])
	);
}
