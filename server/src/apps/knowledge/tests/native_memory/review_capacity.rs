//! Accepted capacities preserve configuration, source mutation, and recovery.
use super::*;
use aidash_server::semantic::{service, services::memory_recovery};

async fn bank(store: &Store, workspace: Uuid, participant: bool) -> Bank {
	if participant {
		memory::create_participant(
			store,
			&Actor::Operator,
			workspace,
			memory::CreateParticipant {
				agent: reference("a"),
			},
		)
		.await
		.unwrap()
		.bank
	} else {
		let bank = Bank {
			home: store.node_id.clone(),
			tenant: "acme".into(),
			workspace,
			participant: None,
		};
		memory::operate(
			store,
			&Actor::Operator,
			memory::Operation {
				operation_id: Uuid::now_v7(),
				provider: reference("p"),
				bank: bank.clone(),
				action: memory::Action::ConfigureBank {
					expected_revision: 0,
				},
			},
		)
		.await
		.unwrap();
		bank
	}
}
async fn add(store: &Store, bank: &Bank, evidence: Vec<Evidence>) -> Unit {
	let mut body = content("Current dependency");
	body.evidence = evidence;
	memory::mutate(
		store,
		&Actor::Operator,
		mutation(
			bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: body,
			},
		),
	)
	.await
	.unwrap()
	.remove(0)
}

#[rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
#[tokio::test]
async fn index_configuration_preserves_every_pinned_bank_embedding(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] participant: bool,
	#[case] populated: bool,
) {
	let database = database.await;
	let (store, registry, workspace) = setup(&database, bounds).await;
	let bank = bank(&store, workspace, participant).await;
	if populated {
		add(&store, &bank, vec![]).await;
	}
	let before = service::get_index(&store, &Actor::Operator, workspace)
		.await
		.unwrap();
	let mut disabled = before.configuration().unwrap();
	disabled.enabled = false;
	assert!(
		matches!(service::configure(&store, workspace, aidash_server::semantic::ConfigureIndex {
		expected_revision: before.revision, spec: disabled }).await,
		Err(aidash_server::Error::Conflict(message)) if message.contains("remain enabled"))
	);
	let mut spec = before.configuration().unwrap();
	spec.embedding.model = "incompatible-model".into();
	assert!(
		matches!(service::configure(&store, workspace, aidash_server::semantic::ConfigureIndex { expected_revision: before.revision, spec }).await,
		Err(aidash_server::Error::Conflict(message)) if message.contains("pinned memory bank"))
	);
	let unchanged = service::get_index(&store, &Actor::Operator, workspace)
		.await
		.unwrap();
	assert_eq!(unchanged.revision, before.revision);
	assert_eq!(unchanged.collection, before.collection);
	assert_eq!(unchanged.spec, before.spec);
	let mut spec = before.configuration().unwrap();
	spec.max_results = 2;
	let compatible = service::configure(
		&store,
		workspace,
		aidash_server::semantic::ConfigureIndex {
			expected_revision: before.revision,
			spec,
		},
	)
	.await
	.unwrap();
	assert_eq!(compatible.revision, before.revision + 1);
	assert_ne!(compatible.collection, before.collection);

	let mut embedding = registry.get("e", "1.0.0").await.unwrap();
	embedding.id = "different-embedding".into();
	embedding.config["model"] = json!("different-model");
	registry.register(embedding).await.unwrap();
	let mut provider = registry.get("p", "1.0.0").await.unwrap();
	provider.id = "different-provider".into();
	provider.config["policy"]["embedding"] = json!(reference("different-embedding"));
	registry.register(provider).await.unwrap();
	// The same invariant also applies when pinning a bank against an existing index.
	let shared = Bank {
		participant: None,
		..bank.clone()
	};
	assert!(
		matches!(memory::operate(&store, &Actor::Operator, memory::Operation {
		operation_id: Uuid::now_v7(), provider: reference("different-provider"), bank: shared,
		action: memory::Action::ConfigureBank { expected_revision: if participant { 0 } else { 1 } },
	}).await, Err(aidash_server::Error::Conflict(message)) if message.contains("Workspace index"))
	);
	assert_eq!(
		memory::list(
			&store,
			&Actor::Operator,
			memory::ReadBank {
				provider: reference("p"),
				bank
			}
		)
		.await
		.unwrap()
		.len(),
		usize::from(populated)
	);
}

#[rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
#[tokio::test]
async fn dependency_admission_keeps_roots_correctable_and_deletable(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
	#[case] transitive: bool,
	#[case] delete: bool,
) {
	bounds.max_graph_visits = 3;
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = bank(&store, workspace, true).await;
	let root = add(&store, &bank, vec![]).await;
	let first = add(&store, &bank, vec![root.evidence()]).await;
	let second = add(
		&store,
		&bank,
		vec![if transitive {
			first.evidence()
		} else {
			root.evidence()
		}],
	)
	.await;
	let mut rejected = content("Would make the root impossible to change");
	rejected.evidence = vec![if transitive {
		second.evidence()
	} else {
		root.evidence()
	}];
	let id = Uuid::now_v7();
	assert!(
		matches!(memory::mutate(&store, &Actor::Operator, mutation(&bank, Change::Add { id, content: rejected })).await,
		Err(aidash_server::Error::Conflict(message)) if message.contains("dependency impact"))
	);
	let current = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			provider: reference("p"),
			bank: bank.clone(),
		},
	)
	.await
	.unwrap();
	assert_eq!(current.len(), 3);
	assert!(!current.iter().any(|unit| unit.id == id));
	let change = if delete {
		Change::Delete {
			id: root.id,
			expected_revision: 1,
		}
	} else {
		Change::Correct {
			id: root.id,
			expected_revision: 1,
			content: content("Corrected root"),
		}
	};
	let result = memory::mutate(&store, &Actor::Operator, mutation(&bank, change))
		.await
		.unwrap();
	assert_eq!(result[0].revision, 2);
	assert_eq!(result[0].deleted, delete);
	let current = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			provider: reference("p"),
			bank,
		},
	)
	.await
	.unwrap();
	assert_eq!(current.len(), usize::from(!delete));
	assert!(
		current
			.iter()
			.all(|unit| unit.id == root.id && unit.content.text == "Corrected root")
	);
}

#[rstest]
#[tokio::test]
async fn batched_root_corrections_each_keep_their_bounded_impact(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
) {
	bounds.max_graph_visits = 2;
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = bank(&store, workspace, true).await;
	let a = add(&store, &bank, vec![]).await;
	let b = add(&store, &bank, vec![]).await;
	add(&store, &bank, vec![a.evidence()]).await;
	add(&store, &bank, vec![b.evidence()]).await;
	let changed = memory::mutate(
		&store,
		&Actor::Operator,
		Mutation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: bank.clone(),
			changes: [a.id, b.id]
				.into_iter()
				.map(|id| Change::Correct {
					id,
					expected_revision: 1,
					content: content("Batch corrected root"),
				})
				.collect(),
		},
	)
	.await
	.unwrap();
	assert_eq!(changed.len(), 2);
	let current = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			provider: reference("p"),
			bank,
		},
	)
	.await
	.unwrap();
	assert_eq!(current.len(), 2);
	assert!(
		current
			.iter()
			.all(|unit| unit.content.text == "Batch corrected root")
	);
}

#[rstest]
#[tokio::test]
async fn policy_replacement_cannot_shrink_below_existing_reverse_impact(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, registry, workspace) = setup(&database, bounds).await;
	let bank = bank(&store, workspace, false).await;
	let root = add(&store, &bank, vec![]).await;
	add(&store, &bank, vec![root.evidence()]).await;
	add(&store, &bank, vec![root.evidence()]).await;
	let mut provider = registry.get("p", "1.0.0").await.unwrap();
	provider.id = "small-impact".into();
	provider.config["policy"]["bounds"]["max_graph_visits"] = json!(2);
	registry.register(provider).await.unwrap();
	assert!(
		matches!(memory::operate(&store, &Actor::Operator, memory::Operation { operation_id: Uuid::now_v7(), provider: reference("small-impact"), bank: bank.clone(), action: memory::Action::ConfigureBank { expected_revision: 1 } }).await,
		Err(aidash_server::Error::Conflict(message)) if message.contains("dependency impact"))
	);
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Delete {
				id: root.id,
				expected_revision: 1,
			},
		),
	)
	.await
	.unwrap();
	assert!(
		memory::list(
			&store,
			&Actor::Operator,
			memory::ReadBank {
				provider: reference("p"),
				bank
			}
		)
		.await
		.unwrap()
		.is_empty()
	);
}

#[rstest]
#[tokio::test]
async fn recovery_shards_keep_home_ledgers_above_64_mib_writable(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use axum::{Json, Router, routing::post};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	struct Provider(tokio::task::JoinHandle<()>);
	impl Drop for Provider {
		fn drop(&mut self) {
			self.0.abort();
		}
	}
	let _provider = Provider(tokio::spawn(async move {
		axum::serve(listener, Router::new().route("/v1/embeddings", post(|Json(input): Json<serde_json::Value>| async move {
			Json(json!({"model":input["model"],"data":[{"index":0,"embedding":[1.,0.1,0.]}],"usage":{"prompt_tokens":1}}))
		}))).await.unwrap();
	}));
	let database = database.await;
	let (store, _, workspace) = setup_endpoint(&database, bounds, &endpoint).await;
	let directory = database.recovery_directory.path();
	let metadata = std::fs::read(directory.join("ledger.cbor")).unwrap();
	let ledger: recovery::Ledger = ciborium::de::from_reader(&metadata[40..]).unwrap();
	// Permanent floors can outlive uncertain database commits. Seed independent
	// bank identities with large valid tenant names to cross the real file cap
	// without hundreds of thousands of fsyncs or a lowered production limit.
	let mut aggregate = 0;
	for _ in 0..65 {
		let id = Uuid::now_v7();
		let path = directory.join("units").join(format!("{id}.cbor"));
		let fence = recovery::Fence {
			bank: Bank {
				home: store.node_id.clone(),
				tenant: "t".repeat(1024 * 1024),
				workspace: Uuid::now_v7(),
				participant: None,
			},
			revision: 1,
			deleted: true,
			digest: "permanent-deletion-fence".into(),
		};
		write_fixture_archive(
			&path,
			&RecoveryFence {
				epoch: ledger.epoch,
				id,
				fence,
			},
		);
		aggregate += std::fs::metadata(path).unwrap().len();
	}
	assert!(aggregate > 64 * 1024 * 1024);
	let bank = bank(&store, workspace, true).await;
	let admitted = add(&store, &bank, vec![]).await;
	assert_eq!(
		std::fs::read(directory.join("ledger.cbor")).unwrap(),
		metadata
	);
	let archive = memory_recovery::backup(&store, directory, bank.clone())
		.await
		.unwrap();
	let report = memory_recovery::restore(&store, directory, &archive)
		.await
		.unwrap();
	assert_eq!((report.restored, report.withheld), (1, 0));
	let changed = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Correct {
				id: admitted.id,
				expected_revision: 1,
				content: content("Corrected above the former Home cap"),
			},
		),
	)
	.await
	.unwrap();
	assert_eq!(changed[0].revision, 2);
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Delete {
				id: admitted.id,
				expected_revision: 2,
			},
		),
	)
	.await
	.unwrap();
	assert!(
		memory::list(
			&store,
			&Actor::Operator,
			memory::ReadBank {
				provider: reference("p"),
				bank
			}
		)
		.await
		.unwrap()
		.is_empty()
	);
	// A missing selected shard never falls back to a lower revision in metadata.
	std::fs::remove_file(
		directory
			.join("units")
			.join(format!("{}.cbor", admitted.id)),
	)
	.unwrap();
	assert!(
		memory_recovery::backup(&store, directory, admitted.bank)
			.await
			.is_err()
	);
}
