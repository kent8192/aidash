use super::*;

use crate::apps::execution::test_database as native_database;

#[rstest::rstest]
#[tokio::test]
async fn a_catalog_admission_does_not_cross_nodes_in_one_transaction(
	#[from(native_database::database)] database: native_database::DatabaseFuture,
) {
	// Arrange: only the default Node approves this definition.
	let database = database.await;
	let tenancy = database.tenant("acme").await;
	let pool = database.pool().await;
	let deployment = aidash_domain::nodes::DeploymentId::parse("aidash://catalog-cache").unwrap();
	let store = Store::from_pool(pool.fixture_driver().clone(), deployment.clone())
		.await
		.unwrap();
	let authorization = Authorization { pool: pool.clone() };
	let bundle = serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[{"id":"work","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]})).unwrap();
	authorization
		.replace("acme", 0, bundle, "fixture")
		.await
		.unwrap();
	let issued = authorization
		.issue_credential("acme", "alice", 3600, "fixture")
		.await
		.unwrap();
	let identity = SubjectIdentity {
		http_session: None,
		tenant: "acme".into(),
		subject: "alice".into(),
		credential_id: issued.credential.id,
	};
	let entry: crate::registry::Entry = serde_json::from_value(json!({"id":"first","version":"1.0.0","kind":"skill","name":{"en":"First"},"description":{"en":"Fixture"},"config":{"instructions":"Frozen"}})).unwrap();
	crate::registry::Registry::new(pool.clone(), &deployment)
		.unwrap()
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			entry.clone(),
		)
		.await
		.unwrap();
	let reference = crate::registry::EntityRef {
		id: entry.id.clone(),
		version: entry.version.clone(),
	};
	let default = aidash_domain::nodes::NodeName::derived("acme");
	authorization
		.set_catalog("acme", &default, &reference, 0, true, "fixture")
		.await
		.unwrap();
	let second = pool
		.atomic(&tenancy, async |tx| {
			crate::apps::identity::repositories::nodes::create(
				tx,
				"acme",
				"Second",
				false,
				crate::apps::identity::repositories::tenants::Creator::DeploymentAdministrator(
					None,
				),
			)
			.await
		})
		.await
		.unwrap();
	let mut access = Access::begin(&store, &identity).await.unwrap();
	access.set_node(default.clone());
	// Act: changing Nodes must reread that Node's approval on the same lease.
	let approved = crate::authorization::catalog::entry(&mut access, &reference, "registry.read")
		.await
		.unwrap();
	access.set_node(second.name);
	let denied =
		crate::authorization::catalog::entry(&mut access, &reference, "registry.read").await;
	access.set_node(default);
	let original = crate::authorization::catalog::entry(&mut access, &reference, "registry.read")
		.await
		.unwrap();
	// Assert
	assert_eq!(approved, entry);
	assert!(
		matches!(denied, Err(Error::Forbidden | Error::NotFound(_))),
		"{denied:?}"
	);
	assert_eq!(original, entry);
	access.finish(Ok(())).await.unwrap();
}

#[rstest::rstest]
#[tokio::test]
async fn a_change_decided_on_a_policy_that_has_since_granted_it_is_refused_as_stale(
	#[from(native_database::database)] database: native_database::DatabaseFuture,
) {
	// Arrange: the member may not update the policy, and a read says so.
	let database = database.await;
	database.tenant("acme").await;
	let pool = database.pool().await;
	let deployment = aidash_domain::nodes::DeploymentId::parse("aidash://stale-change").unwrap();
	let store = Store::from_pool(pool.fixture_driver().clone(), deployment)
		.await
		.unwrap();
	let authorization = Authorization { pool: pool.clone() };
	let mut document = json!({
		"tenant":"acme",
		"subjects":{
			"admin":{"kind":"user","groups":["administrators"]},
			"member":{"kind":"user"}
		},
		"groups":{"administrators":{"roles":["administrator"]}},
		"roles":{"administrator":{}},
		"policies":[{
			"id":"administer","effect":"allow","subjects":{"roles":["administrator"]},
			"actions":["*"],"resources":{"kinds":["*"]}
		}]
	});
	authorization
		.replace(
			"acme",
			0,
			serde_json::from_value(document.clone()).unwrap(),
			"fixture",
		)
		.await
		.unwrap();
	let issued = authorization
		.issue_credential("acme", "member", 3600, "fixture")
		.await
		.unwrap();
	let identity = SubjectIdentity {
		http_session: None,
		tenant: "acme".into(),
		subject: "member".into(),
		credential_id: issued.credential.id,
	};
	let read = authorization.snapshot("acme").await.unwrap();
	// Act: a change grants the member the permission before the member's lock.
	document["subjects"]["member"]["groups"] = json!(["administrators"]);
	authorization
		.replace(
			"acme",
			read.revision,
			serde_json::from_value(document).unwrap(),
			"fixture",
		)
		.await
		.unwrap();
	let stale =
		Access::begin_change_deciding_on(&store, &identity, &["policy.update"], &read.bundle).await;
	let current = authorization.snapshot("acme").await.unwrap();
	let fresh =
		Access::begin_change_deciding_on(&store, &identity, &["policy.update"], &current.bundle)
			.await;

	// Assert: the shared lock the stale decision took is not used to change the
	// policy, while a decision on the current policy takes the exclusive lock.
	assert!(matches!(stale, Err(Error::Conflict(_))));
	fresh
		.expect("a decision on the current policy stands")
		.finish(Ok(()))
		.await
		.unwrap();
}

#[rstest::fixture]
fn access() -> Access {
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: Uuid::new_v4(),
		tenant: "acme".into(),
		subject: "alice".into(),
	};
	let snapshot = Snapshot {
		revision: 1,
		bundle: serde_json::from_value(json!({
			"tenant":"acme", "subjects":{"alice":{"kind":"user"}},
			"policies":[
				{"id":"reader-peer", "effect":"allow", "subjects":{"ids":["alice"]},
				"actions":["memory.read"], "resources":{"kinds":["memory"]},
				"condition":{"op":"eq", "left":{"source":"environment","path":"/source_node"}, "right":{"source":"literal","value":"aidash://reader-peer"}}},
				{"id":"local-worker", "effect":"allow", "subjects":{"ids":["alice"]},
				"actions":["memory.write"], "resources":{"kinds":["memory"]},
				"condition":{"op":"eq", "left":{"source":"environment","path":"/transport"}, "right":{"source":"literal","value":"worker"}}}
			]
		})).unwrap(),
	};
	let environment = json!({"node_id":"aidash://home", "transport":"federation", "source_node":"aidash://reader-peer", "request_id":"reader-request"});
	Access {
		node: None,
		node_address: None,
		marketplace_audit: None,
		core_gc_complete: false,
		remote_read_cache: Default::default(),
		unavailable_peers: Default::default(),
		denied: false,
		silent_events: 0,
		silent_last: None,
		denial_events: 0,
		checking_reads: Default::default(),
		read_verdicts: Default::default(),
		read_block: None,
		wire: None,
		node_id: "aidash://home".into(),
		dependency_frontier: None,
		recheck_workspaces: Default::default(),
		subnode_workspaces: Default::default(),
		subnode_creators: Default::default(),
		// Same-credential publisher checks perform no database I/O.
		tx: AccessTransaction::default(),
		identity,
		snapshot,
		subjects: vec!["alice".into()],
		delegated_subject: None,
		durable_audit: false,
		audit: false,
		context: json!({}),
		inherited_lease: false,
		approved_catalog: Default::default(),
		cached_runs: Default::default(),
		cached_humans: Default::default(),
		pending_decisions: vec![],
		recorded_decisions: Default::default(),
		recheck_owner: None,
		walk_root: None,
		read_lookups: Default::default(),
		last_policy_explicit_deny: false,
		decision_serial: 0,
		pool: sqlx::postgres::PgPoolOptions::new()
			.connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
			.unwrap()
			.into(),
		read_run: None,
		read_grant: None,
		shared_area: false,
		home_corpus_digest: None,
		environment,
	}
}

#[rstest::rstest]
#[tokio::test]
async fn publisher_checks_use_local_worker_facts_and_restore_the_reader_environment(
	mut access: Access,
) {
	let environment = access.environment.clone();
	let identity = access.identity.clone();
	let resource = access.resource("memory", "bank", json!({}));
	let reader = access.evaluation("alice", &resource, "memory.read");
	assert!(access.snapshot.bundle.evaluate(&reader).allowed);
	{
		let publisher = access
			.publisher_view(identity, vec!["alice".into()])
			.await
			.unwrap();
		let read = publisher.evaluation("alice", &resource, "memory.read");
		assert!(
			!publisher.snapshot.bundle.evaluate(&read).allowed,
			"reader peer facts cannot grant publisher permissions"
		);
		let write = publisher.evaluation("alice", &resource, "memory.write");
		assert!(
			publisher.snapshot.bundle.evaluate(&write).allowed,
			"local publisher checks use worker authority"
		);
		assert_eq!(
			write.environment,
			json!({"node_id":"aidash://home","transport":"worker"})
		);
	}
	assert_eq!(access.environment, environment);
	assert!(
		access
			.snapshot
			.bundle
			.evaluate(&access.evaluation("alice", &resource, "memory.read"))
			.allowed
	);
}

/// A refusal that follows a Node that did not answer is that Node's silence;
/// a refusal of this authority's own `require` stays a refusal (#751).
#[rstest::rstest]
#[tokio::test]
async fn a_forbidden_after_a_silent_node_names_the_node_unless_this_authority_denied(
	mut access: Access,
) {
	// Nobody is silent: the refusal passes through.
	assert!(matches!(access.named(Error::Forbidden), Error::Forbidden));
	access.mark_peer_unavailable("aidash://other/sibling");
	access.mark_peer_unavailable("aidash://other/zebra");
	// The first Node in a fixed order is named, and nothing else changes.
	assert!(matches!(
		access.named(Error::Forbidden),
		Error::NodeUnreachable { node, after_effect: false } if node == "aidash://other/sibling"
	));
	assert!(matches!(
		access.named(Error::Conflict("x".into())),
		Error::Conflict(_)
	));
	assert!(matches!(
		access.named(Error::Unauthorized),
		Error::Unauthorized
	));
	// A `require` of this authority that denies is a real refusal.
	let workspace = access.resource("workspace", "w", json!({}));
	assert!(matches!(
		access.require(&workspace, "workspace.read").await,
		Err(Error::Forbidden)
	));
	assert!(matches!(access.named(Error::Forbidden), Error::Forbidden));
}

/// One authority serves several calls, such as the calls of a Tool Batch. A Node
/// an earlier call met, or a denial it made, says nothing about a later call's
/// refusal: only what the call itself met and denied counts (#751).
#[rstest::rstest]
#[tokio::test]
async fn a_call_is_blamed_on_a_silent_node_only_if_its_own_checks_met_the_node(mut access: Access) {
	let sibling = "aidash://other/sibling";
	let workspace = access.resource("workspace", "w", json!({}));
	let silent = |error: Error| matches!(error, Error::NodeUnreachable { node, after_effect: false } if node == sibling);
	// A call that met the Node: its refusal is the Node's silence.
	let first = access.call_mark();
	access.mark_peer_unavailable(sibling);
	assert!(silent(access.named_since(first, Error::Forbidden)));
	// A later call on the same authority never met it, and is refused as it is.
	let second = access.call_mark();
	assert!(matches!(
		access.named_since(second, Error::Forbidden),
		Error::Forbidden
	));
	// The reading of a whole authority, which serves one call, still names it.
	assert!(silent(access.named(Error::Forbidden)));
	// A call that meets the Node again is told it is silent, though it is not asked again.
	let third = access.call_mark();
	assert!(access.peer_unavailable(sibling));
	access.mark_peer_unavailable(sibling);
	assert!(silent(access.named_since(third, Error::Forbidden)));
	// Unless a `require` of that same call denied: that is a real refusal.
	assert!(matches!(
		access.require(&workspace, "workspace.read").await,
		Err(Error::Forbidden)
	));
	assert!(matches!(
		access.named_since(third, Error::Forbidden),
		Error::Forbidden
	));
	// A denial of an earlier call does not hide a later call's silence.
	let fourth = access.call_mark();
	access.mark_peer_unavailable(sibling);
	assert!(silent(access.named_since(fourth, Error::Forbidden)));
	// Only a refusal is made into silence.
	assert!(matches!(
		access.named_since(fourth, Error::Conflict("x".into())),
		Error::Conflict(_)
	));
}

/// What a publisher view finds silent is not the reader call's own (#751).
#[rstest::rstest]
#[tokio::test]
async fn a_publishers_silent_node_does_not_count_for_the_readers_call(mut access: Access) {
	let identity = access.identity.clone();
	let call = access.call_mark();
	{
		let mut publisher = access
			.publisher_view(identity, vec!["alice".into()])
			.await
			.unwrap();
		publisher.mark_peer_unavailable("aidash://other/publisher");
	}
	assert!(matches!(
		access.named_since(call, Error::Forbidden),
		Error::Forbidden
	));
}

/// No negative read verdict is kept while a Node has not answered, and a
/// positive one is (#751).
#[rstest::rstest]
#[tokio::test]
async fn no_negative_read_verdict_is_remembered_while_a_node_is_silent(mut access: Access) {
	let run = Uuid::new_v4();
	let other = Uuid::new_v4();
	access.remember_remote_read(run, false);
	assert_eq!(access.cached_remote_read(run), Some(false));
	access.mark_peer_unavailable("aidash://other/sibling");
	access.remember_remote_read(other, false);
	assert_eq!(
		access.cached_remote_read(other),
		None,
		"a refusal reached while a Node was silent is asked again"
	);
	access.remember_remote_read(other, true);
	assert_eq!(access.cached_remote_read(other), Some(true));
}

/// A memory step names a Node that did not answer for `Invalidated` as well as
/// for `Authority`, so decisions 7 and 8 can fire; other reasons keep their
/// meaning (#751).
#[rstest::rstest]
#[tokio::test]
async fn a_memory_step_names_a_silent_node_for_authority_and_invalidated(mut access: Access) {
	use aidash_domain::semantic::Failure;
	let step = CallMark::default();
	// Nobody is silent: the old mapping.
	assert!(matches!(
		access.memory_failure_since(step, Failure::Authority),
		Error::Forbidden
	));
	assert!(matches!(
		access.memory_failure_since(step, Failure::Invalidated),
		Error::RemoteSemantic(Failure::Invalidated)
	));
	access.mark_peer_unavailable("aidash://other/sibling");
	for reason in [Failure::Authority, Failure::Invalidated] {
		assert!(matches!(
			access.memory_failure_since(step, reason),
			Error::NodeUnreachable { node, after_effect: false } if node == "aidash://other/sibling"
		));
	}
	// Another reason is not made into silence.
	assert!(matches!(
		access.memory_failure_since(step, Failure::Allowance),
		Error::RemoteSemantic(Failure::Allowance)
	));
	// A denial of this authority stays a refusal.
	let workspace = access.resource("workspace", "w", json!({}));
	assert!(matches!(
		access.require(&workspace, "workspace.read").await,
		Err(Error::Forbidden)
	));
	assert!(matches!(
		access.memory_failure_since(step, Failure::Authority),
		Error::Forbidden
	));
	assert!(matches!(
		access.memory_failure_since(step, Failure::Invalidated),
		Error::RemoteSemantic(Failure::Invalidated)
	));
}

/// The memory path reads a call the same way: only a Node that call's own
/// checks met is named, not one an earlier call found silent (#751).
#[rstest::rstest]
#[tokio::test]
async fn a_memory_step_names_only_a_node_its_own_call_met(mut access: Access) {
	use aidash_domain::semantic::Failure;
	access.mark_peer_unavailable("aidash://other/sibling");
	let later = access.call_mark();
	assert!(matches!(
		access.memory_failure_since(later, Failure::Authority),
		Error::Forbidden
	));
	assert!(matches!(
		access.memory_failure_since(later, Failure::Invalidated),
		Error::RemoteSemantic(Failure::Invalidated)
	));
	access.mark_peer_unavailable("aidash://other/sibling");
	for reason in [Failure::Authority, Failure::Invalidated] {
		assert!(matches!(
			access.memory_failure_since(later, reason),
			Error::NodeUnreachable {
				after_effect: false,
				..
			}
		));
	}
}

/// A publisher check runs under another identity on the reader's `Access`. Its
/// silent Nodes and denials are its own: they are discarded when it leaves, and
/// the reader's survive it (#751). Only a reader who may discover a Node is
/// told it was silent, by `adopt_silent_peers`; to any other reader the Node
/// stays unnamed.
#[rstest::rstest]
#[tokio::test]
async fn silent_nodes_and_denials_do_not_cross_a_publisher_view(mut access: Access) {
	let identity = access.identity.clone();
	// The publisher's mark and denial do not reach the reader.
	{
		let mut publisher = access
			.publisher_view(identity.clone(), vec!["alice".into()])
			.await
			.unwrap();
		assert!(!publisher.peer_unavailable("aidash://other/reader"));
		publisher.mark_peer_unavailable("aidash://other/publisher");
		publisher.denied = true;
	}
	assert!(!access.peer_unavailable("aidash://other/publisher"));
	assert!(matches!(access.named(Error::Forbidden), Error::Forbidden));
	// The reader's mark survives entering and leaving, and is not visible inside.
	access.mark_peer_unavailable("aidash://other/reader");
	{
		let publisher = access
			.publisher_view(identity.clone(), vec!["alice".into()])
			.await
			.unwrap();
		assert!(!publisher.peer_unavailable("aidash://other/reader"));
		assert!(matches!(
			publisher.named(Error::Forbidden),
			Error::Forbidden
		));
	}
	assert!(access.peer_unavailable("aidash://other/reader"));
	assert!(matches!(
		access.named(Error::Forbidden),
		Error::NodeUnreachable { node, after_effect: false } if node == "aidash://other/reader"
	));
	// A denial the reader suffered survives too, and stays a refusal.
	access.denied = true;
	{
		let _publisher = access
			.publisher_view(identity, vec!["alice".into()])
			.await
			.unwrap();
	}
	assert!(matches!(access.named(Error::Forbidden), Error::Forbidden));
}

/// What the publisher found silent reaches the reader only when the reader may
/// discover that Node (#751).
#[rstest::rstest]
#[case::may_discover(true)]
#[case::may_not_discover(false)]
#[tokio::test]
async fn a_publishers_silent_node_is_the_readers_only_if_the_reader_may_discover_it(
	mut access: Access,
	#[case] allowed: bool,
) {
	if allowed {
		access.snapshot.bundle = serde_json::from_value(json!({
			"tenant":"acme", "subjects":{"alice":{"kind":"user"}},
			"policies":[{"id":"discover","effect":"allow","subjects":{"ids":["alice"]},
				"actions":["federation.discover"],"resources":{"kinds":["node"]}}]
		}))
		.unwrap();
	}
	let identity = access.identity.clone();
	let silent = {
		let mut publisher = access
			.publisher_view(identity, vec!["alice".into()])
			.await
			.unwrap();
		publisher.mark_peer_unavailable("aidash://other/publisher");
		publisher.take_peer_marks()
	};
	assert!(!access.peer_unavailable("aidash://other/publisher"));
	access.adopt_silent_peers(silent).await.unwrap();
	assert_eq!(access.peer_unavailable("aidash://other/publisher"), allowed);
	// Allowed: a refusal that follows is the Node's silence. Otherwise it stays.
	assert_eq!(
		matches!(
			access.named(Error::Forbidden),
			Error::NodeUnreachable { .. }
		),
		allowed
	);
}

#[rstest::rstest]
#[case::delegated_agent_is_denied(Some("agent-x"), Some("agent-x"))]
#[case::no_delegated_agent(None, None)]
#[tokio::test]
async fn a_denial_leaves_a_note_that_names_only_the_agent_delegated_to(
	mut access: Access,
	#[case] delegated: Option<&str>,
	#[case] named: Option<&str>,
) {
	// Arrange: the chain ends in an Agent the policy does not know, so it is denied.
	access.subjects = vec!["alice".into(), "agent-x".into()];
	access.delegated_subject = delegated.map(str::to_owned);
	let resource = access.resource("memory", "bank", json!({}));
	// Act
	let (result, note) = refusal_note::scoped(access.require(&resource, "memory.read")).await;
	// Assert
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(
		note,
		Some(
			refusal_note::Note::new(RefusalCheck::Policy, Some("memory.read"))
				.subject(named.map(str::to_owned))
		)
	);
}

#[rstest::rstest]
#[tokio::test]
async fn an_allowed_check_supersedes_an_earlier_policy_refusal_but_not_another_kind(
	mut access: Access,
) {
	let resource = access.resource("memory", "bank", json!({}));
	let denied = access.resource("secret", "vault", json!({}));
	let (_, note) = refusal_note::scoped(async {
		assert!(access.require(&denied, "secret.use").await.is_err());
		access.require(&resource, "memory.read").await.unwrap();
	})
	.await;
	assert_eq!(note, None, "the last check wins");
	let (_, note) = refusal_note::scoped(async {
		refusal_note::set(refusal_note::Note::new(RefusalCheck::TenantDeletion, None));
		access.require(&resource, "memory.read").await.unwrap();
	})
	.await;
	assert_eq!(
		note,
		Some(refusal_note::Note::new(RefusalCheck::TenantDeletion, None))
	);
}

/// A scoped response can audit hundreds of checks. Database round trips, rather
/// than the number of decisions, must bound the cost of finishing that response.
#[rstest::rstest]
#[case::empty(0, 0)]
#[case::single(1, 3)]
#[case::compound(101, 4)]
#[tokio::test]
async fn finishing_a_compound_check_bounds_audit_round_trips(
	mut access: Access,
	#[case] decisions: usize,
	#[case] budget: usize,
) {
	use reinhardt::db::backends::{QueryResult, QueryValue, Row};
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};
	type DbResult<T> = reinhardt::db::backends::error::Result<T>;
	#[derive(Clone, Default)]
	struct RoundTrips(Arc<AtomicUsize>, Arc<AtomicUsize>);
	#[async_trait::async_trait]
	impl TransactionExecutor for RoundTrips {
		async fn execute(&mut self, _: &str, values: Vec<QueryValue>) -> DbResult<QueryResult> {
			self.0.fetch_add(1, Ordering::Relaxed);
			self.1.fetch_add(
				values
					.iter()
					.filter(|value| matches!(value, QueryValue::Json(_)))
					.count(),
				Ordering::Relaxed,
			);
			Ok(QueryResult {
				rows_affected: 1,
				last_insert_id: None,
			})
		}
		async fn fetch_one(&mut self, _: &str, _: Vec<QueryValue>) -> DbResult<Row> {
			unreachable!("audit writes return no rows")
		}
		async fn fetch_all(&mut self, _: &str, _: Vec<QueryValue>) -> DbResult<Vec<Row>> {
			unreachable!("audit writes return no rows")
		}
		async fn fetch_optional(&mut self, _: &str, _: Vec<QueryValue>) -> DbResult<Option<Row>> {
			unreachable!("audit writes return no rows")
		}
		async fn commit(self: Box<Self>) -> DbResult<()> {
			Ok(())
		}
		async fn rollback(self: Box<Self>) -> DbResult<()> {
			Ok(())
		}
	}
	// Arrange: only the database transport is replaced; Access owns the real
	// pending audit queue, SQL generation, transaction completion, and batching.
	let decision = Decision {
		allowed: true,
		reason: "allow".into(),
		matched_policies: vec![],
		effective_roles: Default::default(),
		revision: 1,
	};
	for id in 0..decisions {
		let resource = access.resource("workspace", id, json!({}));
		access.pending_decisions.push(DecisionRow::direct(
			access.evaluation("alice", &resource, "workspace.read"),
			decision.clone(),
		));
	}
	let transport = RoundTrips::default();
	// Act
	access
		.complete(Some(Box::new(transport.clone())), Ok(()))
		.await
		.unwrap();
	// Assert: per-decision SQL would make a large response spend its time on I/O.
	let round_trips = transport.0.load(Ordering::Relaxed);
	assert_eq!(
		transport.1.load(Ordering::Relaxed),
		decisions,
		"every decision must reach the database transport"
	);
	assert!(
		round_trips <= budget,
		"{decisions} decisions took {round_trips} database round trips (budget {budget})"
	);
}

#[tokio::test]
async fn catalog_admissions_end_with_their_transaction_or_a_savepoint_rollback() {
	let pool: Pool = sqlx::postgres::PgPoolOptions::new()
		.connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
		.unwrap()
		.into();
	let admission = CatalogAdmission {
		tenant: "acme".into(),
		node: None,
		subject: "alice".into(),
		revision: 1,
		inherited_lease: false,
		audit: true,
		durable_audit: true,
		authority: "authority".into(),
		id: "first".into(),
		version: "1.0.0".into(),
		action: "registry.read".into(),
	};
	let entry: crate::registry::Entry = serde_json::from_value(json!({
		"id":"first", "version":"1.0.0", "kind":"skill", "name":{"en":"first"},
		"description":{"en":"Fixture"}, "config":{"instructions":"Frozen"}
	}))
	.unwrap();
	let probe = crate::database::native::probe::SavepointProbe::default();
	let mut tx = AccessTransaction::new(probe.transaction(pool.clone()));
	tx.admit(admission.clone(), entry.clone());
	assert_eq!(tx.admitted(&admission), Some(entry.clone()));
	tx.rollback_to_savepoint("admission").await.unwrap();
	assert_eq!(probe.statements(), ["ROLLBACK TO SAVEPOINT"]);
	assert_eq!(
		tx.admitted(&admission),
		None,
		"a savepoint rollback may have released the admission's locks"
	);
	tx.admit(admission.clone(), entry.clone());
	assert_eq!(tx.admitted(&admission), Some(entry.clone()));
	drop(tx.take());
	assert_eq!(tx.admitted(&admission), None);
	tx.admit(admission.clone(), entry.clone());
	tx.install(crate::database::native::Transaction::detached(pool));
	assert_eq!(
		tx.admitted(&admission),
		None,
		"a new transaction holds none of the earlier locks"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn node_liveness_is_rechecked_after_a_savepoint_rollback(
	#[from(native_database::database)] database: native_database::DatabaseFuture,
) {
	let database = database.await;
	let tenancy = database.tenant("acme").await;
	let pool = database.pool().await;
	let node = aidash_domain::nodes::NodeName::derived("acme");
	let mut tx = AccessTransaction::new(pool.begin(&tenancy).await.unwrap());
	tx.savepoint("node_check").await.unwrap();
	tx.require_live_node("acme", node.as_str()).await.unwrap();
	tx.rollback_to_savepoint("node_check").await.unwrap();

	// A later state change must be observed even if an earlier savepoint had
	// checked this Node. The deletion stays inside this rolled-back fixture.
	let query = Query::insert()
		.into_table(Alias::new("node_deletions"))
		.columns(
			[
				"id",
				"tenant",
				"node",
				"deleted_at",
				"deleted_by_kind",
				"purge_after",
				"purge_failures",
			]
			.map(Alias::new),
		)
		.from_subquery(
			Query::select()
				.expr(Expr::value(Uuid::new_v4()))
				.expr(Expr::value("acme"))
				.expr(Expr::value(node.as_str()))
				.expr(Expr::cust("CURRENT_TIMESTAMP"))
				.expr(Expr::value("deployment_administrator"))
				.expr(Expr::cust("CURRENT_TIMESTAMP + INTERVAL '1 day'"))
				.expr(Expr::value(0_i64))
				.to_owned(),
		)
		.to_string(PostgresQueryBuilder);
	crate::database::native::query(&query)
		.execute(&mut **tx)
		.await
		.unwrap();
	assert!(matches!(
		tx.require_live_node("acme", node.as_str()).await,
		Err(Error::Forbidden)
	));
	tx.take().unwrap().rollback().await.unwrap();
	// A new transaction must not inherit either the prior result or deletion.
	tx.install(pool.begin(&tenancy).await.unwrap());
	tx.require_live_node("acme", node.as_str()).await.unwrap();
	tx.take().unwrap().rollback().await.unwrap();
}

#[tokio::test]
async fn workspace_owner_cache_respects_lock_strength_and_savepoint_rollback() {
	let pool: Pool = sqlx::postgres::PgPoolOptions::new()
		.connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
		.unwrap()
		.into();
	let probe = crate::database::native::probe::SavepointProbe::default();
	let mut tx = AccessTransaction::new(probe.transaction(pool));
	let locked = Uuid::new_v4();
	let unlocked = Uuid::new_v4();
	tx.remember_workspace_owner(locked, true, "alice");
	tx.remember_workspace_owner(unlocked, false, "bob");
	assert_eq!(tx.workspace_owner(locked, false).as_deref(), Some("alice"));
	assert_eq!(tx.workspace_owner(unlocked, true), None);
	tx.rollback_to_savepoint("owner").await.unwrap();
	assert_eq!(tx.workspace_owner(locked, false), None);
	assert_eq!(tx.workspace_owner(unlocked, false), None);
}
