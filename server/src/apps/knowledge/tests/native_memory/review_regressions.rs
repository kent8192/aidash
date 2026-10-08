//! Review regressions exercise real admission, recovery, retention, and Run reads.
use super::*;
use aidash_server::database::native;

#[rstest]
#[case("workspace")]
#[case("settings")]
#[tokio::test]
async fn retention_skips_a_full_busy_page_and_retries_after_release(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] busy: &str,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let mut candidates = Vec::new();
	for _ in 0..32 {
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
		candidates.push(review_delivery::candidate(&store, &bank).await);
	}
	let later_workspace = store
		.create_workspace("Later bank", "Maintenance must reach this bank")
		.await
		.unwrap();
	let mut db = database.lease.handle();
	AuthorizationWorkspace::objects()
		.create_with_conn(
			&mut db,
			&AuthorizationWorkspace::build()
				.workspace_id(later_workspace.id)
				.tenant("acme")
				.owner_subject("operator")
				.finish(),
		)
		.await
		.unwrap();
	let index = aidash_server::semantic::service::get_index(&store, &Actor::Operator, workspace)
		.await
		.unwrap();
	aidash_server::semantic::service::configure(
		&store,
		later_workspace.id,
		aidash_server::semantic::ConfigureIndex {
			expected_revision: 0,
			spec: serde_json::from_value(index.spec).unwrap(),
		},
	)
	.await
	.unwrap();
	let later_bank = memory::create_participant(
		&store,
		&Actor::Operator,
		later_workspace.id,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let later = review_delivery::candidate(&store, &later_bank).await;
	let old = chrono::Utc::now() - chrono::Duration::days(8);
	let mut tx = native::begin(&store.pool).await.unwrap();
	for query in [
		Query::update()
			.table(Alias::new("memory_candidates"))
			.value(Alias::new("created_at"), old)
			.to_string(PostgresQueryBuilder),
		Query::update()
			.table(Alias::new("memory_bank_settings"))
			.value(Alias::new("next_maintenance"), old)
			.to_string(PostgresQueryBuilder),
		Query::update()
			.table(Alias::new("memory_bank_settings"))
			.value(
				Alias::new("next_maintenance"),
				old + chrono::Duration::days(1),
			)
			.and_where(
				Expr::col("bank_id").in_subquery(
					Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("memory_banks"))
						.and_where(Expr::col("workspace_id").eq(Expr::value(later_workspace.id)))
						.to_owned(),
				),
			)
			.to_string(PostgresQueryBuilder),
	] {
		native::query(&query).execute(&mut *tx).await.unwrap();
	}
	tx.commit().await.unwrap();
	let mut reader = native::begin(&store.pool).await.unwrap();
	let mut locked = Query::select();
	if busy == "workspace" {
		locked
			.column(Alias::new("id"))
			.from(Alias::new("workspaces"))
			.and_where(Expr::col("id").eq(Expr::value(workspace)));
	} else {
		locked
			.column(Alias::new("bank_id"))
			.from(Alias::new("memory_bank_settings"))
			.and_where(
				Expr::col("bank_id").in_subquery(
					Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("memory_banks"))
						.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
						.to_owned(),
				),
			);
	}
	locked.lock(reinhardt::query::LockType::Share);
	native::query(&locked.to_string(PostgresQueryBuilder))
		.fetch_all(&mut *reader)
		.await
		.unwrap();
	tokio::time::timeout(
		std::time::Duration::from_secs(5),
		aidash_server::semantic::worker::sweep(&store),
	)
	.await
	.expect("maintenance must skip the full busy page without waiting")
	.unwrap();
	async fn disposition(store: &Store, id: Uuid) -> (String, String) {
		let row = native::query(
			&Query::select()
				.columns(["text", "state"].map(Alias::new))
				.from(Alias::new("memory_candidates"))
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&store.pool)
		.await
		.unwrap();
		(row.try_get("text").unwrap(), row.try_get("state").unwrap())
	}
	assert_eq!(
		disposition(&store, later).await,
		(String::new(), "invalidated".into()),
		"an unlocked later bank must not starve behind 32 busy banks"
	);
	for id in &candidates {
		assert_eq!(
			disposition(&store, *id).await,
			("Candidate to review".into(), "pending".into())
		);
	}
	reader.rollback().await.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	for id in candidates {
		assert_eq!(
			disposition(&store, id).await,
			(String::new(), "invalidated".into())
		);
	}
}

#[rstest]
#[case(Kind::Observation, Kind::World)]
#[case(Kind::Observation, Kind::Experience)]
#[case(Kind::MentalModel, Kind::World)]
#[case(Kind::MentalModel, Kind::Experience)]
#[tokio::test]
async fn derived_corrections_preserve_kind_and_require_derive_authority(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] kind: Kind,
	#[case] replacement: Kind,
) {
	use axum::{Json, Router, routing::post};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let app = Router::new().route("/v1/chat/completions", post(|Json(input): Json<serde_json::Value>| async move {
        let context: serde_json::Value = serde_json::from_str(input["messages"][1]["content"].as_str().unwrap()).unwrap();
        let units: Vec<Unit> = serde_json::from_value(context["units"].clone()).unwrap();
        let mut output = content("Derived from admitted evidence");
        output.kind = serde_json::from_value(context["kind"].clone()).unwrap();
        output.mental_model = serde_json::from_value(context["mental_model"].clone()).unwrap();
        output.evidence = units.iter().map(Unit::evidence).collect();
        Json(json!({"choices":[{"finish_reason":"stop","message":{"content":serde_json::to_string(&output).unwrap()}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
    }));
	let server = tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	});
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
	let source = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("Admitted support"),
			},
		),
	)
	.await
	.unwrap()
	.remove(0);
	let mut draft = content("Derived draft");
	draft.kind = kind;
	draft.evidence = vec![source.evidence()];
	if kind == Kind::MentalModel {
		draft.mental_model = Some(MentalModel {
			question: "Recurring question?".into(),
			automatic_refresh: false,
		});
	}
	let id = Uuid::now_v7();
	let input = mutation(&bank, Change::Add { id, content: draft });
	let memory::Outcome::Units(derived) = memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: input.operation_id,
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::Derive {
				kind,
				sources: vec![source.evidence()],
				mutation: input,
			},
		},
	)
	.await
	.unwrap() else {
		panic!("derived unit");
	};
	let candidate = super::review_delivery::candidate(&store, &bank).await;
	let mut reviewed = content("Reviewed replacement");
	reviewed.kind = replacement;
	let reviewed = mutation(
		&bank,
		Change::Correct {
			id,
			expected_revision: 1,
			content: reviewed,
		},
	);
	let result = memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: reviewed.operation_id,
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::Review {
				id: candidate,
				expected_revision: 1,
				mutation: Some(reviewed),
			},
		},
	)
	.await;
	assert!(
		matches!(result, Err(aidash_server::Error::Invalid(ref message)) if message.contains("derive operation")),
		"{result:?}"
	);
	let authorization = aidash_server::authorization::Authorization {
		pool: store.pool.clone(),
	};
	authorization.replace("acme", 1, serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[
        {"id":"allow","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},
        {"id":"write-only","effect":"deny","subjects":{"any":true},"actions":["memory.read","memory.derive"],"resources":{"kinds":["*"]}}
    ]})).unwrap(), "operator").await.unwrap();
	for entry in ["m", "e", "r", "t", "p", "a"] {
		authorization
			.set_catalog("acme", &reference(entry), 0, true, "operator")
			.await
			.unwrap();
	}
	let credential = authorization
		.issue_credential("acme", "alice", 3600, "operator")
		.await
		.unwrap();
	let writer = authorization.authenticate(&credential.token).await.unwrap();
	for actor in [&Actor::Operator, &writer] {
		let mut body = derived[0].content.clone();
		body.kind = replacement;
		body.mental_model = None;
		assert!(
			matches!(memory::mutate(&store, actor, mutation(&bank, Change::Correct { id, expected_revision: 1, content: body })).await,
            Err(aidash_server::Error::Invalid(message)) if message.contains("derive operation"))
		);
	}
	assert!(matches!(
		memory::mutate(
			&store,
			&writer,
			mutation(
				&bank,
				Change::Correct {
					id,
					expected_revision: 1,
					content: derived[0].content.clone()
				}
			)
		)
		.await,
		Err(aidash_server::Error::Forbidden)
	));
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
	assert_eq!(current.iter().find(|unit| unit.id == id), Some(&derived[0]));
	server.abort();
}

#[rstest]
#[tokio::test]
async fn run_journal_above_1024_checks_revisions_on_later_pages(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
) {
	bounds.max_units = 2048;
	bounds.max_candidates = 32;
	let database = database.await;
	let (store, _, indexed) = setup(&database, bounds).await;
	// Admit the bank against a valid index, then remove the disposable search
	// service. Its source quota is independent of the Run lifetime read journal.
	let workspace = store
		.create_workspace("Long Run", "Accumulate admitted recall pages")
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
	spec.max_sources = 64;
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
	let mut tx = native::begin(&store.pool).await.unwrap();
	for table in ["semantic_collections", "semantic_indexes"] {
		native::query(
			&Query::delete()
				.from_table(Alias::new(table))
				.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
	}
	tx.commit().await.unwrap();
	let mut ids: Vec<_> = (0..1025).map(|_| Uuid::now_v7()).collect();
	ids.sort();
	for batch in ids.chunks(32) {
		memory::mutate(
			&store,
			&Actor::Operator,
			Mutation {
				operation_id: Uuid::now_v7(),
				provider: reference("p"),
				bank: bank.clone(),
				changes: batch
					.iter()
					.map(|id| Change::Add {
						id: *id,
						content: content("Current lifetime Run dependency"),
					})
					.collect(),
			},
		)
		.await
		.unwrap();
	}
	let (scope, run) = policy_bounds::recorded_run(&store, &bank, &ids).await;
	assert!(
		policy_bounds::run_visible(&scope, &store, run).await,
		"all 1025 admitted revisions remain readable"
	);
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Correct {
				id: *ids.last().unwrap(),
				expected_revision: 1,
				content: content("Corrected late-page dependency"),
			},
		),
	)
	.await
	.unwrap();
	assert!(
		!policy_bounds::run_visible(&scope, &store, run).await,
		"a correction beyond the first 1024 records must fence the Run"
	);
}

#[rstest]
#[tokio::test]
async fn restore_propagates_withholding_beyond_recall_hop_limit(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_server::semantic::services::memory_recovery as recovery;
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;

	let authorization = aidash_server::authorization::Authorization {
		pool: store.pool.clone(),
	};
	authorization.replace("acme", 1, serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[{"id":"allow","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]})).unwrap(), "operator").await.unwrap();
	for entry in ["m", "e", "r", "t", "p", "a"] {
		authorization
			.set_catalog("acme", &reference(entry), 0, true, "operator")
			.await
			.unwrap();
	}
	let credential = authorization
		.issue_credential("acme", "alice", 3600, "operator")
		.await
		.unwrap();
	let writer = authorization.authenticate(&credential.token).await.unwrap();
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
	let message = Uuid::now_v7();
	let body = "Primary authority to withdraw";
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::insert()
			.into_table(Alias::new("messages"))
			.columns(
				[
					"id",
					"workspace_id",
					"sender",
					"content",
					"idempotency_key",
					"created_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(message))
					.expr(Expr::value(workspace))
					.expr(Expr::value("operator"))
					.expr(Expr::value(body))
					.expr(Expr::value(format!("primary-{message}")))
					.expr(Expr::value(chrono::Utc::now()))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let mut evidence = Evidence::Message {
		id: message,
		revision: 1,
		digest: aidash_domain::semantic::indexing::content_digest(body),
	};
	// Each dependent sorts before its parent, requiring another invalidation pass.
	for offset in 0..8 {
		let mut body = content("Quoted primary authority");
		body.evidence = vec![evidence];
		let unit = memory::mutate(
			&store,
			if offset == 0 {
				&writer
			} else {
				&Actor::Operator
			},
			mutation(
				&bank,
				Change::Add {
					id: Uuid::from_u128(1000 - offset),
					content: body,
				},
			),
		)
		.await
		.unwrap()
		.remove(0);
		evidence = unit.evidence();
	}
	let directory = database.recovery_directory.path();
	let archive = recovery::backup(&store, directory, bank.clone())
		.await
		.unwrap();

	// Only the root writer loses authority. Its primary message remains current,
	// so dependents are invalidated only after their parent is withheld.
	authorization.replace("acme", 2, serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[
        {"id":"allow","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},
        {"id":"revoke-root-reader","effect":"deny","subjects":{"any":true},"actions":["memory.read"],"resources":{"kinds":["*"]}}
    ]})).unwrap(), "operator").await.unwrap();
	let report = recovery::restore(&store, directory, &archive)
		.await
		.unwrap();
	assert_eq!((report.restored, report.withheld), (0, 8));
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
		.is_empty(),
		"successful restore reopens the gate with all revoked dependents withheld"
	);
	let remaining: i64 = native::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(Expr::col("id").into()))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("text").ne(""))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(remaining, 0);
}

#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn candidate_expiry_erases_occurrence_dates_without_repeating_cas(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] previously_cleared: bool,
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
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("Initialize pinned bank policy"),
			},
		),
	)
	.await
	.unwrap();
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
	let candidate = Uuid::now_v7();
	let old = chrono::Utc::now() - chrono::Duration::days(8);
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
					"occurred_start",
					"occurred_end",
					"entities",
					"evidence",
					"links",
					"state",
					"created_at",
					"updated_at",
					"mental_model",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(candidate))
					.expr(Expr::value(bank_id))
					.expr(Expr::value(1_i64))
					.expr(Expr::value(json!(Evidence::Run {
						id: Uuid::now_v7(),
						revision: 1,
						digest: "fixture".into()
					})))
					.expr(Expr::value(if previously_cleared {
						""
					} else {
						"Private event description"
					}))
					.expr(Expr::value("world"))
					.expr(Expr::value("fact"))
					.expr(Expr::value("unverified"))
					.expr(Expr::value(old))
					.expr(Expr::value(old + chrono::Duration::hours(1)))
					.expr(Expr::value(json!([])))
					.expr(Expr::value(json!([])))
					.expr(Expr::value(json!([])))
					.expr(Expr::value("pending"))
					.expr(Expr::value(old))
					.expr(Expr::value(old))
					.expr(Expr::value(Some(
						json!({"question":"private occurrence", "automatic_refresh":false}),
					)))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	for _ in 0..2 {
		let mut tx = native::begin(&store.pool).await.unwrap();
		native::query(
			&Query::update()
				.table(Alias::new("memory_bank_settings"))
				.value(Alias::new("next_maintenance"), old)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
		tx.commit().await.unwrap();
		aidash_server::semantic::worker::sweep(&store)
			.await
			.unwrap();
		let row = native::query(
			&Query::select()
				.columns(
					[
						"text",
						"occurred_start",
						"occurred_end",
						"mental_model",
						"entities",
						"evidence",
						"links",
						"state",
						"revision",
					]
					.map(Alias::new),
				)
				.from(Alias::new("memory_candidates"))
				.and_where(Expr::col("id").eq(Expr::value(candidate)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&store.pool)
		.await
		.unwrap();
		assert_eq!(row.try_get::<String>("text").unwrap(), "");
		assert_eq!(
			row.try_get::<Option<chrono::DateTime<chrono::Utc>>>("occurred_start")
				.unwrap(),
			None
		);
		assert_eq!(
			row.try_get::<Option<chrono::DateTime<chrono::Utc>>>("occurred_end")
				.unwrap(),
			None
		);
		assert_eq!(
			row.try_get::<Option<serde_json::Value>>("mental_model")
				.unwrap(),
			None
		);
		for field in ["entities", "evidence", "links"] {
			assert_eq!(row.try_get::<serde_json::Value>(field).unwrap(), json!([]));
		}
		assert_eq!(row.try_get::<String>("state").unwrap(), "invalidated");
		assert_eq!(
			row.try_get::<i64>("revision").unwrap(),
			2,
			"expiry preserves a content-free disposition and advances CAS once"
		);
	}
}

#[rstest]
#[case(Kind::World)]
#[case(Kind::Experience)]
#[tokio::test]
async fn stale_ordinary_dependents_retire_without_ttl_and_release_live_capacity(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
	#[case] kind: Kind,
) {
	bounds.max_units = 2;
	bounds.max_candidates = 2;
	bounds.max_results = 2;
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
	let root = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("Original source"),
			},
		),
	)
	.await
	.unwrap()
	.remove(0);
	let mut dependent = content("Obsolete supported conclusion");
	dependent.kind = kind;
	dependent.evidence = vec![root.evidence()];
	let dependent = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: dependent,
			},
		),
	)
	.await
	.unwrap()
	.remove(0);
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Correct {
				id: root.id,
				expected_revision: 1,
				content: content("Corrected source"),
			},
		),
	)
	.await
	.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let row = native::query(
		&Query::select()
			.columns(["deleted", "text"].map(Alias::new))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("id").eq(Expr::value(dependent.id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&store.pool)
	.await
	.unwrap();
	assert!(row.try_get::<bool>("deleted").unwrap());
	assert_eq!(row.try_get::<String>("text").unwrap(), "");
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_purge_jobs"))
			.value(
				Alias::new("purge_after"),
				chrono::Utc::now() - chrono::Duration::seconds(1),
			)
			.and_where(Expr::col("unit_id").eq(Expr::value(dependent.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let state: String = native::query_scalar(
		&Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("memory_purge_jobs"))
			.and_where(Expr::col("unit_id").eq(Expr::value(dependent.id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(state, "purged");
	let replaced = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("New current knowledge"),
			},
		),
	)
	.await
	.unwrap();
	assert_eq!(replaced.len(), 1);
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
		2
	);
}
