use super::*;

#[tokio::test]
async fn publisher_checks_use_local_worker_facts_and_restore_the_reader_environment() {
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
	let mut access = Access {
		marketplace_audit: None,
		core_gc_complete: false,
		remote_read_cache: Default::default(),
		unavailable_peers: Default::default(),
		checking_reads: Default::default(),
		peer_client: reqwest::Client::new(),
		node_id: "aidash://home".into(),
		dependency_frontier: None,
		// Same-credential publisher checks perform no database I/O.
		tx: AccessTransaction(None),
		identity: identity.clone(),
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
		environment: environment.clone(),
	};
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
