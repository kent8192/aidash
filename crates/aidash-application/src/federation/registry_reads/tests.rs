use super::*;
use crate::ports::federation::registry_reads::RegistryJournalRow;
use aidash_domain::{federation::route::Route, policy::Resource};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::{
	collections::{BTreeMap, BTreeSet},
	sync::Mutex,
};
fn definition(id: &str) -> Entry {
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":"agent","name":{"en":id},"description":{"en":"Frozen agent"},"config":{}})).unwrap()
}
fn row(node: &str, id: &str) -> RegistryJournalRow {
	let metadata = serde_json::to_value(definition(id)).unwrap();
	(
		node.into(),
		id.into(),
		"1.0.0".into(),
		digest(&metadata),
		metadata,
		false,
	)
}
fn tenant_row(node: &str, id: &str) -> RegistryJournalRow {
	let mut row = row(node, id);
	row.5 = true;
	row
}
struct Scope {
	rows: Vec<RegistryJournalRow>,
	cache: BTreeMap<(Uuid, u8), RemoteReads>,
	authority: u8,
	frontier: Option<Vec<Dependency>>,
	unavailable: BTreeSet<String>,
	row_silence: Option<String>,
	/// Nodes a check stopped at without asking, one entry per stop (#770).
	skipped: Vec<String>,
	answered: Vec<String>,
	missing_peer: bool,
	denied: Option<&'static str>,
	fail: Option<&'static str>,
	entry_error: Option<&'static str>,
	calls: Vec<String>,
	decisions: Vec<(Resource, String)>,
	entries: BTreeMap<String, Entry>,
}
impl crate::ports::authorization::visibility::RowOutcome for Scope {
	fn row_silence(&self) -> Option<String> {
		self.row_silence.clone()
	}
	fn take_row_silence(&mut self) -> Option<String> {
		self.row_silence.take()
	}
	fn set_row_silence(&mut self, node: Option<String>) {
		self.row_silence = node;
	}
}
#[fixture]
fn scope() -> Scope {
	Scope {
		rows: vec![row("aidash://leaf", "agent")],
		cache: BTreeMap::new(),
		authority: 0,
		frontier: None,
		unavailable: BTreeSet::new(),
		row_silence: None,
		skipped: vec![],
		answered: vec![],
		missing_peer: false,
		denied: None,
		fail: None,
		entry_error: None,
		calls: vec![],
		decisions: vec![],
		entries: [("agent".into(), definition("agent"))].into(),
	}
}
impl Scope {
	fn touch(&mut self, name: &str) -> Result<()> {
		self.calls.push(name.into());
		if self.fail == Some(name) {
			Err(Error::Port(Box::new(std::io::Error::other(
				"registry adapter fault",
			))))
		} else {
			Ok(())
		}
	}
	fn resource_for(kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
}
#[async_trait]
impl RegistryReadScope for Scope {
	fn identity(&self) -> (&str, &str) {
		("tenant", "reader")
	}
	fn cached(&self, run: Uuid) -> Option<RemoteReads> {
		self.frontier
			.is_none()
			.then(|| self.cache.get(&(run, self.authority)).cloned())
			.flatten()
	}
	fn remember(&mut self, run: Uuid, reads: RemoteReads) {
		if self.frontier.is_none() {
			let visible = reads == RemoteReads::Visible;
			self.calls.push(format!("remember:{visible}"));
			self.cache.insert((run, self.authority), reads);
		}
	}
	fn frontier(&mut self) -> Option<&mut Vec<Dependency>> {
		self.frontier.as_mut()
	}
	fn unavailable(&self, node: &str) -> bool {
		self.unavailable.contains(node)
	}
	fn mark_unavailable(&mut self, node: &str) {
		self.calls.push(format!("unavailable:{node}"));
		self.unavailable.insert(node.into());
		self.row_silence.get_or_insert_with(|| node.into());
	}
	fn skipped(&mut self, node: &str) {
		self.calls.push(format!("skipped:{node}"));
		self.skipped.push(node.into());
		self.row_silence.get_or_insert_with(|| node.into());
	}
	fn answered(&mut self, node: &str) {
		self.answered.push(node.into());
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Self::resource_for(kind, id, attributes)
	}
	fn catalog_resource(&self, entry: &Entry) -> Resource {
		Self::resource_for(
			&entry.kind,
			&entry.id,
			crate::authorization::catalog::attributes(entry),
		)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.touch(action)?;
		self.decisions.push((resource.clone(), action.into()));
		Ok(self.denied != Some(action))
	}
	async fn dependencies(&mut self, _: Uuid) -> Result<Vec<RegistryJournalRow>> {
		self.touch("dependencies")?;
		Ok(self.rows.clone())
	}
	async fn route(&mut self, node: &str) -> Result<Option<Route>> {
		self.touch("peer")?;
		self.calls.push(node.into());
		Ok((!self.missing_peer).then(|| {
			let mut route = crate::test_support::foreign_route(node);
			route.subject = Some("reader".into());
			route
		}))
	}
}
#[async_trait]
impl RegistryVerificationScope for Scope {
	async fn node_resource(&mut self) -> Result<Resource> {
		Ok(Self::resource_for(
			"node",
			"brave-otter-k3m9q2xa",
			json!({"label":"Default","default":true}),
		))
	}
	fn catalog_resource(&self, entry: &Entry) -> Resource {
		Self::resource_for(
			&entry.kind,
			&entry.id,
			crate::authorization::catalog::attributes(entry),
		)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.touch("require")?;
		self.decisions.push((resource.clone(), action.into()));
		if self.denied == Some(action) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		RegistryReadScope::decide(self, resource, action).await
	}
	async fn entry(&mut self, reference: &EntityRef) -> Result<Entry> {
		self.touch("entry")?;
		match self.entry_error {
			Some("forbidden") => Err(Error::Forbidden),
			Some("missing") => Err(Error::NotFound("definition".into())),
			Some("opaque") => Err(Error::Port(Box::new(std::io::Error::other(
				"catalog fault",
			)))),
			_ => Ok(self.entries[&reference.id].clone()),
		}
	}
}
#[derive(Default)]
struct Transport {
	calls: Mutex<Vec<VerificationCall>>,
	replies: Vec<Verification>,
}
type VerificationCall = (String, String, String, Vec<Reference>);
#[async_trait]
impl RegistryTransport for Transport {
	async fn verify(&self, route: &Route, references: &[Reference]) -> Verification {
		let mut calls = self.calls.lock().unwrap();
		let response = self
			.replies
			.get(calls.len())
			.copied()
			.unwrap_or(Verification::Verified);
		calls.push((
			route.target.deployment.to_string(),
			route.tenant.to_string(),
			route.subject.clone().unwrap_or_default(),
			references.to_vec(),
		));
		response
	}
}
#[rstest]
#[tokio::test]
async fn qualified_metadata_policies_and_peer_proof_precede_the_cached_decision(mut scope: Scope) {
	let transport = Transport::default();
	let run = Uuid::from_u128(1);
	assert!(reads_visible(&mut scope, &transport, run).await.unwrap());
	assert_eq!(
		scope.calls,
		vec![
			"dependencies",
			"registry.read",
			"federation.discover",
			"peer",
			"aidash://leaf",
			"remember:true"
		]
	);
	assert_eq!(scope.decisions[0].0.id, "aidash://leaf/agents/agent@1.0.0");
	assert_eq!(
		scope.decisions[0].0.attributes["remote_node"],
		"aidash://leaf"
	);
	{
		let calls = transport.calls.lock().unwrap();
		assert_eq!(calls.len(), 1);
		assert_eq!(
			(&calls[0].1, &calls[0].2),
			(&"tenant".to_owned(), &"reader".to_owned())
		);
		assert_eq!(calls[0].3[0].digest, scope.rows[0].3);
	}
	scope.calls.clear();
	assert!(reads_visible(&mut scope, &transport, run).await.unwrap());
	assert!(scope.calls.is_empty());
	assert_eq!(transport.calls.lock().unwrap().len(), 1);
}
#[rstest]
#[case::id(0)]
#[case::version(1)]
#[case::kind(2)]
#[case::digest(3)]
#[tokio::test]
async fn metadata_identity_kind_and_digest_must_match_before_any_policy(
	mut scope: Scope,
	#[case] mismatch: u8,
) {
	match mismatch {
		0 => scope.rows[0].1 = "other".into(),
		1 => scope.rows[0].2 = "2.0.0".into(),
		2 => {
			scope.rows[0].4["kind"] = json!("skill");
			scope.rows[0].3 = digest(&scope.rows[0].4);
		}
		_ => scope.rows[0].3 = "different".into(),
	};
	let transport = Transport::default();
	assert!(
		!reads_visible(&mut scope, &transport, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert_eq!(scope.calls, vec!["dependencies", "remember:false"]);
	assert!(transport.calls.lock().unwrap().is_empty());
}
#[rstest]
#[tokio::test]
async fn malformed_saved_metadata_is_an_error_and_is_not_cached(mut scope: Scope) {
	scope.rows[0].4 = json!({"bad":true});
	assert!(matches!(
		reads_visible(&mut scope, &Transport::default(), Uuid::from_u128(1)).await,
		Err(Error::Json(_))
	));
	assert_eq!(scope.calls, vec!["dependencies"]);
	assert!(scope.cache.is_empty());
}
#[rstest]
#[case::registry("registry.read",vec!["dependencies","registry.read","remember:false"])]
#[case::discover("federation.discover",vec!["dependencies","registry.read","federation.discover","remember:false"])]
#[tokio::test]
async fn every_local_policy_must_pass_before_peer_io(
	mut scope: Scope,
	#[case] denied: &'static str,
	#[case] calls: Vec<&str>,
) {
	scope.denied = Some(denied);
	let transport = Transport::default();
	assert!(
		!reads_visible(&mut scope, &transport, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert_eq!(scope.calls, calls);
	assert!(transport.calls.lock().unwrap().is_empty());
}
#[rstest]
#[case::maximum(255, true)]
#[case::overflow(256, false)]
#[tokio::test]
async fn frontier_appends_each_edge_without_peer_io_or_a_final_cache(
	mut scope: Scope,
	#[case] already: usize,
	#[case] allowed: bool,
) {
	let pending = Dependency::Registry {
		node_id: "aidash://other".into(),
		id: "prior".into(),
		version: "1".into(),
		digest: "prior".into(),
	};
	scope.frontier = Some(vec![pending; already]);
	let transport = Transport::default();
	assert_eq!(
		reads_visible(&mut scope, &transport, Uuid::from_u128(1))
			.await
			.unwrap(),
		allowed
	);
	assert_eq!(scope.frontier.as_ref().unwrap().len(), already + 1);
	assert_eq!(
		scope.frontier.as_ref().unwrap().last(),
		Some(&Dependency::Registry {
			node_id: "aidash://leaf".into(),
			id: "agent".into(),
			version: "1.0.0".into(),
			digest: scope.rows[0].3.clone()
		})
	);
	assert_eq!(scope.calls, vec!["dependencies", "registry.read"]);
	assert!(scope.cache.is_empty());
	assert!(transport.calls.lock().unwrap().is_empty());
}
#[rstest]
#[tokio::test]
async fn frontier_rechecks_local_authority_after_a_previous_success(mut scope: Scope) {
	scope.frontier = Some(vec![]);
	let run = Uuid::from_u128(1);
	let transport = Transport::default();
	assert!(reads_visible(&mut scope, &transport, run).await.unwrap());
	scope.denied = Some("registry.read");
	scope.calls.clear();
	assert!(!reads_visible(&mut scope, &transport, run).await.unwrap());
	assert_eq!(scope.calls, vec!["dependencies", "registry.read"]);
	assert!(scope.cache.is_empty());
}
#[rstest]
#[case::missing(true)]
#[tokio::test]
async fn a_missing_route_denies_before_rpc(mut scope: Scope, #[case] missing: bool) {
	scope.missing_peer = missing;
	let transport = Transport::default();
	assert!(
		!reads_visible(&mut scope, &transport, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert!(transport.calls.lock().unwrap().is_empty());
	assert!(scope.unavailable.is_empty());
}
#[rstest]
#[case::denied(Verification::Rejected, false)]
#[case::transport(Verification::Unavailable, true)]
#[case::not_asked(Verification::NotAsked, false)]
#[tokio::test]
async fn transport_unavailability_is_distinct_from_a_negative_proof(
	mut scope: Scope,
	#[case] reply: Verification,
	#[case] unavailable: bool,
) {
	let transport = Transport {
		replies: vec![reply],
		..Default::default()
	};
	assert!(
		!reads_visible(&mut scope, &transport, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert_eq!(scope.unavailable.contains("aidash://leaf"), unavailable);
	assert_eq!(transport.calls.lock().unwrap().len(), 1);
}
#[rstest]
#[case::refused(Verification::Rejected, 1)]
#[case::confirmed(Verification::Verified, 1)]
#[case::silent(Verification::Unavailable, 0)]
#[tokio::test]
async fn only_a_node_that_answered_is_reported_as_answering(
	mut scope: Scope,
	#[case] reply: Verification,
	#[case] answers: usize,
) {
	let transport = Transport {
		replies: vec![reply],
		..Default::default()
	};
	let _ = reads_visible(&mut scope, &transport, Uuid::from_u128(1)).await;
	assert_eq!(scope.answered.len(), answers);
	assert!(scope.skipped.is_empty());
}
#[rstest]
#[tokio::test]
async fn a_node_already_known_silent_is_reported_as_the_one_the_check_stopped_at(mut scope: Scope) {
	scope.unavailable.insert("aidash://leaf".into());
	let transport = Transport::default();
	assert!(
		!reads_visible(&mut scope, &transport, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert_eq!(scope.skipped, ["aidash://leaf"]);
	assert!(transport.calls.lock().unwrap().is_empty());
}
#[rstest]
#[tokio::test]
async fn unavailable_peer_still_requires_saved_metadata_policy(mut scope: Scope) {
	scope.unavailable.insert("aidash://leaf".into());
	let transport = Transport::default();
	assert!(
		!reads_visible(&mut scope, &transport, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert_eq!(
		scope.calls,
		vec![
			"dependencies",
			"registry.read",
			"federation.discover",
			"skipped:aidash://leaf",
			"remember:false"
		]
	);
	assert!(transport.calls.lock().unwrap().is_empty());
}
#[rstest]
#[tokio::test]
async fn a_mark_does_not_stand_in_for_this_readers_discover_right(mut scope: Scope) {
	scope.unavailable.insert("aidash://leaf".into());
	scope.denied = Some("federation.discover");
	let transport = Transport::default();
	assert!(
		!reads_visible(&mut scope, &transport, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert_eq!(
		scope.calls.last().map(String::as_str),
		Some("remember:false")
	);
	assert!(scope.calls.iter().any(|call| call == "federation.discover"));
	assert!(transport.calls.lock().unwrap().is_empty());
}
#[rstest]
#[tokio::test]
async fn changed_authority_does_not_reuse_the_prior_true_cache(mut scope: Scope) {
	let transport = Transport::default();
	let run = Uuid::from_u128(1);
	assert!(reads_visible(&mut scope, &transport, run).await.unwrap());
	scope.authority = 1;
	scope.denied = Some("registry.read");
	scope.calls.clear();
	assert!(!reads_visible(&mut scope, &transport, run).await.unwrap());
	assert_eq!(
		scope.calls,
		vec!["dependencies", "registry.read", "remember:false"]
	);
	assert_eq!(scope.cache[&(run, 0)], RemoteReads::Visible);
	assert_eq!(scope.cache[&(run, 1)], RemoteReads::Unconfirmed);
}
#[rstest]
#[case::all(Verification::Verified, true)]
#[case::last_denied(Verification::Rejected, false)]
#[tokio::test]
async fn large_remote_journal_keeps_128_reference_batches_and_checks_every_batch(
	mut scope: Scope,
	#[case] last: Verification,
	#[case] allowed: bool,
) {
	scope.rows = (0..129)
		.map(|i| row("aidash://leaf", &format!("agent{i}")))
		.collect();
	let transport = Transport {
		replies: vec![Verification::Verified, last],
		..Default::default()
	};
	assert_eq!(
		reads_visible(&mut scope, &transport, Uuid::from_u128(1))
			.await
			.unwrap(),
		allowed
	);
	let calls = transport.calls.lock().unwrap();
	assert_eq!(
		calls.iter().map(|call| call.3.len()).collect::<Vec<_>>(),
		vec![128, 1]
	);
	assert_eq!(calls[1].3[0].entry.id, "agent128");
}
#[rstest]
#[tokio::test]
async fn node_order_is_deterministic_and_the_first_denial_stops_later_rpc(mut scope: Scope) {
	scope.rows = vec![row("aidash://z", "agent"), row("aidash://a", "agent")];
	let transport = Transport {
		replies: vec![Verification::Rejected],
		..Default::default()
	};
	assert!(
		!reads_visible(&mut scope, &transport, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	let calls = transport.calls.lock().unwrap();
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0].0, "aidash://a");
	assert_eq!(
		scope
			.decisions
			.iter()
			.filter(|(_, action)| action == "registry.read")
			.count(),
		2
	);
}
#[rstest]
#[case::journal("dependencies")]
#[case::registry("registry.read")]
#[case::discover("federation.discover")]
#[case::peer("peer")]
#[tokio::test]
async fn repository_or_policy_errors_keep_their_identity_without_cache(
	mut scope: Scope,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	let Error::Port(error) = reads_visible(&mut scope, &Transport::default(), Uuid::from_u128(1))
		.await
		.unwrap_err()
	else {
		panic!("expected opaque adapter error");
	};
	assert!(error.is::<std::io::Error>());
	assert!(scope.cache.is_empty());
	assert_eq!(scope.calls.last().unwrap(), fail);
}
fn references(scope: &Scope) -> Vec<Reference> {
	scope
		.rows
		.iter()
		.map(|(_, id, version, hash, _, _)| Reference {
			entry: EntityRef {
				id: id.clone(),
				version: version.clone(),
			},
			digest: hash.clone(),
		})
		.collect()
}
#[rstest]
#[tokio::test]
async fn inbound_proof_requires_discovery_then_exact_current_definition_without_execution(
	mut scope: Scope,
) {
	let references = references(&scope);
	assert!(verify(&mut scope, &references).await.unwrap());
	assert_eq!(scope.calls, vec!["require", "entry"]);
	assert_eq!(scope.decisions[0].0.id, "brave-otter-k3m9q2xa");
	assert_eq!(scope.decisions[0].1, "federation.discover");
}
#[rstest]
#[case::forbidden("forbidden")]
#[case::missing("missing")]
#[tokio::test]
async fn inbound_catalog_denials_return_false(mut scope: Scope, #[case] error: &'static str) {
	scope.entry_error = Some(error);
	let references = references(&scope);
	assert!(!verify(&mut scope, &references).await.unwrap());
	assert_eq!(scope.calls, vec!["require", "entry"]);
}
#[rstest]
#[case::kind(true)]
#[case::digest(false)]
#[tokio::test]
async fn inbound_definition_mismatch_is_not_proof(mut scope: Scope, #[case] kind: bool) {
	let references = references(&scope);
	if kind {
		scope.entries.get_mut("agent").unwrap().kind = "skill".into();
	} else {
		scope.entries.get_mut("agent").unwrap().config = json!({"changed":true});
	}
	assert!(!verify(&mut scope, &references).await.unwrap());
	assert_eq!(scope.calls, vec!["require", "entry"]);
}
#[rstest]
#[tokio::test]
async fn inbound_discovery_denial_is_an_outer_error_and_execution_is_not_asked(mut scope: Scope) {
	scope.denied = Some("federation.discover");
	let references = references(&scope);
	assert!(matches!(
		verify(&mut scope, &references).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["require"]);
}
#[rstest]
#[tokio::test]
async fn inbound_proof_does_not_depend_on_agent_execute(mut scope: Scope) {
	scope.denied = Some("agent.execute");
	let references = references(&scope);
	assert!(verify(&mut scope, &references).await.unwrap());
	assert!(
		!scope
			.decisions
			.iter()
			.any(|(_, action)| action == "agent.execute")
	);
}
#[rstest]
#[tokio::test]
async fn inbound_adapter_error_is_not_converted_to_a_missing_definition(mut scope: Scope) {
	scope.entry_error = Some("opaque");
	let references = references(&scope);
	let Error::Port(error) = verify(&mut scope, &references).await.unwrap_err() else {
		panic!("expected opaque adapter error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(scope.calls, vec!["require", "entry"]);
}

#[rstest]
#[case::empty(0)]
#[case::oversized(129)]
#[tokio::test]
async fn inbound_use_case_rejects_invalid_reference_counts_before_authority_io(
	mut scope: Scope,
	#[case] count: usize,
) {
	let input = vec![references(&scope)[0].clone(); count];
	assert!(matches!(
		verify(&mut scope, &input).await,
		Err(Error::Domain(aidash_domain::Error::Invalid(_)))
	));
	assert!(scope.calls.is_empty());
}

#[rstest]
#[tokio::test]
async fn a_same_tenant_node_is_decided_by_policy_alone_and_nobody_is_asked(mut scope: Scope) {
	scope.rows = vec![tenant_row("aidash://sibling", "agent")];
	let transport = Transport::default();
	assert!(
		reads_visible(&mut scope, &transport, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert_eq!(
		scope.calls,
		vec![
			"dependencies",
			"registry.read",
			"federation.discover",
			"remember:true"
		]
	);
	assert!(transport.calls.lock().unwrap().is_empty());
	let node = &scope.decisions[1];
	assert_eq!(
		(node.0.kind.as_str(), node.0.id.as_str()),
		("node", "aidash://sibling")
	);
}
#[rstest]
#[tokio::test]
async fn a_same_tenant_node_that_has_no_route_or_does_not_answer_hides_nothing(mut scope: Scope) {
	scope.rows = vec![tenant_row("aidash://sibling", "agent")];
	scope.missing_peer = true;
	scope.unavailable.insert("aidash://sibling".into());
	let transport = Transport {
		replies: vec![Verification::Unavailable],
		..Default::default()
	};
	assert!(
		reads_visible(&mut scope, &transport, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert!(!scope.calls.contains(&"peer".into()));
	assert!(transport.calls.lock().unwrap().is_empty());
}
#[rstest]
#[case::registry("registry.read", vec!["dependencies", "registry.read", "remember:false"])]
#[case::discover("federation.discover", vec!["dependencies", "registry.read", "federation.discover", "remember:false"])]
#[tokio::test]
async fn a_same_tenant_node_still_needs_both_policies(
	mut scope: Scope,
	#[case] denied: &'static str,
	#[case] calls: Vec<&str>,
) {
	scope.rows = vec![tenant_row("aidash://sibling", "agent")];
	scope.denied = Some(denied);
	assert!(
		!reads_visible(&mut scope, &Transport::default(), Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert_eq!(scope.calls, calls);
}
#[rstest]
#[tokio::test]
async fn a_same_tenant_agent_is_named_as_the_tenant_names_it(mut scope: Scope) {
	scope.rows = vec![tenant_row("aidash://sibling", "agent")];
	assert!(
		reads_visible(&mut scope, &Transport::default(), Uuid::from_u128(1))
			.await
			.unwrap()
	);
	let (resource, action) = &scope.decisions[0];
	assert_eq!(action, "registry.read");
	assert_eq!(
		(resource.kind.as_str(), resource.id.as_str()),
		("agent", "agent")
	);
	assert_eq!(resource.attributes["remote_node"], "aidash://sibling");
	assert_eq!(resource.attributes["version"], "1.0.0");
}
#[rstest]
#[tokio::test]
async fn a_denied_same_tenant_agent_is_a_definition_not_an_unconfirmed_read(mut scope: Scope) {
	scope.rows = vec![tenant_row("aidash://sibling", "agent")];
	scope.denied = Some("registry.read");
	assert_eq!(
		reads(&mut scope, &Transport::default(), Uuid::from_u128(1))
			.await
			.unwrap(),
		RemoteReads::Definition(EntityRef {
			id: "agent".into(),
			version: "1.0.0".into(),
		})
	);
	assert_eq!(
		scope.cache.values().next(),
		Some(&RemoteReads::Definition(EntityRef {
			id: "agent".into(),
			version: "1.0.0".into(),
		}))
	);
}
#[rstest]
#[case::registry("registry.read")]
#[case::discover("federation.discover")]
#[tokio::test]
async fn another_tenant_refusal_and_a_same_tenant_node_refusal_stay_unconfirmed(
	mut scope: Scope,
	#[case] denied: &'static str,
) {
	scope.denied = Some(denied);
	assert_eq!(
		reads(&mut scope, &Transport::default(), Uuid::from_u128(1))
			.await
			.unwrap(),
		RemoteReads::Unconfirmed
	);
	if denied == "federation.discover" {
		scope.rows = vec![tenant_row("aidash://sibling", "agent")];
		scope.cache.clear();
		assert_eq!(
			reads(&mut scope, &Transport::default(), Uuid::from_u128(1))
				.await
				.unwrap(),
			RemoteReads::Unconfirmed
		);
	}
}
#[test]
fn only_another_tenant_agent_is_named_by_its_node() {
	let local = Scope::resource_for("agent", "agent", json!({"version":"1.0.0"}));
	let own = remote_agent_resource(local.clone(), "aidash://sibling", "1.0.0", true);
	let foreign = remote_agent_resource(local, "aidash://leaf", "1.0.0", false);
	assert_eq!(own.id, "agent");
	assert_eq!(foreign.id, "aidash://leaf/agents/agent@1.0.0");
	assert_eq!(own.attributes["remote_node"], "aidash://sibling");
	assert_eq!(foreign.attributes["remote_node"], "aidash://leaf");
}
#[rstest]
#[tokio::test]
async fn a_same_tenant_node_adds_nothing_to_the_frontier(mut scope: Scope) {
	scope.rows = vec![
		tenant_row("aidash://sibling", "agent"),
		row("aidash://leaf", "other"),
	];
	scope.frontier = Some(vec![]);
	let _ = scope.entries.insert("other".into(), definition("other"));
	assert!(
		reads_visible(&mut scope, &Transport::default(), Uuid::from_u128(1))
			.await
			.unwrap()
	);
	let frontier = scope.frontier.as_ref().unwrap();
	assert_eq!(frontier.len(), 1);
	assert!(
		matches!(&frontier[0], Dependency::Registry { node_id, .. } if node_id == "aidash://leaf")
	);
}
#[rstest]
#[tokio::test]
async fn a_withdrawn_approval_is_invisible_to_the_remote_read_check(mut scope: Scope) {
	// Nothing in the check asks for a Catalog Approval or for `agent.execute`.
	scope.denied = Some("agent.execute");
	assert!(
		reads_visible(&mut scope, &Transport::default(), Uuid::from_u128(1))
			.await
			.unwrap()
	);
}

#[rstest]
#[tokio::test]
async fn a_silent_node_does_not_hide_a_refusal_by_another_registry_node(mut scope: Scope) {
	scope.rows = vec![row("aidash://a", "agent"), row("aidash://b", "other")];
	let transport = Transport {
		calls: Mutex::new(vec![]),
		replies: vec![Verification::Unavailable, Verification::Rejected],
	};
	assert_eq!(
		reads(&mut scope, &transport, Uuid::from_u128(1))
			.await
			.unwrap(),
		RemoteReads::Unconfirmed
	);
	assert_eq!(scope.row_silence, None);
	assert_eq!(transport.calls.lock().unwrap().len(), 2);
}
