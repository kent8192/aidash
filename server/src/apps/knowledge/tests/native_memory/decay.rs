//! Real PostgreSQL regressions for independent recall state and bounded maintenance.
use super::*;
use aidash_server::database::native;
use chrono::{Duration, SubsecRound, Utc};

fn policy() -> Decay {
	Decay {
		half_life_days: 1,
		prior_floor_millionths: 100_000,
		dormancy: Some(Dormancy {
			threshold_millionths: 500_000,
			interval_hours: 1,
			batch: 1,
			include_preferences: false,
			include_procedures: false,
		}),
	}
}
async fn participant(store: &Store, workspace: Uuid) -> Bank {
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
}
async fn operate(store: &Store, bank: &Bank, action: memory::Action) -> memory::Outcome {
	memory::operate(
		store,
		&Actor::Operator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: bank.clone(),
			action,
		},
	)
	.await
	.unwrap()
}
async fn dormant(store: &Store, bank: &Bank) -> Vec<Unit> {
	let memory::Outcome::Units(units) = operate(store, bank, memory::Action::Dormant).await else {
		panic!("units");
	};
	units
}
async fn id(store: &Store, bank: &Bank) -> Uuid {
	native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_banks"))
			.and_where(Expr::col("participant_id").eq(Expr::value(bank.participant.unwrap())))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap()
}
async fn clock(store: &Store, bank: &Bank, as_of: chrono::DateTime<Utc>, cursor: Option<Uuid>) {
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_bank_decay"))
			.value(Alias::new("as_of"), as_of)
			.value(Alias::new("cursor"), cursor)
			.value(Alias::new("next_job"), Utc::now() - Duration::hours(1))
			.and_where(Expr::col("bank_id").eq(Expr::value(id(store, bank).await)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
}
async fn sweep(store: &Store) {
	aidash_server::semantic::worker::sweep(store).await.unwrap();
}

#[rstest]
#[tokio::test]
async fn retention_controls_replay_without_renewing_reactivation_and_reject_reused_ids(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup_endpoint_decay(
		&database,
		bounds,
		"http://127.0.0.1:9/v1",
		(false, false, false),
		None,
		Some(policy()),
	)
	.await;
	let bank = participant(&store, workspace).await;
	let ids = [Uuid::now_v7(), Uuid::now_v7()];
	let mut admission = mutation(
		&bank,
		Change::Add {
			id: ids[0],
			content: content("Retention controls preserve this canonical Unit"),
		},
	);
	admission.changes.push(Change::Add {
		id: ids[1],
		content: content("A different Unit cannot reuse the same control request"),
	});
	let admitted = memory::mutate(&store, &Actor::Operator, admission)
		.await
		.unwrap();
	let anchor = (Utc::now() - Duration::days(3)).trunc_subsecs(6);
	for (action, pinned) in [
		(memory::Action::Pin { id: ids[0] }, true),
		(memory::Action::Unpin { id: ids[0] }, false),
		(memory::Action::Reactivate { id: ids[0] }, false),
	] {
		let request = memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: bank.clone(),
			action,
		};
		let memory::Outcome::Units(result) =
			memory::operate(&store, &Actor::Operator, request.clone())
				.await
				.unwrap()
		else {
			panic!("control result");
		};
		assert_eq!(result, vec![admitted[0].clone()]);
		// Model a later dormancy sweep between the first response and a retry.
		// A receipt replay must neither reactivate again nor extend the anchor.
		let mut tx = native::begin(&store.pool).await.unwrap();
		native::query(
			&Query::update()
				.table(Alias::new("memory_unit_retention"))
				.value(Alias::new("reactivated_at"), anchor)
				.value(
					Alias::new("dormant_policy"),
					serde_json::to_value(reference("p")).unwrap(),
				)
				.and_where(Expr::col("unit_id").eq(Expr::value(ids[0])))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
		tx.commit().await.unwrap();
		let memory::Outcome::Units(replayed) =
			memory::operate(&store, &Actor::Operator, request.clone())
				.await
				.unwrap()
		else {
			panic!("control replay");
		};
		assert_eq!(replayed, result);
		for action in [
			memory::Action::Reactivate { id: ids[1] },
			if pinned {
				memory::Action::Unpin { id: ids[0] }
			} else {
				memory::Action::Pin { id: ids[0] }
			},
		] {
			assert!(matches!(
				memory::operate(
					&store,
					&Actor::Operator,
					memory::Operation {
						action,
						..request.clone()
					}
				)
				.await,
				Err(aidash_server::Error::Conflict(_))
			));
		}
		let row = native::query(
			&Query::select()
				.columns(["reactivated_at", "pinned", "dormant_policy"].map(Alias::new))
				.from(Alias::new("memory_unit_retention"))
				.and_where(Expr::col("unit_id").eq(Expr::value(ids[0])))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&store.pool)
		.await
		.unwrap();
		assert_eq!(
			row.try_get::<chrono::DateTime<Utc>>("reactivated_at")
				.unwrap(),
			anchor
		);
		assert_eq!(row.try_get::<bool>("pinned").unwrap(), pinned);
		assert_eq!(
			row.try_get::<serde_json::Value>("dormant_policy").unwrap(),
			json!(reference("p"))
		);
		let receipt: Vec<Evidence> = native::query_scalar(
			&Query::select()
				.column(Alias::new("outcome"))
				.from(Alias::new("memory_receipts"))
				.and_where(Expr::col("operation_id").eq(Expr::value(request.operation_id)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&store.pool)
		.await
		.unwrap();
		assert_eq!(receipt, vec![admitted[0].evidence()]);
	}
	assert!(matches!(
		memory::operate(
			&store,
			&Actor::Operator,
			memory::Operation {
				operation_id: Uuid::nil(),
				provider: reference("p"),
				bank,
				action: memory::Action::Reactivate { id: ids[0] },
			}
		)
		.await,
		Err(aidash_server::Error::Invalid(_))
	));
}

#[rstest]
#[tokio::test]
async fn dormant_is_recall_only_and_pin_correction_and_manual_reactivation_preserve_content(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	use axum::{Json, Router, routing::post};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let model = tokio::spawn(async move {
		axum::serve(listener, Router::new().route("/v1/embeddings", post(|Json(input): Json<serde_json::Value>| async move {
			Json(json!({"model":input["model"],"data":[{"index":0,"embedding":[1.,0.1,0.]}],"usage":{"prompt_tokens":1}}))
		}))).await.unwrap();
	});
	let (store, _, workspace) = setup_endpoint_decay(
		&database,
		bounds,
		&endpoint,
		(false, false, false),
		None,
		Some(policy()),
	)
	.await;
	let bank = participant(&store, workspace).await;
	let source = Uuid::now_v7();
	let admitted = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: source,
				content: content("Dormant conflict must remain evidence"),
			},
		),
	)
	.await
	.unwrap();
	// Enabling decay does not hide newly activated historical Banks en masse.
	sweep(&store).await;
	assert!(dormant(&store, &bank).await.is_empty());
	let as_of = Utc::now() + Duration::days(365);
	clock(&store, &bank, as_of, None).await;
	sweep(&store).await;
	assert_eq!(dormant(&store, &bank).await, admitted);
	let read = memory::ReadBank {
		provider: reference("p"),
		bank: bank.clone(),
	};
	assert_eq!(
		memory::list(&store, &Actor::Operator, read.clone())
			.await
			.unwrap(),
		admitted
	);
	let memory::Outcome::Recall(result) = operate(
		&store,
		&bank,
		memory::Action::Recall {
			query: RecallQuery {
				text: "conflict".into(),
				time: None,
				kinds: vec![],
				max_tokens: 8192,
			},
		},
	)
	.await
	else {
		panic!("recall");
	};
	assert_eq!(result, Recall::Empty);
	let memory::Outcome::Recall(Recall::Ready { units }) = operate(
		&store,
		&bank,
		memory::Action::RecallIncludingDormant {
			query: RecallQuery {
				text: "conflict".into(),
				time: None,
				kinds: vec![],
				max_tokens: 8192,
			},
		},
	)
	.await
	else {
		panic!("dormant-inclusive recall");
	};
	assert_eq!(units, admitted);
	assert!(dormant(&store, &bank).await.is_empty());
	clock(&store, &bank, as_of, None).await;
	sweep(&store).await;
	let memory::Outcome::Units(pinned) =
		operate(&store, &bank, memory::Action::Pin { id: source }).await
	else {
		panic!("units");
	};
	assert_eq!(pinned, admitted);
	assert!(dormant(&store, &bank).await.is_empty());
	operate(&store, &bank, memory::Action::Unpin { id: source }).await;
	clock(&store, &bank, as_of, None).await;
	sweep(&store).await;
	assert_eq!(dormant(&store, &bank).await, admitted);
	let memory::Outcome::Units(reactivated) =
		operate(&store, &bank, memory::Action::Reactivate { id: source }).await
	else {
		panic!("units");
	};
	assert_eq!(reactivated, admitted);
	assert!(dormant(&store, &bank).await.is_empty());
	clock(&store, &bank, as_of, None).await;
	sweep(&store).await;
	let corrected = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Correct {
				id: source,
				expected_revision: 1,
				content: content("Correction remains possible while Dormant"),
			},
		),
	)
	.await
	.unwrap();
	assert_eq!(corrected[0].revision, 2);
	assert!(dormant(&store, &bank).await.is_empty());
	clock(&store, &bank, as_of, None).await;
	sweep(&store).await;
	let deleted = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Delete {
				id: source,
				expected_revision: 2,
			},
		),
	)
	.await
	.unwrap();
	assert!(deleted[0].deleted);
	assert!(
		memory::list(&store, &Actor::Operator, read)
			.await
			.unwrap()
			.is_empty()
	);
	model.abort();
}

#[rstest]
#[tokio::test]
async fn dormancy_pages_resume_fixed_clock_and_exempt_pinned_preference_and_procedure(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup_endpoint_decay(
		&database,
		bounds,
		"http://127.0.0.1:9/v1",
		(false, false, false),
		None,
		Some(policy()),
	)
	.await;
	let bank = participant(&store, workspace).await;
	let mut ids = vec![];
	for learning in [
		Learning::Fact,
		Learning::Preference,
		Learning::Procedure,
		Learning::Fact,
	] {
		let id = Uuid::now_v7();
		ids.push(id);
		let mut body = content("Keep independent recall state");
		body.learning = learning;
		memory::mutate(
			&store,
			&Actor::Operator,
			mutation(&bank, Change::Add { id, content: body }),
		)
		.await
		.unwrap();
	}
	operate(&store, &bank, memory::Action::Pin { id: ids[3] }).await;
	let as_of = Utc::now() + Duration::days(365);
	clock(&store, &bank, as_of, None).await;
	sweep(&store).await;
	let first = native::query(
		&Query::select()
			.columns(["as_of", "cursor"].map(Alias::new))
			.from(Alias::new("memory_bank_decay"))
			.and_where(Expr::col("bank_id").eq(Expr::value(id(&store, &bank).await)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(
		first.try_get::<Option<Uuid>>("cursor").unwrap(),
		Some(ids[0])
	);
	assert_eq!(
		first
			.try_get::<Option<chrono::DateTime<Utc>>>("as_of")
			.unwrap()
			.unwrap()
			.timestamp(),
		as_of.timestamp()
	);
	assert_eq!(
		dormant(&store, &bank)
			.await
			.iter()
			.map(|u| u.id)
			.collect::<Vec<_>>(),
		vec![ids[0]]
	);
	let restarted = database.kill_and_restart(&store).await;
	for _ in 0..4 {
		sweep(&restarted).await;
	}
	assert_eq!(
		dormant(&restarted, &bank)
			.await
			.iter()
			.map(|u| u.id)
			.collect::<Vec<_>>(),
		vec![ids[0]]
	);
	clock(&restarted, &bank, as_of, None).await;
	for _ in 0..5 {
		sweep(&restarted).await;
	}
	assert_eq!(
		dormant(&restarted, &bank)
			.await
			.iter()
			.map(|u| u.id)
			.collect::<Vec<_>>(),
		vec![ids[0]]
	);
	// A flag from another immutable policy version is ignored, never deleted.
	let mut tx = native::begin(&restarted.pool).await.unwrap();
	let other = EntityRef {
		id: "p".into(),
		version: "2.0.0".into(),
	};
	native::query(
		&Query::update()
			.table(Alias::new("memory_unit_retention"))
			.value(
				Alias::new("dormant_policy"),
				serde_json::to_value(&other).unwrap(),
			)
			.and_where(Expr::col("unit_id").eq(Expr::value(ids[0])))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	assert!(dormant(&restarted, &bank).await.is_empty());
	let flag: serde_json::Value = native::query_scalar(
		&Query::select()
			.column(Alias::new("dormant_policy"))
			.from(Alias::new("memory_unit_retention"))
			.and_where(Expr::col("unit_id").eq(Expr::value(ids[0])))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&restarted.pool)
	.await
	.unwrap();
	assert_eq!(flag, serde_json::to_value(other).unwrap());
}
