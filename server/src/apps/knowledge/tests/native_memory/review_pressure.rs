//! Temporary candidate pressure preserves durable learning and its model receipt.
use super::*;
use aidash_server::database::native;

pub(super) async fn retry_learning(store: &Store, bank: &Bank, proof: &Evidence) -> Vec<Candidate> {
	let Evidence::Run { id: run, .. } = proof else {
		panic!("Run proof")
	};
	let queued = Uuid::now_v7();
	let bank_id: Uuid = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_banks"))
			.and_where(Expr::col("participant_id").eq(Expr::value(bank.participant.unwrap())))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_candidates"))
			.columns(
				[
					"id",
					"bank_id",
					"revision",
					"run",
					"text",
					"kind",
					"learning",
					"verification",
					"entities",
					"evidence",
					"links",
					"state",
					"created_at",
					"updated_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(queued))
					.expr(Expr::value(bank_id))
					.expr(Expr::value(1_i64))
					.expr(Expr::value(serde_json::to_value(proof).unwrap()))
					.expr(Expr::value("Existing review queue entry"))
					.expr(Expr::value("world"))
					.expr(Expr::value("fact"))
					.expr(Expr::value("unverified"))
					.expr(Expr::value(json!([])))
					.expr(Expr::value(json!([proof])))
					.expr(Expr::value(json!([])))
					.expr(Expr::value("pending"))
					.expr(Expr::value(chrono::Utc::now()))
					.expr(Expr::value(chrono::Utc::now()))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	// Two extracted candidates cannot fit beside the existing entry. Repeat past
	// the configured two model attempts: human backpressure must not exhaust them.
	for _ in 0..3 {
		due(store, *run).await;
		aidash_server::semantic::worker::sweep(store).await.unwrap();
		let row = native::query(
			&Query::select()
				.columns(["state", "attempts", "last_error"].map(Alias::new))
				.from(Alias::new("memory_engine_jobs"))
				.and_where(Expr::col("id").eq(Expr::value(*run)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&store.pool)
		.await
		.unwrap();
		assert_eq!(row.try_get::<String>("state").unwrap(), "pending");
		assert_eq!(row.try_get::<i32>("attempts").unwrap(), 0);
		assert_eq!(
			row.try_get::<String>("last_error").unwrap(),
			"candidate_queue_full"
		);
	}
	memory::operate(
		store,
		&Actor::Operator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::Review {
				id: queued,
				expected_revision: 1,
				mutation: None,
			},
		},
	)
	.await
	.unwrap();
	due(store, *run).await;
	aidash_server::semantic::worker::sweep(store).await.unwrap();
	let state: String = native::query_scalar(
		&Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("memory_engine_jobs"))
			.and_where(Expr::col("id").eq(Expr::value(*run)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(
		state, "complete",
		"reviewing existing candidates resumes durable learning"
	);
	let memory::Outcome::Candidates(candidates) = memory::operate(
		store,
		&Actor::Operator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::Candidates,
		},
	)
	.await
	.unwrap() else {
		panic!("candidates")
	};
	assert_eq!(candidates.len(), 2);
	candidates
}

async fn due(store: &Store, run: Uuid) {
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_engine_jobs"))
			.value(
				Alias::new("next_attempt"),
				chrono::Utc::now() - chrono::Duration::seconds(1),
			)
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
}
