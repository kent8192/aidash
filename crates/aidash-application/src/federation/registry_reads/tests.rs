use super::*;
use crate::ports::federation::registry_reads::RegistryJournalRow;
use aidash_domain::{federation::Peer, policy::Resource};
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
	)
}
struct Scope {
	rows: Vec<RegistryJournalRow>,
	cache: BTreeMap<(Uuid, u8), bool>,
	authority: u8,
	frontier: Option<Vec<Dependency>>,
	unavailable: BTreeSet<String>,
	missing_peer: bool,
	protocol: String,
	denied: Option<&'static str>,
	fail: Option<&'static str>,
	entry_error: Option<&'static str>,
	calls: Vec<String>,
	decisions: Vec<(Resource, String)>,
	entries: BTreeMap<String, Entry>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		rows: vec![row("aidash://leaf", "agent")],
		cache: BTreeMap::new(),
		authority: 0,
		frontier: None,
		unavailable: BTreeSet::new(),
		missing_peer: false,
		protocol: "0.1".into(),
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
	fn protocol(&self) -> &str {
		"0.1"
	}
	fn identity(&self) -> (&str, &str) {
		("tenant", "reader")
	}
	fn cached(&self, run: Uuid) -> Option<bool> {
		self.frontier
			.is_none()
			.then(|| self.cache.get(&(run, self.authority)).copied())
			.flatten()
	}
	fn remember(&mut self, run: Uuid, visible: bool) {
		if self.frontier.is_none() {
			self.calls.push(format!("remember:{visible}"));
			self.cache.insert((run, self.authority), visible);
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
	async fn peer(&mut self, node: &str) -> Result<Option<Peer>> {
		self.touch("peer")?;
		self.calls.push(node.into());
		Ok((!self.missing_peer).then(|| Peer {
			node_id: node.into(),
			endpoint: "https://fixture.invalid".into(),
			credential_env: "FIXTURE_KEY".into(),
			protocol_version: self.protocol.clone(),
			enabled: true,
		}))
	}
}
#[async_trait]
impl RegistryVerificationScope for Scope {
	fn node(&self) -> &str {
		"aidash://local"
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
	async fn verify(
		&self,
		peer: &Peer,
		tenant: &str,
		subject: &str,
		references: &[Reference],
	) -> Verification {
		let mut calls = self.calls.lock().unwrap();
		let response = self
			.replies
			.get(calls.len())
			.copied()
			.unwrap_or(Verification::Verified);
		calls.push((
			peer.node_id.clone(),
			tenant.into(),
			subject.into(),
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
			"agent.execute",
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
#[case::execute("agent.execute",vec!["dependencies","registry.read","agent.execute","remember:false"])]
#[case::discover("federation.discover",vec!["dependencies","registry.read","agent.execute","federation.discover","remember:false"])]
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
	assert_eq!(
		scope.calls,
		vec!["dependencies", "registry.read", "agent.execute"]
	);
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
#[case::missing(true, "0.1")]
#[case::protocol(false, "other")]
#[tokio::test]
async fn missing_or_incompatible_enabled_peer_denies_before_rpc(
	mut scope: Scope,
	#[case] missing: bool,
	#[case] protocol: &str,
) {
	scope.missing_peer = missing;
	scope.protocol = protocol.into();
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
			"agent.execute",
			"remember:false"
		]
	);
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
	assert!(scope.cache[&(run, 0)]);
	assert!(!scope.cache[&(run, 1)]);
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
#[case::execute("agent.execute")]
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
		.map(|(_, id, version, hash, _)| Reference {
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
async fn inbound_proof_requires_discovery_then_exact_current_definition_and_execution(
	mut scope: Scope,
) {
	let references = references(&scope);
	assert!(verify(&mut scope, &references).await.unwrap());
	assert_eq!(scope.calls, vec!["require", "entry", "agent.execute"]);
	assert_eq!(scope.decisions[0].0.id, "aidash://local");
	assert_eq!(scope.decisions[0].1, "federation.discover");
}
#[rstest]
#[case::forbidden("forbidden")]
#[case::missing("missing")]
#[tokio::test]
async fn inbound_catalog_denials_return_false_before_execution(
	mut scope: Scope,
	#[case] error: &'static str,
) {
	scope.entry_error = Some(error);
	let references = references(&scope);
	assert!(!verify(&mut scope, &references).await.unwrap());
	assert_eq!(scope.calls, vec!["require", "entry"]);
}
#[rstest]
#[case::kind(true)]
#[case::digest(false)]
#[tokio::test]
async fn inbound_definition_mismatch_does_not_evaluate_execution(
	mut scope: Scope,
	#[case] kind: bool,
) {
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
#[case::discover("federation.discover", true)]
#[case::execute("agent.execute", false)]
#[tokio::test]
async fn inbound_denial_retains_its_outer_error_or_boolean_contract(
	mut scope: Scope,
	#[case] action: &'static str,
	#[case] outer: bool,
) {
	scope.denied = Some(action);
	let references = references(&scope);
	let result = verify(&mut scope, &references).await;
	if outer {
		assert!(matches!(result, Err(Error::Forbidden)));
		assert_eq!(scope.calls, vec!["require"]);
	} else {
		assert!(!result.unwrap());
		assert_eq!(scope.calls, vec!["require", "entry", "agent.execute"]);
	}
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
