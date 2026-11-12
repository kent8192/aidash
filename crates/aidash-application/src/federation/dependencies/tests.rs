use super::*;
use crate::ports::federation::dependencies::Check;
use aidash_domain::{
	RecoveryState, Run, RunControl, RunState, StateVersion, ThinkingState, context::Context,
	federation::route::Route, policy::Resource, registry::Entry,
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::{
	collections::{BTreeMap, BTreeSet, VecDeque},
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
	/// Time that asking a peer takes, charged when its route is resolved.
	latency_on_peer: Duration,
	frontier: Vec<Reference>,
	edges: BTreeMap<Uuid, Vec<Reference>>,
	decision: bool,
	denied_node: Option<String>,
	peer: Option<Route>,
	fault: Option<Fault>,
	admission: Option<Run>,
	bound: Option<Uuid>,
	entry: Entry,
	marked: BTreeSet<String>,
	/// Nodes a check stopped at without asking, one entry per stop (#770).
	skipped: Vec<String>,
	/// Nodes that answered, one entry per answer.
	answered: Vec<String>,
	/// Every Node a walk reported meeting as unavailable, in order, repeats included.
	met: Vec<String>,
	row_silence: Option<String>,
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
		Ok(self.decision && self.denied_node.as_deref() != Some(resource.id.as_str()))
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
	async fn readable_entry(&mut self, reference: &EntityRef) -> Result<Entry> {
		assert_eq!(reference.id, "agent");
		assert_eq!(reference.version, "1.0.0");
		self.calls.push("catalog".into());
		match self.fault {
			Some(fault) => Err(fault.error()),
			None => Ok(self.entry.clone()),
		}
	}
	async fn route(&mut self, node: &str) -> Result<Option<Route>> {
		self.calls.push(format!("peer:{node}"));
		self.elapsed += self.latency_on_peer;
		Ok(self.peer.clone())
	}
	fn unavailable(&self, node: &str) -> bool {
		self.marked.contains(node)
	}
	fn mark_unavailable(&mut self, node: &str) {
		self.marked.insert(node.into());
		self.row_silence.get_or_insert_with(|| node.into());
		self.met.push(node.into());
	}
	fn skipped(&mut self, node: &str) {
		self.marked.insert(node.into());
		self.row_silence.get_or_insert_with(|| node.into());
		self.skipped.push(node.into());
		self.met.push(node.into());
	}
	fn answered(&mut self, node: &str) {
		self.answered.push(node.into());
	}
}
struct Transport {
	calls: Mutex<Vec<(String, Value, Duration)>>,
	replies: Mutex<VecDeque<Check>>,
}
#[async_trait]
impl DependencyTransport for Transport {
	async fn check(&self, peer: &Route, input: &Input, remaining: Duration) -> Check {
		self.calls.lock().unwrap().push((
			peer.target.deployment.to_string(),
			serde_json::to_value(input).unwrap(),
			remaining,
		));
		self.replies
			.lock()
			.unwrap()
			.pop_front()
			.unwrap_or(Check::Unavailable)
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
	Scope { calls:vec![],started:Instant::now(),elapsed:Duration::ZERO,latency:Duration::ZERO,latency_on_peer:Duration::ZERO,frontier:vec![],edges:BTreeMap::new(),decision:true,denied_node:None,fault:None,admission:None,bound:None,
        entry:serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{},"config":{"model":{"id":"model","version":"1.0.0"}}})).unwrap(),
        peer:Some(crate::test_support::foreign_route("aidash://foreign")),marked:BTreeSet::new(),skipped:vec![],answered:vec![],met:Vec::new(),row_silence:None,
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
	transport
		.replies
		.lock()
		.unwrap()
		.push_back(Check::Confirmed(Checked {
			visible: true,
			pending: vec![local.clone(), foreign.clone()],
		}));
	// Act
	assert!(
		verify_all(&mut scope, &transport, vec![local])
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
	assert_eq!(calls[0].1, json!({"reference":foreign}));
	assert_eq!(calls[0].2, Duration::from_secs(20));
}

#[rstest]
#[case(false, true, 1)]
#[case(true, false, 2)]
#[tokio::test]
async fn remote_policy_and_route_gate_precede_transport(
	mut scope: Scope,
	transport: Transport,
	#[case] allowed: bool,
	#[case] enabled: bool,
	#[case] calls: usize,
) {
	scope.decision = allowed;
	if !enabled {
		scope.peer = None;
	}
	assert!(
		!verify_all(&mut scope, &transport, vec![grant("aidash://foreign", 1)])
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
		!verify_all(&mut scope, &transport, vec![grant("aidash://local", 1)])
			.await
			.unwrap()
	);
	assert_eq!(
		scope.calls,
		vec![
			format!("grant:{}", Uuid::from_u128(1)),
			"decide:federation.discover:aidash://foreign".into(),
			"peer:aidash://foreign".into()
		]
	);
	assert!(transport.calls.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn the_deadline_names_no_node_to_a_reader_who_may_not_discover_it(
	mut scope: Scope,
	transport: Transport,
) {
	scope.latency = Duration::from_secs(20);
	scope.decision = false;
	scope
		.edges
		.insert(Uuid::from_u128(1), vec![grant("aidash://foreign", 2)]);
	assert!(
		!verify_all(&mut scope, &transport, vec![grant("aidash://local", 1)])
			.await
			.unwrap()
	);
	assert!(scope.marked.is_empty());
	assert!(transport.calls.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn a_mark_does_not_stand_in_for_this_readers_discover_right(
	mut scope: Scope,
	transport: Transport,
) {
	// The mark may come from another identity of the same authority.
	scope.marked.insert("aidash://foreign".into());
	scope.decision = false;
	assert!(
		!verify_all(&mut scope, &transport, vec![grant("aidash://foreign", 1)])
			.await
			.unwrap()
	);
	assert_eq!(
		scope.calls,
		vec!["decide:federation.discover:aidash://foreign"]
	);
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
		verify_all(&mut scope, &transport, edges).await.unwrap(),
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
	transport
		.replies
		.lock()
		.unwrap()
		.push_back(Check::Confirmed(Checked {
			visible,
			pending: vec![grant("aidash://local", 1); count],
		}));
	assert!(
		!verify_all(&mut scope, &transport, vec![grant("aidash://foreign", 1)])
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
		!verify_all(&mut scope, &transport, vec![grant("aidash://foreign", 1)])
			.await
			.unwrap()
	);
	assert_eq!(scope.calls.len(), 2);
	assert_eq!(transport.calls.lock().unwrap().len(), 1);
}

#[rstest]
#[tokio::test]
async fn a_node_that_did_not_answer_is_marked_and_not_asked_again(
	mut scope: Scope,
	transport: Transport,
) {
	// Arrange: the first reference gets no answer.
	transport
		.replies
		.lock()
		.unwrap()
		.push_back(Check::Unavailable);
	// Act
	let first = verify_all(&mut scope, &transport, vec![grant("aidash://foreign", 1)])
		.await
		.unwrap();
	let calls_after_first = scope.calls.len();
	let second = verify_all(&mut scope, &transport, vec![grant("aidash://foreign", 2)])
		.await
		.unwrap();
	// Assert: both are unconfirmed, the Node is marked, and the second walk
	// only checked this reader's right to discover it, then did not ask.
	assert!(!first && !second);
	assert!(scope.marked.contains("aidash://foreign"));
	assert_eq!(transport.calls.lock().unwrap().len(), 1);
	// Each walk met the Node, the second without asking it, so the call that made
	// the second walk can tell that its refusal is that Node's silence.
	assert_eq!(scope.met, ["aidash://foreign", "aidash://foreign"]);
	assert_eq!(
		scope.calls[calls_after_first..],
		["decide:federation.discover:aidash://foreign"]
	);
}

#[rstest]
#[tokio::test]
async fn every_row_stopped_by_a_silent_node_is_reported_and_a_refusal_is_not(
	mut scope: Scope,
	transport: Transport,
) {
	// Arrange: the first reference gets no answer, three more rows follow, then a
	// refusal from another reference on the same Node would be asked again only
	// if the Node had not been marked.
	transport
		.replies
		.lock()
		.unwrap()
		.push_back(Check::Unavailable);
	// Act
	for id in 1..=4 {
		assert!(
			!verify_all(&mut scope, &transport, vec![grant("aidash://foreign", id)])
				.await
				.unwrap()
		);
	}
	// Assert: the walk that met the silence marked the Node, and each of the
	// three later rows stopped at it without asking.
	assert!(scope.marked.contains("aidash://foreign"));
	assert_eq!(scope.skipped, ["aidash://foreign"; 3]);
	assert_eq!(transport.calls.lock().unwrap().len(), 1);
}

#[rstest]
#[tokio::test]
async fn a_node_that_answers_is_reported_as_answering_even_when_it_refuses(
	mut scope: Scope,
	transport: Transport,
) {
	transport.replies.lock().unwrap().push_back(Check::Refused);
	assert!(
		!verify_all(&mut scope, &transport, vec![grant("aidash://foreign", 1)])
			.await
			.unwrap()
	);
	assert_eq!(scope.answered, ["aidash://foreign"]);
	assert!(scope.skipped.is_empty());
}

#[rstest]
#[case::refused(Check::Refused)]
#[case::not_asked(Check::NotAsked)]
#[tokio::test]
async fn a_refusal_or_a_node_nobody_asked_is_not_marked(
	mut scope: Scope,
	transport: Transport,
	#[case] reply: Check,
) {
	transport.replies.lock().unwrap().push_back(reply);
	assert!(
		!verify_all(&mut scope, &transport, vec![grant("aidash://foreign", 1)])
			.await
			.unwrap()
	);
	assert!(scope.marked.is_empty());
	// A refusal is asked about again: nothing remembers it.
	transport.replies.lock().unwrap().push_back(Check::Refused);
	assert!(
		!verify_all(&mut scope, &transport, vec![grant("aidash://foreign", 2)])
			.await
			.unwrap()
	);
	assert_eq!(transport.calls.lock().unwrap().len(), 2);
}

#[rstest]
#[tokio::test]
async fn the_deadline_names_the_node_whose_reference_was_left_unchecked(
	mut scope: Scope,
	transport: Transport,
) {
	// Arrange: the local grant uses the whole budget and discloses a reference
	// on another Node, then a local one.
	scope.latency = Duration::from_secs(20);
	scope.edges.insert(
		Uuid::from_u128(1),
		vec![grant("aidash://local", 3), grant("aidash://foreign", 2)],
	);
	// Act
	assert!(
		!verify_all(&mut scope, &transport, vec![grant("aidash://local", 1)])
			.await
			.unwrap()
	);
	// Assert: the reference popped first is the foreign one.
	assert!(scope.marked.contains("aidash://foreign"));
	assert!(transport.calls.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn the_deadline_on_this_node_marks_the_last_node_asked(
	mut scope: Scope,
	transport: Transport,
) {
	// The slow answer from the foreign Node used the time; the next reference
	// is on this Node, so the Node that was slow is the one to name.
	transport
		.replies
		.lock()
		.unwrap()
		.push_back(Check::Confirmed(Checked {
			visible: true,
			pending: vec![grant("aidash://local", 3)],
		}));
	scope.latency_on_peer = Duration::from_secs(20);
	assert!(
		!verify_all(&mut scope, &transport, vec![grant("aidash://foreign", 1)])
			.await
			.unwrap()
	);
	assert!(scope.marked.contains("aidash://foreign"));
}

#[rstest]
#[tokio::test]
async fn the_deadline_on_this_node_with_no_node_asked_marks_nothing(
	mut scope: Scope,
	transport: Transport,
) {
	scope.latency = Duration::from_secs(20);
	scope.edges.insert(
		Uuid::from_u128(1),
		vec![grant("aidash://local", 3), grant("aidash://local", 4)],
	);
	assert!(
		!verify_all(&mut scope, &transport, vec![grant("aidash://local", 1)])
			.await
			.unwrap()
	);
	assert!(scope.marked.is_empty());
}

#[rstest]
#[tokio::test]
async fn the_deadline_names_no_node_that_has_no_route(mut scope: Scope, transport: Transport) {
	scope.latency = Duration::from_secs(20);
	scope.peer = None;
	scope
		.edges
		.insert(Uuid::from_u128(1), vec![grant("aidash://foreign", 2)]);
	assert!(
		!verify_all(&mut scope, &transport, vec![grant("aidash://local", 1)])
			.await
			.unwrap()
	);
	assert!(scope.marked.is_empty());
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
#[case("agent", true, true, 2)]
#[case("model", true, false, 2)]
#[case("agent", false, false, 2)]
#[tokio::test]
async fn registry_dependency_keeps_kind_and_digest_gates_without_asking_for_execution(
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
	assert!(
		!scope
			.calls
			.iter()
			.any(|call| call.contains("agent.execute"))
	);
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
			node: "brave-otter-k3m9q2xa".parse().unwrap(),
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
			control_revision: 0,
			control_source: Default::default(),
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

#[rstest]
#[case::bad_request(400, true)]
#[case::unauthorized(401, true)]
#[case::forbidden(403, true)]
#[case::missing(404, true)]
#[case::conflict(409, true)]
#[case::gone(410, true)]
#[case::unprocessable(422, true)]
#[case::request_timeout(408, false)]
#[case::too_early(425, false)]
#[case::too_many_requests(429, false)]
#[case::server_error(500, false)]
#[case::unavailable(503, false)]
#[case::gateway_timeout(504, false)]
#[case::redirect(302, false)]
#[case::no_content(204, false)]
fn a_peer_that_answered_refuses_and_one_that_could_not_is_unavailable(
	#[case] status: u16,
	#[case] refused: bool,
) {
	assert_eq!(crate::ports::federation::refuses(status), refused);
	assert_eq!(
		matches!(Check::of_status(status), Check::Refused),
		refused,
		"{status}"
	);
}

#[rstest]
#[tokio::test]
async fn a_silent_dependency_does_not_hide_a_refusal_on_another_node(
	mut scope: Scope,
	transport: Transport,
) {
	transport
		.replies
		.lock()
		.unwrap()
		.extend([Check::Unavailable, Check::Refused]);
	let result = verify_all(
		&mut scope,
		&transport,
		vec![grant("aidash://second", 1), grant("aidash://foreign", 2)],
	)
	.await
	.unwrap();
	assert!(!result);
	assert_eq!(scope.row_silence, None);
	assert_eq!(transport.calls.lock().unwrap().len(), 2);
}

#[rstest]
#[tokio::test]
async fn a_silent_node_does_not_hide_discovery_denial_at_the_walk_deadline(
	mut scope: Scope,
	transport: Transport,
) {
	scope.latency_on_peer = Duration::from_secs(20);
	scope.denied_node = Some("aidash://second".into());
	transport
		.replies
		.lock()
		.unwrap()
		.push_back(Check::Unavailable);
	assert!(
		!verify_all(
			&mut scope,
			&transport,
			vec![grant("aidash://second", 1), grant("aidash://foreign", 2)]
		)
		.await
		.unwrap()
	);
	assert_eq!(scope.row_silence, None);
}
