//! Replacement policies and maintenance keep bank and provider bounds consistent.
use super::*;
use aidash_server::{apps::registry::models::Definition, database::native};

async fn config(database: &DatabaseFixture) -> serde_json::Value {
	Definition::objects()
		.filter(Definition::field_id().eq("p".to_string()))
		.filter(Definition::field_version().eq("1.0.0".to_string()))
		.get_with_db(&mut database.lease.handle())
		.await
		.unwrap()
		.metadata
		.0["config"]
		.clone()
}

#[rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
#[tokio::test]
async fn smaller_storage_policy_rolls_back_shared_changes_and_participant_upgrades(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] participant: bool,
	#[case] record_cap: bool,
) {
	let database = database.await;
	let (store, registry, workspace) = setup(&database, bounds).await;
	let bank = if participant {
		memory::create_participant(
			&store,
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
		Bank {
			home: store.node_id.clone(),
			tenant: "acme".into(),
			workspace,
			participant: None,
		}
	};
	let mut ids = Vec::new();
	for _ in 0..3 {
		let id = Uuid::now_v7();
		memory::mutate(
			&store,
			&Actor::Operator,
			mutation(
				&bank,
				Change::Add {
					id,
					content: content("Storage bound fixture"),
				},
			),
		)
		.await
		.unwrap();
		ids.push(id);
	}
	if record_cap {
		for id in &ids[..2] {
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Delete {
						id: *id,
						expected_revision: 1,
					},
				),
			)
			.await
			.unwrap();
		}
	}
	let mut replacement = config(&database).await;
	let limits = &mut replacement["policy"]["bounds"];
	limits["max_units"] = json!(1);
	limits["max_candidates"] = json!(1);
	limits["max_results"] = json!(1);
	if record_cap {
		replacement["policy"]["retention"]["max_unit_records"] = json!(2);
	}
	registry
		.register(entry("memory", "small", replacement))
		.await
		.unwrap();
	let settings = || memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		action: memory::Action::Settings,
	};
	let memory::Outcome::Settings(Some(before)) =
		memory::operate(&store, &Actor::Operator, settings())
			.await
			.unwrap()
	else {
		panic!("existing settings");
	};
	let result = if participant {
		let mut agent = entry(
			"agent",
			"a",
			json!({"model":reference("m"),"instructions":"Use bounded memory",
			"tools":[],"skills":[],"memory":reference("small"),"allow_memory_write":true}),
		);
		agent.version = "1.1.0".into();
		registry.register(agent).await.unwrap();
		memory::upgrade_participant(
			&store,
			&Actor::Operator,
			bank.clone(),
			memory::UpgradeParticipant {
				agent: EntityRef {
					id: "a".into(),
					version: "1.1.0".into(),
				},
				expected_revision: 1,
			},
		)
		.await
		.map(|_| ())
	} else {
		memory::operate(
			&store,
			&Actor::Operator,
			memory::Operation {
				operation_id: Uuid::now_v7(),
				provider: reference("small"),
				bank: bank.clone(),
				action: memory::Action::ConfigureBank {
					expected_revision: before.revision,
				},
			},
		)
		.await
		.map(|_| ())
	};
	assert!(
		matches!(result, Err(aidash_server::Error::Conflict(message))
		if message == "replacement memory policy is below existing bank storage")
	);
	let memory::Outcome::Settings(Some(after)) =
		memory::operate(&store, &Actor::Operator, settings())
			.await
			.unwrap()
	else {
		panic!("retained settings");
	};
	assert_eq!(after.provider, before.provider);
	assert_eq!(after.revision, before.revision);
	if participant {
		// The failed provider replacement must roll back the Agent CAS as well.
		let still_current = memory::upgrade_participant(
			&store,
			&Actor::Operator,
			bank.clone(),
			memory::UpgradeParticipant {
				agent: reference("a"),
				expected_revision: 1,
			},
		)
		.await
		.unwrap();
		assert_eq!(still_current.revision, 2);
		assert_eq!(still_current.agent, reference("a"));
	}
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Delete {
				id: ids[2],
				expected_revision: 1,
			},
		),
	)
	.await
	.unwrap();
}

#[rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
#[tokio::test]
async fn smaller_operation_policy_preserves_durable_capacity_and_participant_revision(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
	#[case] participant: bool,
	#[case] model_operations: bool,
) {
	// Admit the model's full output reservation before injecting transport failure.
	bounds.max_model_tokens = 32_768;
	let database = database.await;
	let (store, registry, workspace) = setup(&database, bounds.clone()).await;
	let bank = if participant {
		memory::create_participant(
			&store,
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
		Bank {
			home: store.node_id.clone(),
			tenant: "acme".into(),
			workspace,
			participant: None,
		}
	};
	let id = Uuid::now_v7();
	let admitted = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("Operation capacity fixture"),
			},
		),
	)
	.await
	.unwrap()
	.remove(0);
	if model_operations {
		// Failed model calls still retain durable operation identities and charges.
		for _ in 0..3 {
			assert!(
				memory::operate(
					&store,
					&Actor::Operator,
					memory::Operation {
						operation_id: Uuid::now_v7(),
						provider: reference("p"),
						bank: bank.clone(),
						action: memory::Action::Retain {
							text: "Operation capacity fixture".into(),
							evidence: vec![admitted.evidence()],
						},
					}
				)
				.await
				.is_err()
			);
		}
	} else {
		for revision in 1..=2 {
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Correct {
						id,
						expected_revision: revision,
						content: content("Corrected capacity fixture"),
					},
				),
			)
			.await
			.unwrap();
		}
	}
	let table = if model_operations {
		"memory_model_operations"
	} else {
		"memory_receipts"
	};
	let count: i64 = native::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(Expr::col("bank_id").into()))
			.from(Alias::new(table))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(
		count, 3,
		"three durable operations must precede the policy change"
	);
	let mut replacement = config(&database).await;
	replacement["policy"]["retention"]["max_model_operations"] = json!(2);
	registry
		.register(entry("memory", "small", replacement))
		.await
		.unwrap();
	let settings = || memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		action: memory::Action::Settings,
	};
	let memory::Outcome::Settings(Some(before)) =
		memory::operate(&store, &Actor::Operator, settings())
			.await
			.unwrap()
	else {
		panic!("existing settings")
	};
	let result = if participant {
		let mut agent = entry(
			"agent",
			"a",
			json!({"model":reference("m"),"instructions":"Use bounded memory","tools":[],"skills":[],"memory":reference("small"),"allow_memory_write":true}),
		);
		agent.version = "1.1.0".into();
		registry.register(agent).await.unwrap();
		memory::upgrade_participant(
			&store,
			&Actor::Operator,
			bank.clone(),
			memory::UpgradeParticipant {
				agent: EntityRef {
					id: "a".into(),
					version: "1.1.0".into(),
				},
				expected_revision: 1,
			},
		)
		.await
		.map(|_| ())
	} else {
		memory::operate(
			&store,
			&Actor::Operator,
			memory::Operation {
				operation_id: Uuid::now_v7(),
				provider: reference("small"),
				bank: bank.clone(),
				action: memory::Action::ConfigureBank {
					expected_revision: before.revision,
				},
			},
		)
		.await
		.map(|_| ())
	};
	assert!(
		matches!(result, Err(aidash_server::Error::Conflict(message)) if message == "replacement memory policy is below existing bank storage")
	);
	let memory::Outcome::Settings(Some(after)) =
		memory::operate(&store, &Actor::Operator, settings())
			.await
			.unwrap()
	else {
		panic!("retained settings")
	};
	assert_eq!(
		(after.provider, after.revision),
		(before.provider, before.revision)
	);
	if participant {
		let retained = memory::upgrade_participant(
			&store,
			&Actor::Operator,
			bank.clone(),
			memory::UpgradeParticipant {
				agent: reference("a"),
				expected_revision: 1,
			},
		)
		.await
		.unwrap();
		assert_eq!(retained.agent, reference("a"));
	}
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Delete {
				id,
				expected_revision: if model_operations { 1 } else { 3 },
			},
		),
	)
	.await
	.unwrap();
}

#[rstest]
#[tokio::test]
async fn expiry_drains_in_batches_no_larger_than_mutation_capacity(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
) {
	bounds.max_candidates = 2;
	bounds.max_results = 2;
	let database = database.await;
	let (store, _, workspace) = setup_endpoint_retention(
		&database,
		bounds,
		"http://127.0.0.1:9/v1",
		(false, false, false),
		Some(1),
	)
	.await;
	let bank = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let mut units = Vec::new();
	for _ in 0..5 {
		units.extend(
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Add {
						id: Uuid::now_v7(),
						content: content("Expired batch fixture"),
					},
				),
			)
			.await
			.unwrap(),
		);
	}
	// Inject the fixture clock into both canonical bodies and independent fences.
	let mut tx = native::begin(&store.pool).await.unwrap();
	for unit in &mut units {
		unit.learned_at = chrono::DateTime::from_timestamp_micros(
			(chrono::Utc::now() - chrono::Duration::days(2)).timestamp_micros(),
		)
		.unwrap();
		let fence_path = database
			.recovery_directory
			.path()
			.join("units")
			.join(format!("{}.cbor", unit.id));
		let bytes = std::fs::read(&fence_path).unwrap();
		let mut record: RecoveryFence = ciborium::de::from_reader(&bytes[40..]).unwrap();
		record.fence.digest = aidash_domain::memory::recovery::digest(unit).unwrap();
		write_fixture_archive(&fence_path, &record);
		native::query(
			&Query::update()
				.table(Alias::new("memory_units"))
				.value(Alias::new("learned_at"), unit.learned_at)
				.and_where(Expr::col("id").eq(Expr::value(unit.id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
	}
	tx.commit().await.unwrap();
	for expected in [2, 4, 5] {
		let mut tx = native::begin(&store.pool).await.unwrap();
		native::query(
			&Query::update()
				.table(Alias::new("memory_bank_settings"))
				.value(
					Alias::new("next_maintenance"),
					chrono::Utc::now() - chrono::Duration::seconds(1),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
		tx.commit().await.unwrap();
		aidash_server::semantic::worker::sweep(&store)
			.await
			.unwrap();
		let count: i64 = native::query_scalar(
			&Query::select()
				.expr(reinhardt::query::Func::count(
					Expr::col(reinhardt::query::ColumnRef::Asterisk).into(),
				))
				.from(Alias::new("memory_units"))
				.and_where(Expr::col("deleted").eq(true))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&store.pool)
		.await
		.unwrap();
		assert_eq!(count, expected);
	}
}

#[rstest]
#[tokio::test]
async fn semantic_sources_allow_the_pinned_provenance_limit_above_1024(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
) {
	use axum::{Json, Router, routing::post};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, Router::new().route("/v1/embeddings", post(|Json(input): Json<serde_json::Value>| async move {
			Json(json!({"model":input["model"],"data":[{"index":0,"embedding":[1.,0.1,0.]}],"usage":{"prompt_tokens":1}}))
		}))).await.unwrap();
	});
	bounds.max_units = 32;
	bounds.max_graph_visits = 4096;
	let database = database.await;
	let (store, _, workspace) = setup_endpoint(&database, bounds, &endpoint).await;
	let bank = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let mut support = Vec::new();
	let mut last = Uuid::nil();
	// A small DAG requires 2046 exact support visits at its last layer.
	for layer in 0..11 {
		let mut next = Vec::new();
		for _ in 0..2 {
			let mut body = content("Pinned semantic provenance budget");
			if layer > 0 {
				body.kind = Kind::World;
				body.evidence = support.clone();
			}
			last = Uuid::now_v7();
			let unit = memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Add {
						id: last,
						content: body,
					},
				),
			)
			.await
			.unwrap()
			.remove(0);
			next.push(unit.evidence());
		}
		support = next;
	}
	// Old semantic source reads fail here even though admission accepts the DAG.
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let entries = aidash_server::semantic::service::entries(&store, &Actor::Operator, workspace)
		.await
		.unwrap();
	assert_eq!(
		entries.iter().find(|entry| entry.id == last).unwrap().state,
		"READY"
	);
	assert_large_graph_run_is_readable(&store, &bank, last).await;
	server.abort();
}

#[rstest]
#[tokio::test]
async fn deleting_an_unprojected_unit_does_not_consume_full_search_capacity(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, indexed) = setup(&database, bounds).await;
	let workspace = store
		.create_workspace("Late index", "Deletion must remain usable")
		.await
		.unwrap()
		.id;
	AuthorizationWorkspace::objects()
		.create_with_conn(
			&mut database.lease.handle(),
			&AuthorizationWorkspace::build()
				.workspace_id(workspace)
				.tenant("acme")
				.owner_subject("operator")
				.finish(),
		)
		.await
		.unwrap();
	let configured = aidash_server::semantic::service::get_index(&store, &Actor::Operator, indexed)
		.await
		.unwrap();
	let mut spec = configured.configuration().unwrap();
	spec.max_sources = 1;
	aidash_server::semantic::service::configure(
		&store,
		workspace,
		aidash_server::semantic::ConfigureIndex {
			expected_revision: 0,
			spec,
		},
	)
	.await
	.unwrap();
	let bank = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let old = Uuid::now_v7();
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: old,
				content: content("Admitted before its projection was lost"),
			},
		),
	)
	.await
	.unwrap();
	// Simulate a missing projection after valid bank admission. New banks now
	// require an enabled matching index, while deletion must still repair gaps.
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::delete()
			.from_table(Alias::new("semantic_points"))
			.and_where(Expr::col("entry_id").eq(Expr::value(old)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	native::query(
		&Query::delete()
			.from_table(Alias::new("semantic_entries"))
			.and_where(Expr::col("id").eq(Expr::value(old)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let current = Uuid::now_v7();
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: current,
				content: content("The only visible search source"),
			},
		),
	)
	.await
	.unwrap();
	let deleted = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Delete {
				id: old,
				expected_revision: 1,
			},
		),
	)
	.await
	.unwrap();
	assert!(deleted[0].deleted);
	let entries = aidash_server::semantic::service::entries(&store, &Actor::Operator, workspace)
		.await
		.unwrap();
	assert_eq!(entries.iter().filter(|entry| !entry.deleted).count(), 1);
	let tombstone = aidash_server::apps::knowledge::models::SemanticEntry::objects()
		.get(old)
		.get_with_db(&mut database.lease.handle())
		.await
		.unwrap();
	assert!(tombstone.deleted);
	assert_eq!(
		tombstone.state,
		aidash_server::apps::knowledge::models::states::SemanticEntryState::Deleted
	);
	let jobs: i64 = native::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(
				Expr::col(reinhardt::query::ColumnRef::Asterisk).into(),
			))
			.from(Alias::new("memory_purge_jobs"))
			.and_where(Expr::col("unit_id").eq(Expr::value(old)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(jobs, 1);
	let denied = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("Still over the visible source quota"),
			},
		),
	)
	.await;
	assert!(
		matches!(denied, Err(aidash_server::Error::Conflict(message)) if message == "Workspace search source limit reached")
	);
}

async fn assert_large_graph_run_is_readable(store: &Store, bank: &Bank, unit: Uuid) {
	let (scope, run) = recorded_run(store, bank, &[unit]).await;
	assert!(run_visible(&scope, store, run).await);
}

pub(super) async fn recorded_run(
	store: &Store,
	bank: &Bank,
	units: &[Uuid],
) -> (aidash_server::authorization::workspace::Workspaces, Uuid) {
	// Workspace inspection now resolves built-in capabilities through Registry.
	aidash_server::registry::Registry::new(store.pool.clone(), &store.node_id)
		.unwrap()
		.seed_system()
		.await
		.unwrap();
	let authorization = aidash_server::authorization::Authorization {
		pool: store.pool.clone(),
	};
	let owner = aidash_domain::qualified_agent(&store.node_id, "a", "1.0.0");
	authorization.replace("acme", 1, serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},owner:{"kind":"agent"}},"policies":[{"id":"read","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]})).unwrap(), "operator").await.unwrap();
	for id in ["m", "e", "r", "t", "p", "a"] {
		authorization
			.set_catalog("acme", &reference(id), 0, true, "operator")
			.await
			.unwrap();
	}
	let credential = authorization
		.issue_credential("acme", "alice", 3600, "operator")
		.await
		.unwrap();
	let Actor::Subject(identity) = authorization.authenticate(&credential.token).await.unwrap()
	else {
		panic!("subject");
	};
	let task = store
		.create_task(
			bank.workspace,
			&aidash_domain::NewTask {
				title: "Large provenance Run".into(),
				description: "Read the admitted graph".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"operator",
			None,
		)
		.await
		.unwrap();
	let run = Uuid::now_v7();
	let pending = aidash_domain::run_state::encode(
		&aidash_domain::run_state::RunState::Completed(aidash_domain::run_state::TerminalState {}),
		&aidash_domain::run_state::RecoveryState::default(),
	)
	.unwrap();
	let mut tx = native::begin(&store.pool).await.unwrap();
	for (table, columns, values) in [
		(
			"runs",
			vec![
				"id",
				"task_id",
				"workspace_id",
				"home_node",
				"agent_id",
				"agent_version",
				"phase",
				"pending",
				"revision",
			],
			vec![
				Expr::value(run),
				Expr::value(task.id),
				Expr::value(bank.workspace),
				Expr::value(&store.node_id),
				Expr::value("a"),
				Expr::value("1.0.0"),
				Expr::value("COMPLETED"),
				Expr::value(pending),
				Expr::value(1_i64),
			],
		),
		(
			"memory_run_bindings",
			vec![
				"run_id",
				"participant_id",
				"participant_revision",
				"provider_id",
				"provider_version",
			],
			vec![
				Expr::value(run),
				Expr::value(bank.participant.unwrap()),
				Expr::value(1_i64),
				Expr::value("p"),
				Expr::value("1.0.0"),
			],
		),
	] {
		let mut row = Query::select();
		for value in values {
			row.expr(value);
		}
		native::query(
			&Query::insert()
				.into_table(Alias::new(table))
				.columns(columns.into_iter().map(Alias::new))
				.from_subquery(row)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
	}
	for unit in units {
		native::query(
			&Query::insert()
				.into_table(Alias::new("memory_run_reads"))
				.columns(["run_id", "unit_id", "revision"].map(Alias::new))
				.from_subquery(
					Query::select()
						.expr(Expr::value(run))
						.expr(Expr::value(*unit))
						.expr(Expr::value(1_i64))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
	}

	tx.commit().await.unwrap();
	let scope = aidash_server::authorization::workspace::Workspaces {
		store: store.clone(),
		identity,
	};
	(scope, run)
}

pub(super) async fn run_visible(
	scope: &aidash_server::authorization::workspace::Workspaces,
	store: &Store,
	run: Uuid,
) -> bool {
	let state = scope
		.state(aidash_server::config::NodeIdentity {
			id: store.node_id.clone(),
			endpoint: "http://127.0.0.1:9".into(),
			capabilities: vec![],
			clusters: vec![],
			protocol_version: "0.1".into(),
		})
		.await
		.unwrap();
	state.runs.iter().any(|inspection| inspection.id == run)
}

#[rstest]
#[case(false, "max_unit_bytes")]
#[case(true, "max_unit_bytes")]
#[case(false, "max_entities")]
#[case(true, "max_entities")]
#[case(false, "max_evidence")]
#[case(true, "max_evidence")]
#[case(false, "max_links")]
#[case(true, "max_links")]
#[tokio::test]
async fn smaller_content_policy_rejects_existing_units_atomically(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] participant: bool,
	#[case] limit: &str,
) {
	let database = database.await;
	let (store, registry, workspace) = setup(&database, bounds).await;
	let bank = if participant {
		memory::create_participant(
			&store,
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
		Bank {
			home: store.node_id.clone(),
			tenant: "acme".into(),
			workspace,
			participant: None,
		}
	};
	let mut parents = Vec::new();
	for _ in 0..2 {
		parents.push(
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Add {
						id: Uuid::now_v7(),
						content: content("Supporting unit"),
					},
				),
			)
			.await
			.unwrap()
			.remove(0),
		);
	}
	let mut body = content("Content exceeding a replacement policy");
	match limit {
		"max_entities" => {
			body.entities = ["Alice", "Bob"]
				.into_iter()
				.map(|name| Entity {
					name: name.into(),
					category: "person".into(),
					aliases: vec![],
				})
				.collect()
		}
		"max_evidence" => body.evidence = parents.iter().map(Unit::evidence).collect(),
		"max_links" => {
			body.links = parents
				.iter()
				.map(|unit| Link {
					target: unit.id,
					revision: unit.revision,
					kind: LinkKind::Causes,
					weight: 1.0,
				})
				.collect()
		}
		_ => {}
	}
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: body,
			},
		),
	)
	.await
	.unwrap();
	let before = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			provider: reference("p"),
			bank: bank.clone(),
		},
	)
	.await
	.unwrap();
	let mut replacement = config(&database).await;
	replacement["policy"]["bounds"][limit] = json!(if limit == "max_unit_bytes" { 32 } else { 1 });
	registry
		.register(entry("memory", "small-content", replacement))
		.await
		.unwrap();
	let settings = || memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		action: memory::Action::Settings,
	};
	let memory::Outcome::Settings(Some(original)) =
		memory::operate(&store, &Actor::Operator, settings())
			.await
			.unwrap()
	else {
		panic!("settings");
	};
	let result = if participant {
		let mut agent = entry(
			"agent",
			"a",
			json!({"model":reference("m"),"instructions":"Use bounded memory","memory":reference("small-content"),"allow_memory_write":true}),
		);
		agent.version = "1.1.0".into();
		registry.register(agent).await.unwrap();
		memory::upgrade_participant(
			&store,
			&Actor::Operator,
			bank.clone(),
			memory::UpgradeParticipant {
				agent: EntityRef {
					id: "a".into(),
					version: "1.1.0".into(),
				},
				expected_revision: 1,
			},
		)
		.await
		.map(|_| ())
	} else {
		memory::operate(
			&store,
			&Actor::Operator,
			memory::Operation {
				operation_id: Uuid::now_v7(),
				provider: reference("small-content"),
				bank: bank.clone(),
				action: memory::Action::ConfigureBank {
					expected_revision: original.revision,
				},
			},
		)
		.await
		.map(|_| ())
	};
	assert!(
		matches!(result, Err(aidash_server::Error::Conflict(message)) if message == "replacement memory policy is below existing unit content bounds")
	);
	let memory::Outcome::Settings(Some(after)) =
		memory::operate(&store, &Actor::Operator, settings())
			.await
			.unwrap()
	else {
		panic!("settings");
	};
	assert_eq!(
		(after.provider, after.revision),
		(original.provider, original.revision)
	);
	assert_eq!(
		memory::list(
			&store,
			&Actor::Operator,
			memory::ReadBank {
				provider: reference("p"),
				bank: bank.clone()
			}
		)
		.await
		.unwrap(),
		before
	);
	if participant {
		let unchanged = memory::upgrade_participant(
			&store,
			&Actor::Operator,
			bank,
			memory::UpgradeParticipant {
				agent: reference("a"),
				expected_revision: 1,
			},
		)
		.await
		.unwrap();
		assert_eq!(unchanged.agent, reference("a"));
	}
}
