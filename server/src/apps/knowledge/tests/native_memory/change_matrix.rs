//! Issue #127 change matrix: current-behavior fixtures for dependent Runs.
//! Each case maps to a row of `docs/memory-recovery.md#change-matrix`.
use super::policy_bounds::{recorded_run, run_visible};
use super::*;

/// The shape a caller chooses for a change is not an exemption. A correction
/// that keeps the text byte-identical and only edits metadata, or repeats the
/// exact content, still creates a new revision and fences every Run that read
/// the old one. Withdrawal (a contradicted claim) and deletion fence it too.
#[rstest]
#[case::text("text")]
#[case::metadata_only("metadata_only")]
#[case::identical("identical")]
#[case::withdrawal("withdrawal")]
#[case::deletion("deletion")]
#[tokio::test]
async fn every_unit_change_fences_a_dependent_run_regardless_of_its_shape(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] shape: &str,
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
	let original = content("Deploy from the release branch only.");
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
	let (scope, run) = recorded_run(&store, &bank, &[id]).await;
	assert!(
		run_visible(&scope, &store, run).await,
		"the Run's read of revision 1 is valid before any change"
	);

	// Act
	let change = match shape {
		"deletion" => Change::Delete {
			id,
			expected_revision: 1,
		},
		_ => {
			let mut next = original.clone();
			match shape {
				"text" => next.text = "Deploy from any reviewed branch.".into(),
				"metadata_only" => {
					next.learning = Learning::Procedure;
					next.entities = vec![Entity {
						name: "release branch".into(),
						category: "branch".into(),
						aliases: vec![],
					}];
				}
				"withdrawal" => next.verification = Verification::Contradicted,
				_ => {}
			}
			Change::Correct {
				id,
				expected_revision: 1,
				content: next,
			}
		}
	};
	memory::mutate(&store, &Actor::Operator, mutation(&bank, change))
		.await
		.unwrap();

	// Assert
	assert!(
		!run_visible(&scope, &store, run).await,
		"a {shape} change must fence the Run that consumed revision 1"
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
	match shape {
		"deletion" | "withdrawal" => assert!(
			current.is_empty(),
			"a {shape} excludes the Unit from listing immediately"
		),
		_ => assert_eq!(
			current.iter().map(|unit| unit.revision).collect::<Vec<_>>(),
			vec![2],
			"a {shape} correction creates a new revision"
		),
	}
}

/// A stale observed revision is rejected by compare-and-set: the caller cannot
/// override another writer's change by restating the revision it last saw.
#[rstest]
#[tokio::test]
async fn concurrent_writers_on_one_unit_have_a_single_winner_and_fence_readers(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
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
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("Shared runbook step one."),
			},
		),
	)
	.await
	.unwrap();
	let (scope, first_reader) = recorded_run(&store, &bank, &[id]).await;
	let second_reader = another_reader(&store, workspace, first_reader).await;
	assert!(run_visible(&scope, &store, second_reader).await);
	let correct = |text: &str| {
		mutation(
			&bank,
			Change::Correct {
				id,
				expected_revision: 1,
				content: content(text),
			},
		)
	};

	// Act
	let (left, right) = tokio::join!(
		memory::mutate(
			&store,
			&Actor::Operator,
			correct("Shared runbook step one, writer A.")
		),
		memory::mutate(
			&store,
			&Actor::Operator,
			correct("Shared runbook step one, writer B.")
		),
	);

	// Assert
	assert_eq!(
		[left.is_ok(), right.is_ok()]
			.iter()
			.filter(|ok| **ok)
			.count(),
		1,
		"exactly one writer wins the revision-1 compare-and-set: {left:?} {right:?}"
	);
	let loser = if left.is_ok() { right } else { left };
	assert!(
		matches!(loser, Err(aidash_server::Error::Conflict(_))),
		"the losing writer observes a conflict, not a silent overwrite: {loser:?}"
	);
	for reader in [first_reader, second_reader] {
		assert!(
			!run_visible(&scope, &store, reader).await,
			"every reader of revision 1 is fenced by the winning correction"
		);
	}
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
	assert_eq!(current.len(), 1);
	assert_eq!(current[0].revision, 2, "no third revision was created");
}

/// A second Run, for its own Task, with the same Binding and read journal as `run`.
async fn another_reader(store: &Store, workspace: Uuid, run: Uuid) -> Uuid {
	use aidash_server::database::native;
	let task = store
		.create_task(
			workspace,
			&aidash_domain::NewTask {
				title: "Second reader".into(),
				description: "Read the same shared Unit".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"operator",
			None,
		)
		.await
		.unwrap();
	let reader = Uuid::now_v7();
	let mut tx = native::begin(&store.pool).await.unwrap();
	for (table, key, fixed, columns) in [
		(
			"runs",
			"id",
			Some(("task_id", task.id)),
			&[
				"workspace_id",
				"home_node",
				"agent_id",
				"agent_version",
				"phase",
				"pending",
				"revision",
			][..],
		),
		(
			"memory_run_bindings",
			"run_id",
			None,
			&[
				"participant_id",
				"participant_revision",
				"provider_id",
				"provider_version",
			][..],
		),
		(
			"memory_run_reads",
			"run_id",
			None,
			&["unit_id", "revision"][..],
		),
	] {
		let mut copied = Query::select();
		copied.expr(Expr::value(reader));
		let mut targets = vec![key];
		if let Some((column, value)) = fixed {
			copied.expr(Expr::value(value));
			targets.push(column);
		}
		targets.extend(columns.iter().copied());
		for column in columns {
			copied.column(Alias::new(*column));
		}
		copied
			.from(Alias::new(table))
			.and_where(Expr::col(key).eq(Expr::value(run)));
		native::query(
			&Query::insert()
				.into_table(Alias::new(table))
				.columns(targets.into_iter().map(Alias::new))
				.from_subquery(copied)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
	}
	tx.commit().await.unwrap();
	reader
}
