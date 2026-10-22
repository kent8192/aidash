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
		checking_reads: Default::default(),
		read_verdicts: Default::default(),
		wire: None,
		node_id: "aidash://home".into(),
		dependency_frontier: None,
		// Same-credential publisher checks perform no database I/O.
		tx: AccessTransaction::default(),
		identity,
		snapshot,
		subjects: vec!["alice".into()],
		durable_audit: false,
		audit: false,
		context: json!({}),
		inherited_lease: false,
		approved_catalog: Default::default(),
		cached_runs: Default::default(),
		cached_humans: Default::default(),
		pending_decisions: vec![],
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
		access.pending_decisions.push((
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
