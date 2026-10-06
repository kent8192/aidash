use super::*;
use aidash_domain::{
	RecoveryState, Run, RunControl, RunState, StateVersion, ThinkingState, context::Context,
	federation::Peer, policy::Resource, registry::Entry,
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::{
	collections::{BTreeMap, VecDeque},
	sync::Mutex,
	time::Instant,
};
use uuid::Uuid;

#[derive(Clone, Copy)]
enum Fault {
	Forbidden,
	Unauthorized,
	Missing,
	Semantic,
	Database,
}
impl Fault {
	fn error(self) -> Error {
		match self {
			Self::Forbidden => Error::Forbidden,
			Self::Unauthorized => Error::Unauthorized,
			Self::Missing => Error::NotFound("admission".into()),
			Self::Semantic => Error::RemoteSemantic(aidash_domain::semantic::Failure::Unavailable),
			Self::Database => Error::Port(Box::new(DatabaseFailure)),
		}
	}
}
#[derive(Debug, thiserror::Error)]
#[error("database lock failed")]
struct DatabaseFailure;
struct Scope {
	calls: Vec<String>,
	started: Instant,
	elapsed: Duration,
	latency: Duration,
	frontier: Vec<Reference>,
	edges: BTreeMap<Uuid, Vec<Reference>>,
	decision: bool,
	peer: Option<Peer>,
	fault: Option<Fault>,
	admission: Option<Run>,
	bound: Option<Uuid>,
	entry: Entry,
}
#[async_trait]
impl DependencyScope for Scope {
	fn node(&self) -> &str {
		"aidash://local"
	}
	fn identity(&self) -> (&str, &str) {
		("mapped-tenant", "mapped-reader")
	}
	fn now(&self) -> Instant {
		self.started + self.elapsed
	}
	fn start_frontier(&mut self) {
		self.frontier.clear();
	}
	fn take_frontier(&mut self) -> Vec<Reference> {
		std::mem::take(&mut self.frontier)
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "mapped-tenant".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	fn catalog_resource(&self, entry: &Entry) -> Resource {
		self.resource(&entry.kind, &entry.id, json!({"version":entry.version}))
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		assert_eq!(resource.tenant, "mapped-tenant");
		self.calls.push(format!("decide:{action}:{}", resource.id));
		if resource.id == "aidash://foreign" {
			assert_eq!(
				resource.attributes,
				json!({"remote_node":"aidash://foreign"})
			);
		}
		Ok(self.decision)
	}
	async fn admission(&mut self, id: Uuid, home: &str) -> Result<Option<Run>> {
		self.calls.push(format!("admission:{id}:{home}"));
		Ok(self.admission.clone())
	}
	async fn bound_grant(&mut self, admission: Uuid) -> Result<Option<Uuid>> {
		self.calls.push(format!("binding:{admission}"));
		Ok(self.bound)
	}
	async fn run_visible(&mut self, run: &Run) -> Result<bool> {
		self.calls.push(format!("run:{}", run.id));
		match self.fault {
			Some(fault) => Err(fault.error()),
			None => Ok(true),
		}
	}
	async fn grant_visible(
		&mut self,
		execution: &str,
		grant: Uuid,
		admission: Uuid,
	) -> Result<bool> {
		assert_eq!(execution, "aidash://executor");
		assert_eq!(admission, Uuid::from_u128(10000));
		self.calls.push(format!("grant:{grant}"));
		self.frontier
			.extend(self.edges.get(&grant).cloned().unwrap_or_default());
		self.elapsed += self.latency;
		match self.fault {
			Some(fault) => Err(fault.error()),
			None => Ok(true),
		}
	}
	async fn catalog_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		assert_eq!(reference.id, "agent");
		assert_eq!(reference.version, "1.0.0");
		assert_eq!(action, "registry.read");
		self.calls.push("catalog".into());
		match self.fault {
			Some(fault) => Err(fault.error()),
			None => Ok(self.entry.clone()),
		}
	}
	async fn peer(&mut self, node: &str) -> Result<Option<Peer>> {
		self.calls.push(format!("peer:{node}"));
		Ok(self.peer.clone())
	}
}
struct Transport {
	calls: Mutex<Vec<(String, Value, Duration)>>,
	replies: Mutex<VecDeque<Option<Checked>>>,
}
#[async_trait]
impl DependencyTransport for Transport {
	async fn check(&self, peer: &Peer, input: &Input, remaining: Duration) -> Option<Checked> {
		self.calls.lock().unwrap().push((
			peer.node_id.clone(),
			serde_json::to_value(input).unwrap(),
			remaining,
		));
		self.replies.lock().unwrap().pop_front().unwrap_or(None)
	}
}
fn grant(node: &str, id: u128) -> Reference {
	Reference::Grant {
		node_id: node.into(),
		execution_node: "aidash://executor".into(),
		grant_id: Uuid::from_u128(id),
		admission_id: Uuid::from_u128(10000),
	}
}
#[fixture]
fn scope() -> Scope {
	Scope { calls:vec![],started:Instant::now(),elapsed:Duration::ZERO,latency:Duration::ZERO,frontier:vec![],edges:BTreeMap::new(),decision:true,fault:None,admission:None,bound:None,
        entry:serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{},"config":{"model":{"id":"model","version":"1.0.0"}}})).unwrap(),
        peer:Some(Peer {node_id:"aidash://foreign".into(),endpoint:"https://foreign.invalid".into(),credential_env:"FIXTURE".into(),protocol_version:"0.1".into(),enabled:true}),
    }
}
#[fixture]
fn transport() -> Transport {
	Transport {
		calls: Mutex::new(vec![]),
		replies: Mutex::new(VecDeque::new()),
	}
}

#[rstest]
#[tokio::test]
async fn cycles_are_checked_once_but_each_nodes_authority_is_checked(
	mut scope: Scope,
	transport: Transport,
) {
	// Arrange: the same grant ID on another node is a separate authority edge.
	let local = grant("aidash://local", 1);
	let foreign = grant("aidash://foreign", 1);
	scope
		.edges
		.insert(Uuid::from_u128(1), vec![foreign.clone(), local.clone()]);
	transport.replies.lock().unwrap().push_back(Some(Checked {
		visible: true,
		pending: vec![local.clone(), foreign.clone()],
	}));
	// Act
	assert!(
		verify_all(&mut scope, &transport, "0.1", vec![local])
			.await
			.unwrap()
	);
	// Assert
	assert_eq!(
		scope.calls,
		vec![
			format!("grant:{}", Uuid::from_u128(1)),
			"decide:federation.discover:aidash://foreign".into(),
			"peer:aidash://foreign".into()
		]
	);
	let calls = transport.calls.lock().unwrap();
	assert_eq!(calls.len(), 1);
	assert_eq!(
		calls[0].1,
		json!({"tenant":"mapped-tenant","subject":"mapped-reader","reference":foreign})
	);
	assert_eq!(calls[0].2, Duration::from_secs(20));
}

#[rstest]
#[case(false, true, "0.1", 1)]
#[case(true, false, "0.1", 2)]
#[case(true, true, "unsupported", 2)]
#[tokio::test]
async fn remote_policy_peer_and_protocol_gate_precede_transport(
	mut scope: Scope,
	transport: Transport,
	#[case] allowed: bool,
	#[case] enabled: bool,
	#[case] protocol: &str,
	#[case] calls: usize,
) {
	scope.decision = allowed;
	if enabled {
		scope.peer.as_mut().unwrap().protocol_version = protocol.into();
	} else {
		scope.peer = None;
	}
	assert!(
		!verify_all(
			&mut scope,
			&transport,
			"0.1",
			vec![grant("aidash://foreign", 1)]
		)
		.await
		.unwrap()
	);
	assert_eq!(scope.calls.len(), calls);
	assert!(transport.calls.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn deadline_expiration_prevents_the_next_authority_call(
	mut scope: Scope,
	transport: Transport,
) {
	scope.latency = Duration::from_secs(20);
	scope
		.edges
		.insert(Uuid::from_u128(1), vec![grant("aidash://foreign", 2)]);
	assert!(
		!verify_all(
			&mut scope,
			&transport,
			"0.1",
			vec![grant("aidash://local", 1)]
		)
		.await
		.unwrap()
	);
	assert_eq!(scope.calls, vec![format!("grant:{}", Uuid::from_u128(1))]);
	assert!(transport.calls.lock().unwrap().is_empty());
}

#[rstest]
#[case(LIMIT, true, LIMIT)]
#[case(LIMIT+1,false,LIMIT)]
#[case(LIMIT+2,false,0)]
#[tokio::test]
async fn unique_graph_size_and_pending_bounds_are_retained(
	mut scope: Scope,
	transport: Transport,
	#[case] count: usize,
	#[case] visible: bool,
	#[case] calls: usize,
) {
	let edges = (1..=count)
		.map(|id| grant("aidash://local", id as u128))
		.collect();
	assert_eq!(
		verify_all(&mut scope, &transport, "0.1", edges)
			.await
			.unwrap(),
		visible
	);
	assert_eq!(scope.calls.len(), calls);
}

#[rstest]
#[case(false, 1)]
#[case(true,LIMIT+1)]
#[tokio::test]
async fn rejected_or_oversized_peer_frontiers_prevent_followup_calls(
	mut scope: Scope,
	transport: Transport,
	#[case] visible: bool,
	#[case] count: usize,
) {
	transport.replies.lock().unwrap().push_back(Some(Checked {
		visible,
		pending: vec![grant("aidash://local", 1); count],
	}));
	assert!(
		!verify_all(
			&mut scope,
			&transport,
			"0.1",
			vec![grant("aidash://foreign", 1)]
		)
		.await
		.unwrap()
	);
	assert_eq!(scope.calls.len(), 2);
	assert_eq!(transport.calls.lock().unwrap().len(), 1);
}

#[rstest]
#[tokio::test]
async fn unavailable_peer_transport_denies_visibility(mut scope: Scope, transport: Transport) {
	assert!(
		!verify_all(
			&mut scope,
			&transport,
			"0.1",
			vec![grant("aidash://foreign", 1)]
		)
		.await
		.unwrap()
	);
	assert_eq!(scope.calls.len(), 2);
	assert_eq!(transport.calls.lock().unwrap().len(), 1);
}

#[rstest]
#[case(Fault::Forbidden)]
#[case(Fault::Unauthorized)]
#[case(Fault::Missing)]
#[case(Fault::Semantic)]
#[tokio::test]
async fn unavailable_local_grant_authority_discloses_no_pending_edges(
	mut scope: Scope,
	#[case] fault: Fault,
) {
	scope.fault = Some(fault);
	scope
		.edges
		.insert(Uuid::from_u128(1), vec![grant("aidash://foreign", 2)]);
	let result = verify_peer(&mut scope, &grant("aidash://local", 1))
		.await
		.unwrap();
	assert!(!result.visible);
	assert!(result.pending.is_empty());
}

#[rstest]
#[tokio::test]
async fn adapter_failures_keep_their_identity_for_recovery(mut scope: Scope) {
	scope.fault = Some(Fault::Database);
	let Error::Port(error) = check_local(&mut scope, &grant("aidash://local", 1))
		.await
		.unwrap_err()
	else {
		panic!("database failure was reclassified")
	};
	assert!(error.downcast_ref::<DatabaseFailure>().is_some());
}

#[rstest]
#[tokio::test]
async fn another_nodes_local_reference_never_reads_this_database(mut scope: Scope) {
	let foreign = grant("aidash://foreign", 1);
	assert!(matches!(
		require_local_target(&foreign, "aidash://local"),
		Err(Error::Forbidden)
	));
	assert!(!check_local(&mut scope, &foreign).await.unwrap());
	assert!(scope.calls.is_empty());
}

#[rstest]
#[case("agent", true, true, 3)]
#[case("model", true, false, 2)]
#[case("agent", false, false, 2)]
#[tokio::test]
async fn registry_dependency_keeps_kind_digest_and_execution_authority_gates(
	mut scope: Scope,
	#[case] kind: &str,
	#[case] digest_matches: bool,
	#[case] visible: bool,
	#[case] calls: usize,
) {
	scope.entry.kind = kind.into();
	let expected = if digest_matches {
		digest(&serde_json::to_value(&scope.entry).unwrap())
	} else {
		"changed".into()
	};
	let reference = Reference::Registry {
		node_id: "aidash://local".into(),
		id: "agent".into(),
		version: "1.0.0".into(),
		digest: expected,
	};
	assert_eq!(check_local(&mut scope, &reference).await.unwrap(), visible);
	assert_eq!(scope.calls.len(), calls);
	assert_eq!(scope.calls[0], "decide:federation.discover:aidash://local");
	if visible {
		assert_eq!(scope.calls[2], "decide:agent.execute:agent");
	}
}

#[rstest]
#[tokio::test]
async fn registry_catalog_denial_keeps_the_existing_error_boundary(mut scope: Scope) {
	scope.fault = Some(Fault::Forbidden);
	let reference = Reference::Registry {
		node_id: "aidash://local".into(),
		id: "agent".into(),
		version: "1.0.0".into(),
		digest: "approved".into(),
	};
	assert!(matches!(
		check_local(&mut scope, &reference).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls.len(), 2);
}

#[rstest]
#[case(false, false, 1)]
#[case(true, false, 2)]
#[case(true, true, 3)]
#[tokio::test]
async fn admission_must_exist_and_match_its_bound_grant_before_visibility(
	mut scope: Scope,
	#[case] present: bool,
	#[case] bound: bool,
	#[case] calls: usize,
) {
	let id = Uuid::from_u128(10000);
	if present {
		scope.admission = Some(Run {
			id,
			task_id: Uuid::from_u128(10),
			workspace_id: Uuid::from_u128(11),
			home_node: "aidash://home".into(),
			agent_id: "agent".into(),
			agent_version: "1.0.0".into(),
			state_version: StateVersion::default(),
			state: RunState::Thinking(ThinkingState::default()),
			recovery: RecoveryState::default(),
			control: RunControl::Active,
			context: Context::default(),
			step: 0,
			revision: 0,
			observed_input_seq: 0,
			ledger_worker_ready: true,
			error: None,
			lease_owner: None,
			lease_until: None,
			updated_at: "2026-10-03T00:00:00Z".parse().unwrap(),
		});
	}
	scope.bound = bound.then_some(Uuid::from_u128(1));
	let reference = Reference::Admission {
		node_id: "aidash://local".into(),
		home_node: "aidash://home".into(),
		grant_id: Uuid::from_u128(1),
		admission_id: id,
	};
	assert_eq!(
		check_local(&mut scope, &reference).await.unwrap(),
		present && bound
	);
	assert_eq!(scope.calls.len(), calls);
}
