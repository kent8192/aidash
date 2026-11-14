//! Issue #127: a failed Run read journal reports its exact cause, so a source
//! change is never reported as lost authority and lost authority never as a
//! source change.
use super::super::fixture::*;
use super::*;
use crate::apps::execution::test_database::{DatabaseFixture, database};
use crate::apps::knowledge::services::native_memory as memory;
use crate::authorization::{Authorization, identity::Actor};
use crate::store::Store;
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_domain::memory::*;
use rstest::rstest;
use serde_json::json;

/// A Run of Agent `a` bound to memory `provider` that read revision 1 of
/// `unit`, and a Subject allowed everything by policy revision 1.
async fn reader(store: &Store, bank: &Bank, unit: Uuid, provider: &str) -> (Uuid, Actor) {
	let actor = subject(store).await;
	(bound_run(store, bank, provider, Some(unit)).await, actor)
}

/// A Subject allowed everything by policy revision 1.
async fn subject(store: &Store) -> Actor {
	crate::registry::Registry::new(store.pool.clone(), &store.deployment)
		.unwrap()
		.seed_system()
		.await
		.unwrap();
	let authorization = Authorization {
		pool: store.pool.clone(),
	};
	let owner = aidash_domain::qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme"),
		"a",
		"1.0.0",
	);
	authorization
		.replace(
			"acme",
			1,
			serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},owner:{"kind":"agent", "node": aidash_domain::nodes::NodeName::derived("acme")}},"policies":[{"id":"all","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]})).unwrap(),
			"deployment_administrator",
		)
		.await
		.unwrap();
	for id in ["m", "e", "r", "t", "p", "a"] {
		authorization
			.set_catalog(
				"acme",
				&aidash_domain::nodes::NodeName::derived("acme"),
				&reference(id),
				0,
				true,
				"deployment_administrator",
			)
			.await
			// The fixture Subject may have approved it already.
			.ok();
	}
	let credential = authorization
		.issue_credential("acme", "alice", 3600, "deployment_administrator")
		.await
		.unwrap();
	authorization.authenticate(&credential.token).await.unwrap()
}

/// A completed Run of Agent `a` bound to memory `provider` that read revision 1
/// of `read`, when given.
async fn bound_run(store: &Store, bank: &Bank, provider: &str, read: Option<Uuid>) -> Uuid {
	let task = store
		.create_task(
			&TEST_FIXTURE,
			bank.workspace,
			&aidash_domain::NewTask {
				title: "Reader".into(),
				description: "Read one Unit".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"deployment_administrator",
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
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	let mut rows = vec![
		(
			"runs",
			vec![
				"id",
				"task_id",
				"workspace_id",
				"home_node",
				"node",
				"agent_id",
				"agent_version",
				"phase",
				"pending",
				"revision",
				"tenant",
			],
			vec![
				Expr::value(run),
				Expr::value(task.id),
				Expr::value(bank.workspace),
				Expr::value(
					aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
						.to_string(),
				),
				Expr::value(aidash_domain::nodes::NodeName::derived("acme").to_string()),
				Expr::value("a"),
				Expr::value("1.0.0"),
				Expr::value("COMPLETED"),
				Expr::value(pending),
				Expr::value(1_i64),
				Expr::value("acme"),
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
				"tenant",
			],
			vec![
				Expr::value(run),
				Expr::value(bank.participant.unwrap()),
				Expr::value(1_i64),
				Expr::value(provider),
				Expr::value("1.0.0"),
				Expr::value("acme"),
			],
		),
	];
	if let Some(unit) = read {
		rows.push((
			"memory_run_reads",
			vec!["run_id", "unit_id", "revision", "tenant"],
			vec![
				Expr::value(run),
				Expr::value(unit),
				Expr::value(1_i64),
				Expr::value("acme"),
			],
		));
	}
	for (table, columns, values) in rows {
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
	run
}

async fn cause(store: &Store, actor: &Actor, workspace: Uuid, run: Uuid) -> Option<Failure> {
	let mut lease = Lease::begin(store, actor, workspace).await.unwrap();
	let result = invalidity(&mut lease, run).await;
	lease.finish(result).await.unwrap()
}

/// Records a read of no new Unit, which revalidates the Run's earlier reads.
async fn record_again(store: &Store, actor: &Actor, workspace: Uuid, run: Uuid) -> Result<()> {
	let mut lease = Lease::begin(store, actor, workspace).await.unwrap();
	let result = record(&mut lease, run, &[]).await;
	lease.finish(result).await
}

#[rstest]
#[case::unchanged("unchanged", None)]
#[case::correction("correction", Some(Failure::Invalidated))]
#[case::withdrawal("withdrawal", Some(Failure::Invalidated))]
#[case::deletion("deletion", Some(Failure::Invalidated))]
#[case::participant_change("participant_change", Some(Failure::Configuration))]
#[case::lost_read_authority("lost_read_authority", Some(Failure::Authority))]
#[case::empty_journal_without_authority("empty_journal_without_authority", None)]
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
		&administrator(&store).await,
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
		&administrator(&store).await,
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
	let (run, actor) = reader(&store, &bank, id, "p").await;
	assert_eq!(cause(&store, &actor, bank.workspace, run).await, None);

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
			memory::mutate(&store, &administrator(&store).await, change)
				.await
				.unwrap();
		}
		"withdrawal" => {
			let change = corrected(|next| next.verification = Verification::Contradicted);
			memory::mutate(&store, &administrator(&store).await, change)
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
			memory::mutate(&store, &administrator(&store).await, change)
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
			.execute(store.pool.in_scope(&TEST_FIXTURE))
			.await
			.unwrap();
		}
		"lost_read_authority" | "empty_journal_without_authority" => {
			if change == "empty_journal_without_authority" {
				native::query(
					&Query::delete()
						.from_table(Alias::new("memory_run_reads"))
						.and_where(Expr::col("run_id").eq(Expr::value(run)))
						.to_string(PostgresQueryBuilder),
				)
				.execute(store.pool.in_scope(&TEST_FIXTURE))
				.await
				.unwrap();
			}
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
				"deployment_administrator",
			)
			.await
			.unwrap();
		}
		_ => unreachable!(),
	}

	// Assert
	assert_eq!(
		cause(&store, &actor, bank.workspace, run).await,
		expected,
		"{change} must report its own cause"
	);
}

/// #852 decision 8: Built-in Memory evicts, corrects, deletes and reconciles its
/// Units by itself, so a Unit a Run already read changing is not a reason to stop
/// that Run. Losing the authority to read memory still is.
#[rstest]
#[case::unchanged("unchanged", None)]
#[case::correction("correction", None)]
#[case::reconciliation("reconciliation", None)]
#[case::eviction("eviction", None)]
#[case::lost_read_authority("lost_read_authority", Some(Failure::Authority))]
#[tokio::test]
async fn builtin_memory_read_journal_fails_only_on_lost_authority(
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
		&administrator(&store).await,
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
		&administrator(&store).await,
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
	let (run, actor) = reader(
		&store,
		&bank,
		id,
		aidash_domain::registry::bindings::memory_defaults::MEMORY_ID,
	)
	.await;
	assert_eq!(cause(&store, &actor, bank.workspace, run).await, None);

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
	let change_unit = match change {
		"unchanged" | "lost_read_authority" => None,
		"correction" => Some(corrected(|next| {
			next.text = "Rotate the signing key monthly.".into()
		})),
		"reconciliation" => Some(corrected(|next| {
			next.verification = Verification::Contradicted
		})),
		"eviction" => Some(mutation(
			&bank,
			Change::Delete {
				id,
				expected_revision: 1,
			},
		)),
		_ => unreachable!(),
	};
	if let Some(change) = change_unit {
		memory::mutate(&store, &administrator(&store).await, change)
			.await
			.unwrap();
	}
	if change == "lost_read_authority" {
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
			"deployment_administrator",
		)
		.await
		.unwrap();
	}

	// Assert
	assert_eq!(
		cause(&store, &actor, bank.workspace, run).await,
		expected,
		"{change} of a built-in Unit"
	);
}

/// #852 decision 8 forgives only Built-in Memory's own changes to its Units. A
/// Run that recalled a built-in fact whose Research was later revoked or expired
/// (ADR 0046) still stops, also when the Unit was corrected afterwards: its
/// earlier memory tool results quote that Research and nothing redacts them.
/// Recording a later read revalidates the earlier one and refuses as well.
#[rstest]
#[case::revocation("revocation", false)]
#[case::account_terms("account_terms", false)]
#[case::revocation_then_correction("revocation", true)]
#[tokio::test]
async fn builtin_memory_read_journal_fails_when_research_behind_a_unit_is_withdrawn(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] loss: &str,
	#[case] corrected: bool,
) {
	// Arrange
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = memory::create_participant(
		&store,
		&administrator(&store).await,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let builtin = aidash_domain::registry::bindings::memory_defaults::MEMORY_ID;
	let actor = subject(&store).await;
	// The Run whose Research the fact was learned from.
	let source = bound_run(&store, &bank, builtin, None).await;
	let result = json!({});
	let digest = aidash_domain::registry::rules::digest(&result);
	let research = json!({"contract":"aidash.research.reference/1", "invocation":"memory-research", "digest":digest, "operation":"search", "sources":[Uuid::now_v7()], "documents":[]});
	let event = json!({"kind":"tool", "call":{"id":"call", "name":"research", "arguments":{}}, "result":research});
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	for statement in [
		Query::insert()
			.into_table(Alias::new("run_context_events"))
			.columns(["run_id", "seq", "origin", "event", "digest", "tenant"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(source))
					.expr(Expr::value(1_i64))
					.expr(Expr::value("appended"))
					.expr(Expr::value(event.clone()))
					.expr(Expr::value(aidash_domain::registry::rules::digest(&event)))
					.expr(Expr::value("acme"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
		Query::insert()
			.into_table(Alias::new("research_journal_results"))
			.columns(
				[
					"id",
					"tenant",
					"run_id",
					"invocation_key",
					"digest",
					"reference",
					"bytes",
					"created_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(Uuid::now_v7()))
					.expr(Expr::value("acme"))
					.expr(Expr::value(source))
					.expr(Expr::value("memory-research"))
					.expr(Expr::value(digest))
					.expr(Expr::value(research))
					.expr(Expr::value(b"{}".to_vec()))
					.expr(Expr::value(chrono::Utc::now()))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	] {
		native::query(&statement).execute(&mut *tx).await.unwrap();
	}
	tx.commit().await.unwrap();
	let id = Uuid::now_v7();
	let mut fact = content("The comet returns every 76 years.");
	fact.evidence = vec![Evidence::JournalRange {
		run_id: source,
		from_seq: 1,
		through_seq: 1,
	}];
	memory::mutate(
		&store,
		&administrator(&store).await,
		mutation(
			&bank,
			Change::Add {
				id,
				content: fact.clone(),
			},
		),
	)
	.await
	.unwrap();
	let run = bound_run(&store, &bank, builtin, Some(id)).await;
	assert_eq!(cause(&store, &actor, bank.workspace, run).await, None);
	record_again(&store, &actor, bank.workspace, run)
		.await
		.unwrap();

	if corrected {
		// Built-in Memory corrects the Unit after the Run read it; that
		// forgiven change must not hide the Research withdrawn next.
		fact.text = "The comet returns every 75 to 79 years.".into();
		memory::mutate(
			&store,
			&administrator(&store).await,
			mutation(
				&bank,
				Change::Correct {
					id,
					expected_revision: 1,
					content: fact,
				},
			),
		)
		.await
		.unwrap();
	}

	// Act
	let withdrawal = if loss == "account_terms" {
		Query::update()
			.table(Alias::new("research_journal_results"))
			.value(Alias::new("bytes"), None::<Vec<u8>>)
			.value(Alias::new("purge_reason"), "account_terms")
			.and_where(Expr::col("run_id").eq(Expr::value(source)))
			.to_string(PostgresQueryBuilder)
	} else {
		Query::insert()
			.into_table(Alias::new("research_revocations"))
			.columns(
				[
					"run_id",
					"tenant",
					"revoked_by",
					"reason",
					"revoked_at",
					"removed",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(source))
					.expr(Expr::value("acme"))
					.expr(Expr::value(
						json!({"kind":"subject","tenant":"acme","subject":"alice"}),
					))
					.expr(Expr::value("test"))
					.expr(Expr::value(chrono::Utc::now()))
					.expr(Expr::value(
						serde_json::to_value(
							aidash_domain::research::revocation::Removed::default(),
						)
						.unwrap(),
					))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder)
	};
	native::query(&withdrawal)
		.execute(store.pool.in_scope(&TEST_FIXTURE))
		.await
		.unwrap();
	// Assert
	assert_eq!(
		cause(&store, &actor, bank.workspace, run).await,
		Some(Failure::Invalidated),
		"{loss} of the Research behind a recalled built-in Unit must stop the Run"
	);
	assert!(
		matches!(
			record_again(&store, &actor, bank.workspace, run).await,
			Err(Error::Conflict(_))
		),
		"recording a later read must not pass over the withdrawn Research"
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
	let result = record(
		&mut Lease::BorrowedWithoutSubject(&mut tx),
		Uuid::new_v4(),
		&[],
	)
	.await;

	// Assert
	assert!(result.is_err(), "the probe refuses the run read");
	assert_eq!(
		probe.statements(),
		["SAVEPOINT", "ROLLBACK TO SAVEPOINT", "RELEASE SAVEPOINT"]
	);
	assert_eq!(tx.lock_epoch(), 1);
}

#[rstest]
#[tokio::test]
async fn owning_and_recorded_run_task_producer_deletion_is_invalidation(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = memory::create_participant(
		&store,
		&administrator(&store).await,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let unit = Uuid::now_v7();
	memory::mutate(
		&store,
		&administrator(&store).await,
		mutation(
			&bank,
			Change::Add {
				id: unit,
				content: content("Original memory"),
			},
		),
	)
	.await
	.unwrap();
	let (producer, actor) = reader(&store, &bank, unit, "p").await;
	let task = store
		.create_task(
			&TEST_FIXTURE,
			workspace,
			&aidash_domain::NewTask {
				title: "Produced task".into(),
				description: "Continue the producer work".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"deployment_administrator",
			None,
		)
		.await
		.unwrap();
	let consumer = Uuid::now_v7();
	let root = Uuid::now_v7();
	let root_task = store
		.create_task(
			&TEST_FIXTURE,
			workspace,
			&aidash_domain::NewTask {
				title: "Root task".into(),
				description: "Read the dependency".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"deployment_administrator",
			None,
		)
		.await
		.unwrap()
		.id;
	let pending = aidash_domain::run_state::encode(
		&aidash_domain::run_state::RunState::Completed(aidash_domain::run_state::TerminalState {}),
		&aidash_domain::run_state::RecoveryState::default(),
	)
	.unwrap();
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	for (table, columns, values) in [
		(
			"runs",
			vec![
				"id",
				"task_id",
				"workspace_id",
				"home_node",
				"node",
				"agent_id",
				"agent_version",
				"phase",
				"pending",
				"revision",
				"tenant",
			],
			vec![
				Expr::value(consumer),
				Expr::value(task.id),
				Expr::value(workspace),
				Expr::value(
					aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
						.to_string(),
				),
				Expr::value(aidash_domain::nodes::NodeName::derived("acme").to_string()),
				Expr::value("dependency"),
				Expr::value("1.0.0"),
				Expr::value("COMPLETED"),
				Expr::value(pending.clone()),
				Expr::value(1_i64),
				Expr::value("acme"),
			],
		),
		(
			"runs",
			vec![
				"id",
				"task_id",
				"workspace_id",
				"home_node",
				"node",
				"agent_id",
				"agent_version",
				"phase",
				"pending",
				"revision",
				"tenant",
			],
			vec![
				Expr::value(root),
				Expr::value(root_task),
				Expr::value(workspace),
				Expr::value(
					aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
						.to_string(),
				),
				Expr::value(aidash_domain::nodes::NodeName::derived("acme").to_string()),
				Expr::value("a"),
				Expr::value("1.0.0"),
				Expr::value("COMPLETED"),
				Expr::value(pending),
				Expr::value(1_i64),
				Expr::value("acme"),
			],
		),
		(
			"authorization_run_reads",
			vec![
				"run_id",
				"tenant",
				"workspace_id",
				"resource_kind",
				"resource_id",
			],
			vec![
				Expr::value(root),
				Expr::value("acme"),
				Expr::value(workspace),
				Expr::value("run"),
				Expr::value(consumer),
			],
		),
		(
			"authorization_run_outputs",
			vec![
				"run_id",
				"tenant",
				"workspace_id",
				"resource_kind",
				"resource_id",
			],
			vec![
				Expr::value(producer),
				Expr::value("acme"),
				Expr::value(workspace),
				Expr::value("task"),
				Expr::value(task.id),
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
	let consumer_run = store.run(&TEST_FIXTURE, consumer).await.unwrap().metadata();
	let Actor::Subject(identity) = actor else {
		panic!("expected subject")
	};
	let mut access = crate::authorization::access::Access::begin(&store, &identity)
		.await
		.unwrap();
	let result = access.run_invalidity(&consumer_run).await;
	assert_eq!(access.finish(result).await.unwrap(), None);
	// A memory-free dependency does not require access to its private bank.
	Authorization { pool: store.pool.clone() }
		.replace("acme", 2, serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[
			{"id":"all","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},
			{"id":"deny-dependency-memory","effect":"deny","subjects":{"any":true},"actions":["memory.read"],"resources":{"kinds":["memory"],"ids":["dependency"]}}
		]})).unwrap(), "deployment_administrator").await.unwrap();
	let mut access = crate::authorization::access::Access::begin(&store, &identity)
		.await
		.unwrap();
	let result = access.run_invalidity(&consumer_run).await;
	assert_eq!(
		access.finish(result).await.unwrap(),
		Some(Failure::Authority)
	);
	let run = store.run(&TEST_FIXTURE, root).await.unwrap().metadata();
	let mut access = crate::authorization::access::Access::begin(&store, &identity)
		.await
		.unwrap();
	let result = access.run_invalidity(&run).await;
	assert_eq!(access.finish(result).await.unwrap(), None);
	memory::mutate(
		&store,
		&administrator(&store).await,
		mutation(
			&bank,
			Change::Delete {
				id: unit,
				expected_revision: 1,
			},
		),
	)
	.await
	.unwrap();
	let mut access = crate::authorization::access::Access::begin(&store, &identity)
		.await
		.unwrap();
	let result = access.run_invalidity(&run).await;
	assert_eq!(
		access.finish(result).await.unwrap(),
		Some(Failure::Invalidated)
	);
	// Restore the original owning-Task regression under unrestricted memory.
	Authorization { pool: store.pool.clone() }.replace("acme", 3,
		serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[
			{"id":"all","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}
		]})).unwrap(), "deployment_administrator").await.unwrap();
	let mut access = crate::authorization::access::Access::begin(&store, &identity)
		.await
		.unwrap();
	let result = access.run_invalidity(&consumer_run).await;
	assert_eq!(
		access.finish(result).await.unwrap(),
		Some(Failure::Invalidated)
	);
	Authorization { pool: store.pool.clone() }
		.replace(
			"acme",
			4,
			serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[
				{"id":"all","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},
				{"id":"deny-task","effect":"deny","subjects":{"any":true},"actions":["task.read"],"resources":{"kinds":["task"],"ids":[task.id]}}
			]})).unwrap(),
			"deployment_administrator",
		)
		.await
		.unwrap();
	let mut access = crate::authorization::access::Access::begin(&store, &identity)
		.await
		.unwrap();
	let result = access.run_invalidity(&run).await;
	assert_eq!(
		access.finish(result).await.unwrap(),
		Some(Failure::Authority)
	);

	Authorization { pool: store.pool.clone() }.replace("acme", 5,
		serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[
			{"id":"all","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},
			{"id":"deny-dependency-run","effect":"deny","subjects":{"any":true},"actions":["run.read"],"resources":{"kinds":["run"],"ids":[consumer]}}
		]})).unwrap(), "deployment_administrator").await.unwrap();
	let mut access = crate::authorization::access::Access::begin(&store, &identity)
		.await
		.unwrap();
	let result = access.run_invalidity(&run).await;
	assert_eq!(
		access.finish(result).await.unwrap(),
		Some(Failure::Authority)
	);
}
