use super::*;
use aidash_domain::policy::Resource;
use async_trait::async_trait;
use rstest::rstest;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
struct Fixture {
	inherited: bool,
	approved: BTreeSet<(String, String)>,
	admissions: BTreeMap<(String, String), Entry>,
	documents: Vec<Value>,
	inactive: BTreeSet<String>,
	denied: Option<String>,
	restricted_ids: BTreeSet<String>,
	/// Direct users every restriction admits.
	restriction_admits: BTreeSet<String>,
	restriction_checks: Vec<String>,
	calls: Vec<String>,
}
fn definition(id: &str) -> Value {
	json!({"id":id,"version":"1.0.0","kind":"skill","name":{"en":id},"description":{"en":"Fixture"},"config":{"instructions":"Frozen"}})
}
fn reference(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
}
impl Fixture {
	fn new() -> Self {
		Self {
			inherited: false,
			approved: BTreeSet::new(),
			admissions: BTreeMap::new(),
			documents: vec![definition("first"), definition("second")],
			inactive: BTreeSet::new(),
			denied: None,
			restricted_ids: BTreeSet::new(),
			restriction_admits: BTreeSet::new(),
			restriction_checks: vec![],
			calls: vec![],
		}
	}
}
#[async_trait]
impl CatalogScope for Fixture {
	fn tenant(&self) -> &str {
		"tenant"
	}
	fn inherited_lease(&self) -> bool {
		self.inherited
	}
	fn approved(&self, reference: &EntityRef) -> bool {
		self.approved
			.contains(&(reference.id.clone(), reference.version.clone()))
	}
	fn direct_user(&self, reference: &EntityRef) -> String {
		format!("user-of:{}", reference.id)
	}
	fn agent_subject(&self, agent: &EntityRef) -> Option<String> {
		Some(format!("agent:{}", agent.id))
	}
	async fn permits_restriction(
		&mut self,
		reference: &EntityRef,
		direct_user: &str,
	) -> Result<bool> {
		self.restriction_checks
			.push(format!("{}:{direct_user}", reference.id));
		Ok(!self.restricted_ids.contains(&reference.id)
			|| self.restriction_admits.contains(direct_user))
	}
	fn remember(&mut self, reference: &EntityRef) {
		self.calls.push(format!("remember:{}", reference.id));
		self.approved
			.insert((reference.id.clone(), reference.version.clone()));
	}
	fn admitted(&self, reference: &EntityRef, action: &str) -> Option<Entry> {
		self.admissions
			.get(&(reference.id.clone(), action.into()))
			.cloned()
	}
	fn admit(&mut self, reference: &EntityRef, action: &str, entry: &Entry) {
		self.admissions
			.insert((reference.id.clone(), action.into()), entry.clone());
	}
	async fn distribution_lock(&mut self) -> Result<()> {
		self.calls.push("lock".into());
		Ok(())
	}
	async fn document(&mut self, reference: &EntityRef) -> Result<Option<Value>> {
		self.calls.push(format!("document:{}", reference.id));
		Ok(self
			.documents
			.iter()
			.find(|d| d["id"] == reference.id && d["version"] == reference.version)
			.cloned())
	}
	async fn documents(&mut self) -> Result<Vec<Value>> {
		self.calls.push("documents".into());
		Ok(self.documents.clone())
	}
	fn resource(&self, entry: &Entry) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: entry.kind.clone(),
			id: entry.id.clone(),
			attributes: json!({}),
		}
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.calls.push(format!("require:{}:{action}", resource.id));
		if self.denied.as_deref() == Some(action) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.calls.push(format!("decide:{}:{action}", resource.id));
		Ok(self.denied.as_deref() != Some(action))
	}
	async fn active(&mut self, entry: &Entry) -> Result<bool> {
		self.calls.push(format!("active:{}", entry.id));
		Ok(!self.inactive.contains(&entry.id))
	}
	async fn check_pinned(&mut self, entry: &Entry) -> Result<()> {
		self.calls.push(format!("pinned:{}", entry.id));
		match self.denied.as_deref() {
			Some("pinned") => Err(Error::Forbidden),
			Some("external") => Err(Error::External("fixture dependency read failed".into())),
			_ => Ok(()),
		}
	}
}
#[rstest]
#[tokio::test]
async fn inherited_lease_cannot_expand_its_approved_definition_frontier() {
	let mut scope = Fixture::new();
	scope.inherited = true;
	let result = entry(&mut scope, &reference("first"), "registry.read").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.calls, Vec::<String>::new());
}
#[rstest]
#[tokio::test]
async fn inherited_exact_read_reuses_its_outer_lock_and_rechecks_current_policy() {
	let mut scope = Fixture::new();
	scope.inherited = true;
	scope.approved.insert(("first".into(), "1.0.0".into()));
	let entry = entry(&mut scope, &reference("first"), "registry.read")
		.await
		.unwrap();
	assert_eq!(entry.id, "first");
	assert_eq!(
		scope.calls,
		vec![
			"document:first",
			"require:first:registry.read",
			"remember:first"
		]
	);
}
#[rstest]
#[tokio::test]
async fn ordinary_exact_read_locks_distribution_before_catalog_and_policy() {
	let mut scope = Fixture::new();
	let result = entry(&mut scope, &reference("first"), "registry.export")
		.await
		.unwrap();
	assert_eq!(result.id, "first");
	assert_eq!(
		scope.calls,
		vec![
			"lock",
			"document:first",
			"require:first:registry.export",
			"remember:first"
		]
	);
}
#[rstest]
#[tokio::test]
async fn repeated_exact_read_reuses_its_admission_and_decides_other_actions() {
	let mut scope = Fixture::new();
	entry(&mut scope, &reference("first"), "registry.read")
		.await
		.unwrap();
	scope.calls.clear();
	let again = entry(&mut scope, &reference("first"), "registry.read")
		.await
		.unwrap();
	assert_eq!(again.id, "first");
	assert_eq!(scope.calls, vec!["remember:first"]);
	scope.calls.clear();
	entry(&mut scope, &reference("first"), "registry.export")
		.await
		.unwrap();
	assert_eq!(
		scope.calls,
		vec![
			"lock",
			"document:first",
			"require:first:registry.export",
			"remember:first"
		]
	);
}
#[rstest]
#[tokio::test]
async fn recorded_admission_cannot_bypass_the_inherited_approval_frontier() {
	let mut scope = Fixture::new();
	entry(&mut scope, &reference("first"), "registry.read")
		.await
		.unwrap();
	scope.inherited = true;
	scope.approved.clear();
	scope.calls.clear();
	let result = entry(&mut scope, &reference("first"), "registry.read").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.calls, Vec::<String>::new());
}
#[rstest]
#[tokio::test]
async fn denied_exact_read_is_never_recorded_as_an_admission() {
	let mut scope = Fixture::new();
	scope.denied = Some("registry.read".into());
	let result = entry(&mut scope, &reference("first"), "registry.read").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	scope.denied = None;
	scope.calls.clear();
	entry(&mut scope, &reference("first"), "registry.read")
		.await
		.unwrap();
	assert_eq!(
		scope.calls,
		vec![
			"lock",
			"document:first",
			"require:first:registry.read",
			"remember:first"
		]
	);
}
#[rstest]
#[case::wrong_contract("tenant", 2)]
#[case::foreign_tenant("other", 1)]
#[tokio::test]
async fn invalid_projected_definition_cannot_gain_policy_or_cached_authority(
	#[case] tenant: &str,
	#[case] contract: u8,
) {
	let mut scope = Fixture::new();
	scope.documents[0]["installation"] =
		json!({"tenant":tenant,"contract":contract,"installation":"installation","revision":1});
	let result = entry(&mut scope, &reference("first"), "registry.read").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.calls, vec!["lock", "document:first"]);
	assert_eq!(scope.approved.len(), 0);
}
#[rstest]
#[tokio::test]
async fn denied_exact_read_is_not_added_to_the_inherited_approval_cache() {
	let mut scope = Fixture::new();
	scope.denied = Some("registry.read".into());
	let result = entry(&mut scope, &reference("first"), "registry.read").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.approved.len(), 0);
}
#[rstest]
#[tokio::test]
async fn inherited_discovery_excludes_unapproved_candidates_before_activation_checks() {
	let mut scope = Fixture::new();
	scope.inherited = true;
	scope.approved.insert(("second".into(), "1.0.0".into()));
	let entries = list(&mut scope, &Search::default()).await.unwrap();
	assert_eq!(
		entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
		vec!["second"]
	);
	assert_eq!(
		scope.calls,
		vec![
			"documents",
			"active:second",
			"decide:second:registry.read",
			"remember:second",
			"pinned:second"
		]
	);
}
#[rstest]
#[tokio::test]
async fn inactive_definition_is_excluded_before_search_and_policy_evaluation() {
	let mut scope = Fixture::new();
	scope.inactive.insert("first".into());
	let entries = list(&mut scope, &Search::default()).await.unwrap();
	assert_eq!(
		entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
		vec!["second"]
	);
	assert!(
		!scope
			.calls
			.iter()
			.any(|c| c == "decide:first:registry.read")
	);
}
#[rstest]
#[tokio::test]
async fn search_exclusion_does_not_cache_or_evaluate_policy_for_nonmatches() {
	let mut scope = Fixture::new();
	let entries = list(
		&mut scope,
		&Search {
			query: Some("SECOND".into()),
			..Default::default()
		},
	)
	.await
	.unwrap();
	assert_eq!(
		entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
		vec!["second"]
	);
	assert_eq!(
		scope.approved,
		BTreeSet::from([("second".into(), "1.0.0".into())])
	);
	assert!(
		!scope
			.calls
			.iter()
			.any(|c| c == "decide:first:registry.read")
	);
}
#[rstest]
#[tokio::test]
async fn discovery_requires_current_policy_and_pinned_dependency_authority() {
	let mut scope = Fixture::new();
	scope.denied = Some("pinned".into());
	let entries = list(&mut scope, &Search::default()).await.unwrap();
	assert_eq!(entries.len(), 0);
	// The exact root grant remains valid; rejected dependencies are never returned.
	assert_eq!(scope.approved.len(), 2);
}
#[rstest]
#[tokio::test]
async fn dependency_connection_failure_retains_the_external_retry_error() {
	let mut scope = Fixture::new();
	scope.denied = Some("external".into());
	let result = get(&mut scope, &reference("first")).await;
	assert!(
		matches!(result,Err(Error::External(message)) if message=="fixture dependency read failed")
	);
}

#[rstest]
#[tokio::test]
async fn a_restricted_out_direct_user_is_refused_like_an_unapproved_version() {
	let mut scope = Fixture::new();
	scope.restricted_ids.insert("first".into());
	let result = entry(&mut scope, &reference("first"), "registry.read").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(
		scope.calls,
		vec!["lock"],
		"a refused restriction records no catalog read, policy decision or admission"
	);
}

#[rstest]
#[tokio::test]
async fn discovery_never_offers_a_definition_its_restriction_excludes() {
	let mut scope = Fixture::new();
	scope.restricted_ids.insert("first".into());
	let listed = list(&mut scope, &Search::default()).await.unwrap();
	assert_eq!(
		listed
			.iter()
			.map(|entry| entry.id.as_str())
			.collect::<Vec<_>>(),
		["second"],
		"a restricted-out definition is not offered by discovery"
	);
}

#[rstest]
#[tokio::test]
async fn a_delegated_closure_definition_is_restricted_to_its_nearest_agents() {
	// The executing agent pins a model only the delegated agent `b` uses.
	let mut scope = Fixture::new();
	scope.restricted_ids.insert("first".into());
	scope.restriction_admits.insert("agent:b".into());
	let refused = entry(&mut scope, &reference("first"), "registry.read").await;
	assert!(matches!(refused, Err(Error::Forbidden)));
	assert_eq!(scope.restriction_checks, ["first:user-of:first"]);
	scope.calls.clear();
	let admitted = closure_entry(
		&mut scope,
		&reference("first"),
		"registry.read",
		&[reference("b")],
	)
	.await
	.unwrap();
	assert_eq!(admitted.id, "first");
	assert!(
		scope.restriction_checks.last() == Some(&"first:agent:b".into())
			&& scope.calls.contains(&"remember:first".into()),
		"{:?} {:?}",
		scope.restriction_checks,
		scope.calls
	);
	// Every nearest agent must be admitted.
	let refused = closure_entry(
		&mut scope,
		&reference("second"),
		"registry.read",
		&[reference("b"), reference("c")],
	)
	.await;
	assert!(refused.is_ok(), "second is unrestricted");
	scope.restricted_ids.insert("second".into());
	let refused = closure_entry(
		&mut scope,
		&reference("second"),
		"registry.read",
		&[reference("b"), reference("c")],
	)
	.await;
	assert!(matches!(refused, Err(Error::Forbidden)));
}
