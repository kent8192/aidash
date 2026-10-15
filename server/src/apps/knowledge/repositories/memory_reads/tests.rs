//! Issue #127: a failed Run read journal reports its exact cause, so a source
//! change is never reported as lost authority and lost authority never as a
//! source change.
use super::super::fixture::*;
use super::*;
use crate::apps::execution::test_database::{DatabaseFixture, database};
use crate::apps::knowledge::services::native_memory as memory;
use crate::authorization::{Authorization, identity::Actor};
use crate::store::Store;
use aidash_domain::memory::*;
use rstest::rstest;
use serde_json::json;

/// A Run of Agent `a` that read revision 1 of `unit`, and a Subject allowed
/// everything by policy revision 1.
async fn reader(store: &Store, bank: &Bank, unit: Uuid) -> (Uuid, Actor) {
	crate::registry::Registry::new(store.pool.clone(), &store.node_id)
		.unwrap()
		.seed_system()
		.await
		.unwrap();
	let authorization = Authorization {
		pool: store.pool.clone(),
	};
	let owner = aidash_domain::qualified_agent(&store.node_id, "a", "1.0.0");
	authorization
		.replace(
			"acme",
			1,
			serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},owner:{"kind":"agent"}},"policies":[{"id":"all","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]})).unwrap(),
			"operator",
		)
		.await
		.unwrap();
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
	let actor = authorization.authenticate(&credential.token).await.unwrap();
	let task = store
		.create_task(
			bank.workspace,
			&aidash_domain::NewTask {
				title: "Reader".into(),
				description: "Read one Unit".into(),
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
		(
			"memory_run_reads",
			vec!["run_id", "unit_id", "revision"],
			vec![Expr::value(run), Expr::value(unit), Expr::value(1_i64)],
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
	(run, actor)
}

async fn cause(store: &Store, actor: &Actor, run: Uuid) -> Option<Failure> {
	let mut lease = Lease::begin(store, actor).await.unwrap();
	let result = invalidity(&mut lease, run).await;
	lease.finish(result).await.unwrap()
}

#[rstest]
#[case::unchanged("unchanged", None)]
#[case::correction("correction", Some(Failure::Invalidated))]
#[case::withdrawal("withdrawal", Some(Failure::Invalidated))]
#[case::deletion("deletion", Some(Failure::Invalidated))]
#[case::participant_change("participant_change", Some(Failure::Configuration))]
#[case::lost_read_authority("lost_read_authority", Some(Failure::Authority))]
#[tokio::test]
async fn a_failed_read_journal_reports_its_exact_cause(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] change: &str,
	#[case] expected: Option<Failure>,
) {
	// Arrange
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
	let original = content("Rotate the signing key every quarter.");
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id,
				content: original.clone(),
			},
		),
	)
	.await
	.unwrap();
	let (run, actor) = reader(&store, &bank, id).await;
	assert_eq!(cause(&store, &actor, run).await, None);

	// Act
	let corrected = |edit: fn(&mut Content)| {
		let mut next = original.clone();
		edit(&mut next);
		mutation(
			&bank,
			Change::Correct {
				id,
				expected_revision: 1,
				content: next,
			},
		)
	};
	match change {
		"unchanged" => {}
		"correction" => {
			let change = corrected(|next| next.text = "Rotate the signing key monthly.".into());
			memory::mutate(&store, &Actor::Operator, change)
				.await
				.unwrap();
		}
		"withdrawal" => {
			let change = corrected(|next| next.verification = Verification::Contradicted);
			memory::mutate(&store, &Actor::Operator, change)
				.await
				.unwrap();
		}
		"deletion" => {
			let change = mutation(
				&bank,
				Change::Delete {
					id,
					expected_revision: 1,
				},
			);
			memory::mutate(&store, &Actor::Operator, change)
				.await
				.unwrap();
		}
		"participant_change" => {
			// Models a participant upgrade: the Run's pinned participant revision
			// no longer matches the current participant.
			native::query(
				&Query::update()
					.table(Alias::new("memory_participants"))
					.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
					.and_where(Expr::col("id").eq(Expr::value(bank.participant.unwrap())))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&store.pool)
			.await
			.unwrap();
		}
		"lost_read_authority" => {
			Authorization {
				pool: store.pool.clone(),
			}
			.replace(
				"acme",
				2,
				serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[
					{"id":"all","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},
					{"id":"no-memory","effect":"deny","subjects":{"any":true},"actions":["memory.read"],"resources":{"kinds":["*"]}}
				]}))
				.unwrap(),
				"operator",
			)
			.await
			.unwrap();
		}
		_ => unreachable!(),
	}

	// Assert
	assert_eq!(
		cause(&store, &actor, run).await,
		expected,
		"{change} must report its own cause"
	);
}

/// Issue #275: a rejected record rolls back to its savepoint, which may release
/// the locks behind catalog admissions made inside it, so the transaction's lock
/// epoch must move although this module does not import `TransactionExecutor`.
#[tokio::test]
async fn a_rejected_record_moves_the_lock_epoch_of_its_transaction() {
	// Arrange
	let probe = crate::database::native::probe::SavepointProbe::default();
	let mut tx = probe.transaction(
		sqlx::postgres::PgPoolOptions::new()
			.connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
			.unwrap()
			.into(),
	);

	// Act
	let result = record(&mut Lease::BorrowedOperator(&mut tx), Uuid::new_v4(), &[]).await;

	// Assert
	assert!(result.is_err(), "the probe refuses the run read");
	assert_eq!(
		probe.statements(),
		["SAVEPOINT", "ROLLBACK TO SAVEPOINT", "RELEASE SAVEPOINT"]
	);
	assert_eq!(tx.lock_epoch(), 1);
}
