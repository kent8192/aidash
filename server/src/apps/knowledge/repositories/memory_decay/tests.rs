//! Repository tests verify the real Delivery savepoint, rather than a Usage-only mock.
use super::*;
use crate::apps::execution::test_database::{DatabaseFixture, database};
use crate::apps::knowledge::services::native_memory as memory;
use crate::apps::registry::models::Definition;
use crate::authorization::identity::Actor;
use reinhardt::db::orm::Model;
use reinhardt::query::QueryStatementBuilder;
use rstest::rstest;
use serde_json::json;
#[allow(dead_code)] // Shared fixture includes helpers used only by the integration binary.
#[path = "../../tests/native_memory/fixture.rs"]
mod fixture;
use fixture::*;

async fn run(store: &Store, bank: &Bank) -> Uuid {
	let task = store
		.create_task(
			bank.workspace,
			&aidash_domain::NewTask {
				title: "Local Delivery".into(),
				description: "Native Usage".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"operator",
			None,
		)
		.await
		.unwrap();
	let id = Uuid::now_v7();
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
				Expr::value(id),
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
				Expr::value(id),
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
	tx.commit().await.unwrap();
	id
}
async fn deliveries(store: &Store, id: Uuid) -> Option<i64> {
	native::query_scalar(
		&Query::select()
			.column(Alias::new("deliveries"))
			.from(Alias::new("memory_unit_retention"))
			.and_where(Expr::col("unit_id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&store.pool)
	.await
	.unwrap()
}

#[rstest]
#[tokio::test]
async fn local_usage_deduplicates_runs_across_revisions_and_rolls_back_with_failed_journal(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
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
	let id = Uuid::now_v7();
	let admitted = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("Usage follows the Unit identity"),
			},
		),
	)
	.await
	.unwrap();
	let first = run(&store, &bank).await;
	for _ in 0..2 {
		let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
		let result = super::super::memory_reads::record(&mut lease, first, &admitted).await;
		lease.finish(result).await.unwrap();
	}
	assert_eq!(deliveries(&store, id).await, Some(1));
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_unit_retention"))
			.value(
				Alias::new("dormant_policy"),
				serde_json::to_value(reference("p")).unwrap(),
			)
			.and_where(Expr::col("unit_id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await
	.unwrap();
	assert!(
		super::super::memory_reads::visible(&mut lease, first)
			.await
			.unwrap()
	);
	assert_eq!(
		units::load(&mut lease, id, false).await.unwrap().unwrap(),
		admitted[0]
	);
	lease.finish(Ok(())).await.unwrap();
	let corrected = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Correct {
				id,
				expected_revision: 1,
				content: content("Same Unit next revision"),
			},
		),
	)
	.await
	.unwrap();
	// A correction invalidates the old revision journal. The failed new Delivery
	// rolls back its Usage/reactivation update together with the attempted read.
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	assert!(
		super::super::memory_reads::record(&mut lease, first, &corrected)
			.await
			.is_err()
	);
	lease.finish(Ok(())).await.unwrap();
	assert_eq!(deliveries(&store, id).await, Some(1));
	let second = run(&store, &bank).await;
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	let bank_id = repository::bank_id(&mut lease, &bank, false)
		.await
		.unwrap()
		.unwrap();
	super::ensure(&mut lease, bank_id, id).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_unit_retention"))
			.value(
				Alias::new("dormant_policy"),
				serde_json::to_value(reference("p")).unwrap(),
			)
			.and_where(Expr::col("unit_id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await
	.unwrap();
	let result = super::super::memory_reads::record(&mut lease, second, &corrected).await;
	lease.finish(result).await.unwrap();
	assert_eq!(deliveries(&store, id).await, Some(2));
	// Rolling back the enclosing completed-Delivery transaction rolls back Usage too.
	let third = run(&store, &bank).await;
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	super::super::memory_reads::record(&mut lease, third, &corrected)
		.await
		.unwrap();
	let _: Result<()> = lease
		.finish(Err(Error::Conflict("abandoned delivery".into())))
		.await;
	assert_eq!(deliveries(&store, id).await, Some(2));
}

#[rstest]
#[tokio::test]
async fn person_only_actions_reject_agent_tokens_even_with_explicit_grants_and_dormant_reads_default_deny(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
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
	let id = Uuid::now_v7();
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("Person-controlled Pinning"),
			},
		),
	)
	.await
	.unwrap();
	let authorization = crate::authorization::Authorization {
		pool: store.pool.clone(),
	};
	let bundle = serde_json::from_value(json!({"tenant":"acme","subjects":{"agent":{"kind":"agent"},"reader":{"kind":"agent"},"person":{"kind":"user"},"admin":{"kind":"user"}},"policies":[
        {"id":"explicit-agent","effect":"allow","subjects":{"ids":["agent"]},"actions":["*"],"resources":{"kinds":["*"]}},
        {"id":"workspace-admin","effect":"allow","subjects":{"ids":["admin"]},"actions":["workspace.read","workspace.update","memory.read","memory.pin","memory.reactivate","registry.read"],"resources":{"kinds":["*"]}},
        {"id":"default-read","effect":"allow","subjects":{"ids":["reader","person"]},"actions":["workspace.read","memory.read","memory.pin","memory.reactivate","registry.read"],"resources":{"kinds":["*"]}}
    ]})).unwrap();
	authorization
		.replace("acme", 1, bundle, "operator")
		.await
		.unwrap();
	for role in ["p", "a", "m", "e", "r", "t"] {
		authorization
			.set_catalog("acme", &reference(role), 0, true, "operator")
			.await
			.unwrap();
	}
	for subject in ["agent", "reader", "person"] {
		let credential = authorization
			.issue_credential("acme", subject, 3600, "operator")
			.await
			.unwrap();
		let actor = authorization.authenticate(&credential.token).await.unwrap();
		if subject == "agent" {
			assert!(matches!(
				memory::operate(
					&store,
					&actor,
					memory::Operation {
						operation_id: Uuid::now_v7(),
						provider: reference("p"),
						bank: bank.clone(),
						action: memory::Action::Dormant,
					}
				)
				.await
				.unwrap(),
				memory::Outcome::Units(_)
			));
		}
		for action in [
			memory::Action::Pin { id },
			memory::Action::Unpin { id },
			memory::Action::Reactivate { id },
		] {
			let outcome = memory::operate(
				&store,
				&actor,
				memory::Operation {
					operation_id: Uuid::now_v7(),
					provider: reference("p"),
					bank: bank.clone(),
					action,
				},
			)
			.await;
			assert!(matches!(outcome, Err(Error::Forbidden)), "{}", subject);
		}
		if subject == "reader" {
			for action in [
				memory::Action::Dormant,
				memory::Action::RecallIncludingDormant {
					query: RecallQuery {
						text: "fixture".into(),
						time: None,
						kinds: vec![],
						max_tokens: 8192,
					},
				},
			] {
				assert!(matches!(
					memory::operate(
						&store,
						&actor,
						memory::Operation {
							operation_id: Uuid::now_v7(),
							provider: reference("p"),
							bank: bank.clone(),
							action
						}
					)
					.await,
					Err(Error::Forbidden)
				));
			}
		}
	}
	let credential = authorization
		.issue_credential("acme", "admin", 3600, "operator")
		.await
		.unwrap();
	let actor = authorization.authenticate(&credential.token).await.unwrap();
	for action in [
		memory::Action::Pin { id },
		memory::Action::Unpin { id },
		memory::Action::Reactivate { id },
	] {
		assert!(matches!(
			memory::operate(
				&store,
				&actor,
				memory::Operation {
					operation_id: Uuid::now_v7(),
					provider: reference("p"),
					bank: bank.clone(),
					action,
				}
			)
			.await
			.unwrap(),
			memory::Outcome::Units(_)
		));
	}
	// Federation's request contract has no operation/state-control extension.
	for field in ["read_dormant", "pin", "reactivate", "include_dormant"] {
		let mut request = json!({"participant": bank.participant,"expected_revision":1,"provider":reference("p")});
		request[field] = json!(true);
		assert!(
			serde_json::from_value::<aidash_domain::semantic::remote::NativeRequest>(request)
				.is_err()
		);
	}
}

#[rstest]
#[tokio::test]
async fn live_derived_support_reactivates_and_remains_evidence_even_when_dormant(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let d = Decay {
		half_life_days: 1,
		prior_floor_millionths: 0,
		dormancy: Some(Dormancy {
			threshold_millionths: 500_000,
			interval_hours: 1,
			batch: 1,
			include_preferences: false,
			include_procedures: false,
		}),
	};
	let (store, _, workspace) = setup_endpoint_decay(
		&database,
		bounds.clone(),
		"http://127.0.0.1:9/v1",
		(false, false, false),
		None,
		Some(d),
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
	let source = Uuid::now_v7();
	let admitted = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: source,
				content: content("Conflicting fact remains live support"),
			},
		),
	)
	.await
	.unwrap();
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	let bank_id = repository::bank_id(&mut lease, &bank, false)
		.await
		.unwrap()
		.unwrap();
	super::ensure(&mut lease, bank_id, source).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_unit_retention"))
			.value(
				Alias::new("dormant_policy"),
				serde_json::to_value(reference("p")).unwrap(),
			)
			.and_where(Expr::col("unit_id").eq(Expr::value(source)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await
	.unwrap();
	units::current(
		&mut lease,
		workspace,
		&[admitted[0].evidence()],
		bounds.max_graph_visits,
	)
	.await
	.unwrap();
	let mut body = content("Observation preserving its Dormant conflict");
	body.kind = Kind::Observation;
	body.evidence = vec![admitted[0].evidence()];
	let result = repository::mutate(
		&mut lease,
		&mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: body,
			},
		),
		&bounds,
	)
	.await;
	lease.finish(result).await.unwrap();
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	assert!(super::live_support(&mut lease, &admitted[0]).await.unwrap());
	let current_policy = memory::policy(&mut lease, &reference("p")).await.unwrap();
	assert_eq!(
		super::scores(
			&mut lease,
			&bank,
			&current_policy,
			&admitted,
			Utc::now() + Duration::days(365)
		)
		.await
		.unwrap()[&source],
		1.0
	);
	let snapshot = repository::list_mode(
		&mut lease,
		&bank,
		bounds.max_units,
		bounds.max_graph_visits,
		repository::ListMode::Recall,
	)
	.await
	.unwrap();
	assert_eq!(snapshot.len(), 2);
	lease.finish(Ok(())).await.unwrap();
}

#[rstest]
#[tokio::test]
async fn default_and_dormant_snapshots_have_separate_fail_closed_bounds_and_disabled_flags_stay_inert(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let d = Decay {
		half_life_days: 1,
		prior_floor_millionths: 0,
		dormancy: Some(Dormancy {
			threshold_millionths: 500_000,
			interval_hours: 1,
			batch: 1,
			include_preferences: false,
			include_procedures: false,
		}),
	};
	let (store, registry, workspace) = setup_endpoint_decay(
		&database,
		bounds.clone(),
		"http://127.0.0.1:9/v1",
		(false, false, false),
		None,
		Some(d),
	)
	.await;
	let bank = Bank {
		home: store.node_id.clone(),
		tenant: "acme".into(),
		workspace,
		participant: None,
	};
	let mut ids = vec![];
	for _ in 0..3 {
		let id = Uuid::now_v7();
		ids.push(id);
		memory::mutate(
			&store,
			&Actor::Operator,
			mutation(
				&bank,
				Change::Add {
					id,
					content: content("Independently bounded snapshot"),
				},
			),
		)
		.await
		.unwrap();
	}
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	let bank_id = repository::bank_id(&mut lease, &bank, false)
		.await
		.unwrap()
		.unwrap();
	for id in &ids[..2] {
		super::ensure(&mut lease, bank_id, *id).await.unwrap();
		native::query(
			&Query::update()
				.table(Alias::new("memory_unit_retention"))
				.value(
					Alias::new("dormant_policy"),
					serde_json::to_value(reference("p")).unwrap(),
				)
				.and_where(Expr::col("unit_id").eq(Expr::value(*id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await
		.unwrap();
	}
	let active = repository::list_mode(
		&mut lease,
		&bank,
		1,
		bounds.max_graph_visits,
		repository::ListMode::Recall,
	)
	.await
	.unwrap();
	assert_eq!(
		active.iter().map(|u| u.id).collect::<Vec<_>>(),
		vec![ids[2]]
	);
	assert!(matches!(
		repository::list_mode(
			&mut lease,
			&bank,
			1,
			bounds.max_graph_visits,
			repository::ListMode::Dormant
		)
		.await,
		Err(Error::Conflict(_))
	));
	assert!(matches!(
		repository::list(&mut lease, &bank, 1, bounds.max_graph_visits).await,
		Err(Error::Conflict(_))
	));
	assert!(matches!(
		repository::recall_including_dormant(&mut lease, &bank, 1, bounds.max_graph_visits).await,
		Err(Error::Conflict(_))
	));
	let (inclusive_active, inclusive_dormant) =
		repository::recall_including_dormant(&mut lease, &bank, 2, bounds.max_graph_visits)
			.await
			.unwrap();
	assert_eq!(
		inclusive_active
			.iter()
			.map(|unit| unit.id)
			.collect::<Vec<_>>(),
		vec![ids[2]]
	);
	assert_eq!(
		inclusive_dormant
			.iter()
			.map(|unit| unit.id)
			.collect::<Vec<_>>(),
		ids[..2]
	);
	lease.finish(Ok(())).await.unwrap();
	let mut disabled = registry.get("p", "1.0.0").await.unwrap();
	disabled.id = "disabled".into();
	disabled.config["policy"]["decay"] = json!(null);
	registry.register(disabled).await.unwrap();
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	bank_settings::set(&mut lease, &bank, &reference("disabled"), 1)
		.await
		.unwrap();
	assert_eq!(
		repository::list_mode(
			&mut lease,
			&bank,
			3,
			bounds.max_graph_visits,
			repository::ListMode::Recall
		)
		.await
		.unwrap()
		.len(),
		3
	);
	assert!(
		repository::list_mode(
			&mut lease,
			&bank,
			3,
			bounds.max_graph_visits,
			repository::ListMode::Dormant
		)
		.await
		.unwrap()
		.is_empty()
	);
	let flag: Option<serde_json::Value> = native::query_scalar(
		&Query::select()
			.column(Alias::new("dormant_policy"))
			.from(Alias::new("memory_unit_retention"))
			.and_where(Expr::col("unit_id").eq(Expr::value(ids[0])))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **lease.tx())
	.await
	.unwrap();
	assert_eq!(flag, Some(serde_json::to_value(reference("p")).unwrap()));
	lease.finish(Ok(())).await.unwrap();
}

#[rstest]
#[tokio::test]
async fn reactivation_resets_the_dormancy_anchor_without_incrementing_usage(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let d = Decay {
		half_life_days: 1,
		prior_floor_millionths: 0,
		dormancy: Some(Dormancy {
			threshold_millionths: 500_000,
			interval_hours: 1,
			batch: 4,
			include_preferences: false,
			include_procedures: false,
		}),
	};
	let (store, _, workspace) = setup_endpoint_decay(
		&database,
		bounds,
		"http://127.0.0.1:9/v1",
		(false, false, false),
		None,
		Some(d),
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
	let id = Uuid::now_v7();
	let admitted = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("Old Usage gets a fresh Reactivation interval"),
			},
		),
	)
	.await
	.unwrap();
	let old: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	let bank_id = repository::bank_id(&mut lease, &bank, false)
		.await
		.unwrap()
		.unwrap();
	super::ensure(&mut lease, bank_id, id).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_unit_retention"))
			.value(Alias::new("deliveries"), 5_i64)
			.value(Alias::new("last_delivered_at"), old)
			.value(
				Alias::new("dormant_policy"),
				serde_json::to_value(reference("p")).unwrap(),
			)
			.and_where(Expr::col("unit_id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await
	.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_bank_decay"))
			.value(Alias::new("activated_at"), old)
			.value(Alias::new("next_job"), old)
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await
	.unwrap();
	lease.finish(Ok(())).await.unwrap();
	let outcome = memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::Reactivate { id },
		},
	)
	.await
	.unwrap();
	let memory::Outcome::Units(reactivated) = outcome else {
		panic!("Units");
	};
	assert_eq!(reactivated, admitted);
	let reactivated_at: DateTime<Utc> = native::query_scalar(
		&Query::select()
			.column(Alias::new("reactivated_at"))
			.from(Alias::new("memory_unit_retention"))
			.and_where(Expr::col("unit_id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	for (as_of, expected_dormant) in [
		(reactivated_at + Duration::minutes(30), false),
		(reactivated_at + Duration::days(4), true),
	] {
		let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
		assert_eq!(super::page(&mut lease, bank_id, as_of).await.unwrap(), 1);
		let flag: Option<serde_json::Value> = native::query_scalar(
			&Query::select()
				.column(Alias::new("dormant_policy"))
				.from(Alias::new("memory_unit_retention"))
				.and_where(Expr::col("unit_id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **lease.tx())
		.await
		.unwrap();
		assert_eq!(flag.is_some(), expected_dormant);
		assert_eq!(
			units::load(&mut lease, id, false).await.unwrap().unwrap(),
			admitted[0]
		);
		lease.finish(Ok(())).await.unwrap();
	}
	assert_eq!(deliveries(&store, id).await, Some(5));
}

#[rstest]
#[tokio::test]
async fn first_dormancy_page_and_replay_share_database_clock_precision(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let decay = Decay {
		half_life_days: 1,
		prior_floor_millionths: 0,
		dormancy: Some(Dormancy {
			threshold_millionths: 500_000,
			interval_hours: 1,
			batch: 1,
			include_preferences: false,
			include_procedures: false,
		}),
	};
	let (store, _, workspace) = setup_endpoint_decay(
		&database,
		bounds,
		"http://127.0.0.1:9/v1",
		(false, false, false),
		None,
		Some(decay),
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
	let id = Uuid::now_v7();
	let admitted = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("Threshold decisions survive clock serialization"),
			},
		),
	)
	.await
	.unwrap();
	let anchor = Utc::now().trunc_subsecs(6);
	let as_of = anchor + Duration::hours(25) + Duration::nanoseconds(123);
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	let bank_id = repository::bank_id(&mut lease, &bank, false)
		.await
		.unwrap()
		.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_bank_decay"))
			.value(Alias::new("activated_at"), anchor)
			.value(Alias::new("next_job"), anchor)
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await
	.unwrap();
	assert_eq!(super::page(&mut lease, bank_id, as_of).await.unwrap(), 1);
	let persisted: DateTime<Utc> = native::query_scalar(
		&Query::select()
			.column(Alias::new("as_of"))
			.from(Alias::new("memory_bank_decay"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **lease.tx())
	.await
	.unwrap();
	assert_eq!(persisted, as_of.trunc_subsecs(6));
	lease.finish(Ok(())).await.unwrap();
	for replay in [false, true] {
		let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
		if replay {
			native::query(
				&Query::update()
					.table(Alias::new("memory_bank_decay"))
					.value(Alias::new("cursor"), None::<Uuid>)
					.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await
			.unwrap();
			assert_eq!(
				super::page(&mut lease, bank_id, as_of + Duration::hours(2))
					.await
					.unwrap(),
				1
			);
		}
		let flag: Option<serde_json::Value> = native::query_scalar(
			&Query::select()
				.column(Alias::new("dormant_policy"))
				.from(Alias::new("memory_unit_retention"))
				.and_where(Expr::col("unit_id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **lease.tx())
		.await
		.unwrap();
		// At the persisted cutoff r=0.5 exactly, so the strict threshold keeps it active.
		assert_eq!(flag, None);
		assert_eq!(
			units::load(&mut lease, id, false).await.unwrap().unwrap(),
			admitted[0]
		);
		lease.finish(Ok(())).await.unwrap();
	}
	assert_eq!(deliveries(&store, id).await, Some(0));
}

#[rstest]
#[tokio::test]
async fn batch_scores_preserve_exact_live_support_dormancy_and_missing_usage_rows(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let d = Decay {
		half_life_days: 1,
		prior_floor_millionths: 0,
		dormancy: Some(Dormancy {
			threshold_millionths: 500_000,
			interval_hours: 1,
			batch: 1,
			include_preferences: true,
			include_procedures: true,
		}),
	};
	let (store, _, workspace) = setup_endpoint_decay(
		&database,
		bounds.clone(),
		"http://127.0.0.1:9/v1",
		(false, false, false),
		None,
		Some(d),
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
	let mut sources = Vec::new();
	for text in [
		"Supported fact A",
		"Supported fact B",
		"An obsolete source",
		"Pinned independent fact",
	] {
		sources.push(
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Add {
						id: Uuid::now_v7(),
						content: content(text),
					},
				),
			)
			.await
			.unwrap()
			.remove(0),
		);
	}
	let mut supports = Vec::new();
	for source in &sources[..3] {
		let mut body = content("Live observation support");
		body.kind = Kind::Observation;
		body.evidence = vec![source.evidence()];
		// Derived Units enter through the trusted engine repository boundary.
		let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
		let result = repository::mutate(
			&mut lease,
			&mutation(
				&bank,
				Change::Add {
					id: Uuid::now_v7(),
					content: body,
				},
			),
			&bounds,
		)
		.await;
		supports.push(lease.finish(result).await.unwrap().remove(0));
	}
	sources[2] = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Correct {
				id: sources[2].id,
				expected_revision: 1,
				content: content("The current source has no current Observation"),
			},
		),
	)
	.await
	.unwrap()
	.remove(0);
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	let bank_id = repository::bank_id(&mut lease, &bank, false)
		.await
		.unwrap()
		.unwrap();
	for id in [sources[0].id, supports[0].id, sources[3].id] {
		super::ensure(&mut lease, bank_id, id).await.unwrap();
		native::query(
			&Query::update()
				.table(Alias::new("memory_unit_retention"))
				.value(
					Alias::new("dormant_policy"),
					serde_json::to_value(reference("p")).unwrap(),
				)
				.value(Alias::new("pinned"), id == sources[3].id)
				.and_where(Expr::col("unit_id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await
		.unwrap();
	}
	let policy = memory::policy(&mut lease, &reference("p")).await.unwrap();
	let scores = super::scores(
		&mut lease,
		&bank,
		&policy,
		&sources,
		Utc::now() + Duration::days(365),
	)
	.await
	.unwrap();
	assert_eq!(scores.len(), sources.len());
	assert_eq!(
		scores[&sources[0].id], 1.0,
		"Dormant support remains live evidence"
	);
	assert_eq!(
		scores[&sources[1].id], 1.0,
		"a never-delivered source can have live support"
	);
	assert!(
		scores[&sources[2].id] < 0.01,
		"an old-revision Observation cannot exempt its corrected source"
	);
	assert_eq!(
		scores[&sources[3].id], 1.0,
		"Pinned is independent from derived support"
	);
	lease.finish(Ok(())).await.unwrap();
	// A smaller replacement snapshot bound still applies to each source, not
	// the union of distinct supports needed by one inclusive recall.
	let mut definition = Definition::objects()
		.filter(Definition::field_id().eq("p".to_string()))
		.filter(Definition::field_version().eq("1.0.0".to_string()))
		.get_with_db(&mut database.lease.handle())
		.await
		.unwrap();
	for bound in ["max_units", "max_candidates", "max_results"] {
		definition.metadata.0["config"]["policy"]["bounds"][bound] = json!(1);
	}
	Definition::objects()
		.update_with_conn(&mut database.lease.handle(), &definition)
		.await
		.unwrap();
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	let policy = memory::policy(&mut lease, &reference("p")).await.unwrap();
	let scores = super::scores(
		&mut lease,
		&bank,
		&policy,
		&sources[..2],
		Utc::now() + Duration::days(365),
	)
	.await
	.unwrap();
	assert_eq!(scores.len(), 2);
	assert!(scores.values().all(|score| *score == 1.0));
	lease.finish(Ok(())).await.unwrap();
}

#[rstest]
#[tokio::test]
async fn inclusive_snapshot_keeps_both_units_when_retention_changes_during_hydration(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use reinhardt::query::prelude::IntoIden;
	use reinhardt::query::{ColumnRef, LockType, SimpleExpr};
	let database = database.await;
	let d = Decay {
		half_life_days: 1,
		prior_floor_millionths: 0,
		dormancy: Some(Dormancy {
			threshold_millionths: 500_000,
			interval_hours: 1,
			batch: 1,
			include_preferences: true,
			include_procedures: true,
		}),
	};
	let (store, _, workspace) = setup_endpoint_decay(
		&database,
		bounds.clone(),
		"http://127.0.0.1:9/v1",
		(false, false, false),
		None,
		Some(d),
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
	let ids = [Uuid::now_v7(), Uuid::now_v7()];
	let mut admitted = Vec::new();
	for id in ids {
		admitted.push(
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Add {
						id,
						content: content("Atomic inclusive membership"),
					},
				),
			)
			.await
			.unwrap()
			.remove(0),
		);
	}
	let mut setup = Lease::begin(&store, &Actor::Operator).await.unwrap();
	let bank_id = repository::bank_id(&mut setup, &bank, false)
		.await
		.unwrap()
		.unwrap();
	for id in ids {
		super::ensure(&mut setup, bank_id, id).await.unwrap();
	}
	native::query(
		&Query::update()
			.table(Alias::new("memory_unit_retention"))
			.value(Alias::new("dormant_policy"), json!(reference("p")))
			.and_where(Expr::col("unit_id").eq(Expr::value(ids[1])))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **setup.tx())
	.await
	.unwrap();
	setup.finish(Ok(())).await.unwrap();

	// Hold canonical hydration after the membership statement has completed.
	let mut writer = native::begin(&store.pool).await.unwrap();
	for id in ids {
		native::query(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("memory_units"))
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut *writer)
		.await
		.unwrap();
	}
	let (send, receive) = tokio::sync::oneshot::channel();
	let max_graph_visits = bounds.max_graph_visits;
	let reader = async {
		let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
		let pid: i32 = native::query_scalar(
			&Query::select()
				.expr(SimpleExpr::FunctionCall(
					Alias::new("pg_backend_pid").into_iden(),
					vec![],
				))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **lease.tx())
		.await
		.unwrap();
		send.send(pid).unwrap();
		let result =
			repository::recall_including_dormant(&mut lease, &bank, 1, max_graph_visits).await;
		lease.finish(result).await.unwrap()
	};
	let mutation = async {
		let pid = receive.await.unwrap();
		tokio::time::timeout(std::time::Duration::from_secs(30), async {
			loop {
				let row = native::query(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("pg_stat_activity"))
						.and_where(Expr::col("pid").eq(pid))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&store.pool)
				.await
				.unwrap();
				if row
					.try_get::<Option<String>>("wait_event_type")
					.unwrap()
					.as_deref() == Some("Lock")
				{
					break;
				}
				tokio::time::sleep(std::time::Duration::from_millis(20)).await;
			}
		})
		.await
		.expect("reader must reach the held Unit hydration lock");
		for (id, dormant) in [(ids[0], true), (ids[1], false)] {
			native::query(
				&Query::update()
					.table(Alias::new("memory_unit_retention"))
					.value(
						Alias::new("dormant_policy"),
						dormant.then(|| json!(reference("p"))),
					)
					.and_where(Expr::col("unit_id").eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *writer)
			.await
			.unwrap();
		}
		writer.commit().await.unwrap();
	};
	let ((active, dormant), ()) = tokio::join!(reader, mutation);
	assert_eq!(active, vec![admitted[0].clone()]);
	assert_eq!(dormant, vec![admitted[1].clone()]);
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	let (active, dormant) =
		repository::recall_including_dormant(&mut lease, &bank, 1, max_graph_visits)
			.await
			.unwrap();
	assert_eq!(active, vec![admitted[1].clone()]);
	assert_eq!(dormant, vec![admitted[0].clone()]);
	lease.finish(Ok(())).await.unwrap();
}

#[reinhardt::post("/v1/embeddings")]
async fn graph_embedding(
	reinhardt::Json(input): reinhardt::Json<serde_json::Value>,
) -> reinhardt::http::ViewResult<reinhardt::Response> {
	reinhardt::Response::ok().with_json(&json!({
		"model": input["model"],
		"data": [{"index": 0, "embedding": [1.0, 0.1, 0.0]}],
		"usage": {"prompt_tokens": 1}
	}))
}

#[rstest]
#[tokio::test]
async fn inclusive_semantic_graph_links_active_seeds_to_dormant_neighbors(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use crate::apps::knowledge::repositories::memory_scope::Scope;
	use crate::apps::knowledge::services::memory_models::Models;
	use aidash_application::ports::memory::MemoryScope;
	use reinhardt::ServerRouter;
	use reinhardt::test::fixtures::server::test_server_guard;
	let model = test_server_guard(ServerRouter::new().endpoint(graph_embedding)).await;
	let database = database.await;
	let d = Decay {
		half_life_days: 1,
		prior_floor_millionths: 0,
		dormancy: Some(Dormancy {
			threshold_millionths: 500_000,
			interval_hours: 1,
			batch: 1,
			include_preferences: true,
			include_procedures: true,
		}),
	};
	let (store, _, workspace) = setup_endpoint_decay(
		&database,
		bounds.clone(),
		&format!("{}/v1", model.url),
		(false, false, false),
		None,
		Some(d),
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
	let mut admitted = Vec::new();
	for text in [
		"Active semantic seed",
		"Dormant neighbor with distinct terms",
	] {
		admitted.push(
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Add {
						id: Uuid::now_v7(),
						content: content(text),
					},
				),
			)
			.await
			.unwrap()
			.remove(0),
		);
	}
	crate::semantic::worker::sweep(&store).await.unwrap();
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	let bank_id = repository::bank_id(&mut lease, &bank, false)
		.await
		.unwrap()
		.unwrap();
	super::ensure(&mut lease, bank_id, admitted[1].id)
		.await
		.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_unit_retention"))
			.value(Alias::new("dormant_policy"), json!(reference("p")))
			.and_where(Expr::col("unit_id").eq(Expr::value(admitted[1].id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await
	.unwrap();
	lease.finish(Ok(())).await.unwrap();
	let mut definition = Definition::objects()
		.filter(Definition::field_id().eq("p".to_string()))
		.filter(Definition::field_version().eq("1.0.0".to_string()))
		.get_with_db(&mut database.lease.handle())
		.await
		.unwrap();
	for bound in ["max_units", "max_candidates", "max_results"] {
		definition.metadata.0["config"]["policy"]["bounds"][bound] = json!(1);
	}
	Definition::objects()
		.update_with_conn(&mut database.lease.handle(), &definition)
		.await
		.unwrap();
	let mut lease = Lease::begin(&store, &Actor::Operator).await.unwrap();
	let policy = memory::policy(&mut lease, &reference("p")).await.unwrap();
	let models = Models::resolve(
		&store,
		&mut lease,
		reference("p"),
		bank.clone(),
		policy.clone(),
		Uuid::now_v7(),
		"inclusive-semantic-graph-fixture".into(),
		None,
	)
	.await
	.unwrap();
	let mut delivered = Vec::new();
	let snapshot = Scope {
		store: &store,
		lease: &mut lease,
		models: &models,
		delivered: &mut delivered,
	}
	.recall_including_dormant_snapshot(&bank, 1)
	.await
	.unwrap();
	assert_eq!(policy.bounds.max_units, 1);
	assert_eq!(
		snapshot.units.len(),
		2,
		"inclusive bounds apply independently"
	);
	assert_eq!(snapshot.units, admitted);
	assert!(
		snapshot
			.graph
			.iter()
			.any(|edge| edge.source == admitted[0].id
				&& edge.source_revision == admitted[0].revision
				&& edge.target.target == admitted[1].id
				&& edge.target.revision == admitted[1].revision
				&& edge.target.kind == LinkKind::Semantic)
	);
	let found = recall::graph_with_edges(
		&snapshot.units,
		&[admitted[0].id],
		&snapshot.graph,
		&policy.bounds,
	);
	assert!(
		found.contains(&admitted[1].id),
		"the capped semantic seed must discover its Dormant neighbor through the graph arm"
	);
	lease.finish(Ok(())).await.unwrap();
}
