//! Issue #127 change matrix: current-behavior fixtures for dependent Runs.
//! Each case maps to a row of `docs/memory-recovery.md#change-matrix`.
use super::policy_bounds::{recorded_run, run_visible};
use super::*;
use aidash_application::tenancy::TEST_FIXTURE;

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
		&Actor::DeploymentAdministrator,
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
		&Actor::DeploymentAdministrator,
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
	memory::mutate(
		&store,
		&Actor::DeploymentAdministrator,
		mutation(&bank, change),
	)
	.await
	.unwrap();

	// Assert
	assert!(
		!run_visible(&scope, &store, run).await,
		"a {shape} change must fence the Run that consumed revision 1"
	);
	let current = memory::list(
		&store,
		&Actor::DeploymentAdministrator,
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
		&Actor::DeploymentAdministrator,
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
		&Actor::DeploymentAdministrator,
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
			&Actor::DeploymentAdministrator,
			correct("Shared runbook step one, writer A.")
		),
		memory::mutate(
			&store,
			&Actor::DeploymentAdministrator,
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
		&Actor::DeploymentAdministrator,
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
			&TEST_FIXTURE,
			workspace,
			&aidash_domain::NewTask {
				title: "Second reader".into(),
				description: "Read the same shared Unit".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"deployment_administrator",
			None,
		)
		.await
		.unwrap();
	let reader = Uuid::now_v7();
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
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
				"tenant",
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
				"tenant",
			][..],
		),
		(
			"memory_run_reads",
			"run_id",
			None,
			&["unit_id", "revision", "tenant"][..],
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

/// Impact inspection is one content-free view: before a change it lists the
/// visible Runs that would be fenced and the derived Units that depend on the
/// Unit; after the change the same Runs are reported as fenced. A Run the
/// requester cannot see is neither listed nor counted.
#[rstest]
#[tokio::test]
async fn impact_lists_visible_dependents_before_and_after_a_change(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_domain::memory::impact::ImpactQuery;
	use aidash_server::apps::knowledge::services::memory_impact::impact;
	// Arrange: the derivation role answers with an Observation of its input.
	let router = reinhardt::test::stub::StubRouter::new()
		.route("/v1/chat/completions", http::Method::POST, |request| async move {
			let input = request.json::<serde_json::Value>().unwrap();
			let context: serde_json::Value =
				serde_json::from_str(input["messages"][1]["content"].as_str().unwrap()).unwrap();
			let units: Vec<Unit> = serde_json::from_value(context["units"].clone()).unwrap();
			let mut output = content("Rollouts happen early in the week.");
			output.kind = serde_json::from_value(context["kind"].clone()).unwrap();
			output.mental_model = serde_json::from_value(context["mental_model"].clone()).unwrap();
			output.evidence = units.iter().map(Unit::evidence).collect();
			Ok(reinhardt::Response::ok()
				.with_json(&json!({"choices":[{"finish_reason":"stop","message":{"content":serde_json::to_string(&output).unwrap()}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
				.unwrap())
		})
		.into_server_router();
	let server = reinhardt::test::fixtures::test_server_guard(router).await;
	let endpoint = format!("{}/v1", server.url);
	let database = database.await;
	let (store, _, workspace) = setup_endpoint(&database, bounds, &endpoint).await;
	let bank = memory::create_participant(
		&store,
		&Actor::DeploymentAdministrator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let source = memory::mutate(
		&store,
		&Actor::DeploymentAdministrator,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("Secret rollout window: Tuesday."),
			},
		),
	)
	.await
	.unwrap()
	.remove(0);
	let mut draft = content("Rollouts happen early in the week.");
	draft.kind = Kind::Observation;
	draft.evidence = vec![source.evidence()];
	let derivation = mutation(
		&bank,
		Change::Add {
			id: Uuid::now_v7(),
			content: draft,
		},
	);
	let memory::Outcome::Units(derived) = memory::operate(
		&store,
		&Actor::DeploymentAdministrator,
		memory::Operation {
			operation_id: derivation.operation_id,
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::Derive {
				kind: Kind::Observation,
				sources: vec![source.evidence()],
				mutation: derivation,
			},
		},
	)
	.await
	.unwrap() else {
		panic!("derived unit");
	};
	let (scope, run) = recorded_run(&store, &bank, &[source.id]).await;
	let actor = Actor::Subject(scope.identity.clone());
	let query = ImpactQuery {
		provider: reference("p"),
		bank: bank.clone(),
		units: vec![source.id],
	};

	// Act: preview
	let before = impact(&store, &actor, query.clone()).await.unwrap();

	// Assert
	assert!(before.complete);
	assert_eq!(before.units[0].readable_revision, Some(1));
	assert_eq!(
		before.units[0]
			.runs
			.iter()
			.map(|run| (run.id, run.read_revision, run.fenced))
			.collect::<Vec<_>>(),
		vec![(run, 1, false)],
		"the reader would be fenced by a change but is not yet"
	);
	assert_eq!(
		before.units[0]
			.derived
			.iter()
			.map(|unit| unit.id)
			.collect::<Vec<_>>(),
		vec![derived[0].id]
	);
	assert!(
		!serde_json::to_string(&before).unwrap().contains("rollout"),
		"the view carries no memory content"
	);

	// Act: change, then the post-change view
	memory::mutate(
		&store,
		&Actor::DeploymentAdministrator,
		mutation(
			&bank,
			Change::Correct {
				id: source.id,
				expected_revision: 1,
				content: content("Secret rollout window: Thursday."),
			},
		),
	)
	.await
	.unwrap();
	let after = impact(&store, &actor, query.clone()).await.unwrap();

	// Assert
	assert_eq!(after.units[0].readable_revision, Some(2));
	assert_eq!(
		after.units[0]
			.runs
			.iter()
			.map(|run| (run.id, run.read_revision, run.fenced))
			.collect::<Vec<_>>(),
		vec![(run, 1, true)],
		"the post-change view reports the reader as fenced"
	);
	assert!(
		after.units[0].derived.is_empty(),
		"the derived Unit retired with its source revision: {:?}",
		after.units[0].derived
	);

	// Act: the requester loses sight of the Run
	aidash_server::authorization::Authorization {
		pool: store.pool.clone(),
	}
	.replace(
		"acme",
		2,
		serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[
			{"id":"read","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},
			{"id":"hide-runs","effect":"deny","subjects":{"any":true},"actions":["run.read"],"resources":{"kinds":["*"]}}
		]}))
		.unwrap(),
		"deployment_administrator",
	)
	.await
	.unwrap();
	let hidden = impact(&store, &actor, query).await.unwrap();

	// Assert
	assert!(hidden.units[0].runs.is_empty());
	assert!(
		hidden.complete,
		"a hidden Run within the scan bound leaves no trace, not even an incomplete flag"
	);
	drop(server);
}

/// Impact inspection cannot probe identities outside the authorized Bank: a
/// Unit of another Bank answers exactly like an unknown ID, with no readers.
#[rstest]
#[tokio::test]
async fn impact_answers_a_foreign_unit_like_an_unknown_id(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_domain::memory::impact::ImpactQuery;
	use aidash_server::apps::knowledge::services::memory_impact::impact;
	// Arrange
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = memory::create_participant(
		&store,
		&Actor::DeploymentAdministrator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let foreign = memory::mutate(
		&store,
		&Actor::DeploymentAdministrator,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("Belongs to the participant Bank only."),
			},
		),
	)
	.await
	.unwrap()
	.remove(0);
	let (scope, _) = recorded_run(&store, &bank, &[foreign.id]).await;
	let actor = Actor::Subject(scope.identity.clone());
	memory::operate(
		&store,
		&Actor::DeploymentAdministrator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: Bank {
				participant: None,
				..bank.clone()
			},
			action: memory::Action::ConfigureBank {
				expected_revision: 0,
			},
		},
	)
	.await
	.unwrap();
	let shared = Bank {
		participant: None,
		..bank
	};
	let unknown = Uuid::now_v7();

	// Act
	let view = impact(
		&store,
		&actor,
		ImpactQuery {
			provider: reference("p"),
			bank: shared,
			units: vec![foreign.id, unknown],
		},
	)
	.await
	.unwrap();

	// Assert
	for unit in &view.units {
		assert_eq!(unit.readable_revision, None);
		assert!(unit.runs.is_empty() && unit.derived.is_empty());
	}
	assert_eq!(
		serde_json::to_value(&view.units[0]).unwrap()["runs"],
		serde_json::to_value(&view.units[1]).unwrap()["runs"]
	);
}

/// Impact inspection applies the same live-read checks as listing and the Run
/// recheck: a withdrawn derived Unit is not reported, an expired derived Unit
/// disappears before the sweep, an expired root fences its readers, and so does
/// a root whose support expired.
#[rstest]
#[tokio::test]
async fn impact_reports_only_live_units_and_fences_expired_reads(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_domain::memory::impact::ImpactQuery;
	use aidash_server::{apps::knowledge::services::memory_impact::impact, database::native};
	// Arrange
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
		&Actor::DeploymentAdministrator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let add = |text: &str, evidence: Vec<Evidence>| {
		let mut draft = content(text);
		draft.kind = Kind::World;
		draft.evidence = evidence;
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: draft,
			},
		)
	};
	let source = memory::mutate(
		&store,
		&Actor::DeploymentAdministrator,
		add("Root finding", vec![]),
	)
	.await
	.unwrap()
	.remove(0);
	let mut derived = Vec::new();
	for text in [
		"Live dependent",
		"Withdrawn dependent",
		"Supported dependent",
	] {
		derived.push(
			memory::mutate(
				&store,
				&Actor::DeploymentAdministrator,
				add(text, vec![source.evidence()]),
			)
			.await
			.unwrap()
			.remove(0),
		);
	}
	let mut withdrawn = derived[1].content.clone();
	withdrawn.verification = Verification::Contradicted;
	memory::mutate(
		&store,
		&Actor::DeploymentAdministrator,
		mutation(
			&bank,
			Change::Correct {
				id: derived[1].id,
				expected_revision: 1,
				content: withdrawn,
			},
		),
	)
	.await
	.unwrap();
	let supported = derived[2].clone();
	let (scope, run) = recorded_run(&store, &bank, &[source.id, supported.id]).await;
	let actor = Actor::Subject(scope.identity.clone());
	let query = ImpactQuery {
		provider: reference("p"),
		bank: bank.clone(),
		units: vec![source.id],
	};
	// Rebase the fixture clock in canonical storage and its recovery fence.
	let age = |unit: &Unit| {
		let mut unit = unit.clone();
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
		record.fence.digest = aidash_domain::memory::recovery::digest(&unit).unwrap();
		write_fixture_archive(&fence_path, &record);
		Query::update()
			.table(Alias::new("memory_units"))
			.value(Alias::new("learned_at"), unit.learned_at)
			.and_where(Expr::col("id").eq(Expr::value(unit.id)))
			.to_string(PostgresQueryBuilder)
	};

	// Act: everything is within retention
	let fresh = impact(&store, &actor, query.clone()).await.unwrap();

	// Assert
	assert_eq!(fresh.units[0].readable_revision, Some(1));
	assert_eq!(
		fresh.units[0]
			.derived
			.iter()
			.map(|unit| unit.id)
			.collect::<Vec<_>>(),
		vec![derived[0].id, supported.id],
		"a withdrawn dependent is not live derived memory"
	);

	// Act: the live dependent expires before any sweep
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	native::query(&age(&derived[0]))
		.execute(&mut *tx)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	let dependent_expired = impact(&store, &actor, query.clone()).await.unwrap();

	// Assert
	assert_eq!(
		dependent_expired.units[0]
			.derived
			.iter()
			.map(|unit| unit.id)
			.collect::<Vec<_>>(),
		vec![supported.id]
	);
	assert_eq!(dependent_expired.units[0].readable_revision, Some(1));
	assert_eq!(
		dependent_expired.units[0]
			.runs
			.iter()
			.map(|run| (run.id, run.fenced))
			.collect::<Vec<_>>(),
		vec![(run, false)]
	);

	// Act: the root expires before any sweep
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	native::query(&age(&source))
		.execute(&mut *tx)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	let root_expired = impact(&store, &actor, query.clone()).await.unwrap();

	// Assert
	assert_eq!(root_expired.units[0].readable_revision, None);
	assert_eq!(
		root_expired.units[0]
			.runs
			.iter()
			.map(|run| (run.id, run.read_revision, run.fenced))
			.collect::<Vec<_>>(),
		vec![(run, 1, true)],
		"the next Run boundary would stop with Invalidated"
	);

	// Act: the dependent is unexpired, but the Unit it rests on has expired
	let unsupported = impact(
		&store,
		&actor,
		ImpactQuery {
			units: vec![supported.id],
			..query
		},
	)
	.await
	.unwrap();

	// Assert
	assert_eq!(
		unsupported.units[0].readable_revision, None,
		"listing and the Run recheck reject a Unit without current support"
	);
	assert_eq!(
		unsupported.units[0]
			.runs
			.iter()
			.map(|run| (run.id, run.read_revision, run.fenced))
			.collect::<Vec<_>>(),
		vec![(run, 1, true)]
	);
}
