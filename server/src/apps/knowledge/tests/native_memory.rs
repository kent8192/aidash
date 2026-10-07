//! Real PostgreSQL tests: unit concurrency, provenance fences and native vector scopes.
use crate::native_database::{DatabaseFixture, database};
use aidash_application::ports::VectorIndex;
use aidash_domain::{memory::*, registry::EntityRef};
use aidash_server::{
	apps::{
		identity::models::{AuthorizationBundle, AuthorizationWorkspace},
		knowledge::services::native_memory as memory,
	},
	authorization::identity::Actor,
	registry::{Entry, Registry},
	store::Store,
};
use reinhardt::query::types::IntoIden;
use reinhardt::{
	db::orm::{Json, Model},
	query::{Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder},
};
use rstest::{fixture, rstest};
use serde_json::json;
use uuid::Uuid;
#[path = "native_memory/admission.rs"]
mod admission;
#[path = "native_memory/policy_bounds.rs"]
mod policy_bounds;
#[path = "native_memory/precision.rs"]
mod precision;
#[path = "native_memory/purge.rs"]
mod purge;
#[path = "../../execution/tests/support/deployment.rs"]
mod recovery_deployment;
#[path = "native_memory/review_capacity.rs"]
mod review_capacity;
#[path = "native_memory/review_delivery.rs"]
mod review_delivery;
#[path = "native_memory/review_regressions.rs"]
mod review_regressions;

#[derive(serde::Serialize, serde::Deserialize)]
struct RecoveryFence {
	epoch: Uuid,
	id: Uuid,
	fence: recovery::Fence,
}

#[rstest]
#[tokio::test]
async fn memory_ttl_withholds_units_and_dependents_before_cleanup_and_after_restore(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_server::{database::native, semantic::services::memory_recovery as recovery};
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
	let source = Uuid::now_v7();
	let admission = mutation(
		&bank,
		Change::Add {
			id: source,
			content: content("期限のある知見 / Expiring findings"),
		},
	);
	let original = memory::mutate(&store, &Actor::Operator, admission.clone())
		.await
		.unwrap();
	let derived = Uuid::now_v7();
	let mut observation = content("期限のある知見に依存 / Depends on expiring findings");
	observation.kind = Kind::World;
	observation.evidence = vec![original[0].evidence()];
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: derived,
				content: observation,
			},
		),
	)
	.await
	.unwrap();
	let directory = database.recovery_directory.path();
	let archive = recovery::backup(&store, directory, bank.clone())
		.await
		.unwrap();
	// Rebase the fixture's clock in both canonical storage and its independent
	// fence. Only time changes; production mutation/recovery paths stay intact.
	let mut aged = original[0].clone();
	aged.learned_at = chrono::DateTime::from_timestamp_micros(
		(chrono::Utc::now() - chrono::Duration::days(2)).timestamp_micros(),
	)
	.unwrap();
	let fence_path = directory.join("units").join(format!("{}.cbor", aged.id));
	let bytes = std::fs::read(&fence_path).unwrap();
	let mut record: RecoveryFence = ciborium::de::from_reader(&bytes[40..]).unwrap();
	// Fixture clock injection cannot use production's same-revision guard.
	record.fence.digest = aidash_domain::memory::recovery::digest(&aged).unwrap();
	write_fixture_archive(&fence_path, &record);
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_units"))
			.value(Alias::new("learned_at"), aged.learned_at)
			.and_where(Expr::col("id").eq(Expr::value(source)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let read = memory::ReadBank {
		provider: reference("p"),
		bank: bank.clone(),
	};
	assert!(
		memory::list(&store, &Actor::Operator, read.clone())
			.await
			.unwrap()
			.is_empty(),
		"both the expired unit and its fresh dependent disappear before a sweep"
	);
	assert!(
		matches!(
			memory::mutate(&store, &Actor::Operator, admission).await,
			Err(aidash_server::Error::Conflict(_))
		),
		"an old receipt cannot deliver an expired body"
	);
	let history = memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::History {
				id: source,
				expected_revision: 1,
				before: None,
			},
		},
	)
	.await
	.unwrap();
	let memory::Outcome::History(history) = history else {
		panic!("history metadata")
	};
	assert!(!history.is_empty());
	assert!(history.iter().all(|version| version.content.is_none()));
	let report = recovery::restore(&store, directory, &archive)
		.await
		.unwrap();
	assert_eq!((report.restored, report.withheld), (0, 2));
	assert!(
		memory::list(&store, &Actor::Operator, read)
			.await
			.unwrap()
			.is_empty()
	);
	let text: Vec<String> = native::query_scalar(
		&Query::select()
			.column(Alias::new("text"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("id").is_in([source, derived].map(Expr::value)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&store.pool)
	.await
	.unwrap();
	assert_eq!(text.len(), 2);
	assert!(
		text.iter().all(String::is_empty),
		"restore removes both expired and unsupported bodies"
	);
}

fn write_fixture_archive<T: serde::Serialize>(path: &std::path::Path, value: &T) {
	use sha2::Digest;
	let mut payload = vec![];
	ciborium::ser::into_writer(value, &mut payload).unwrap();
	let mut bytes = b"AIDMEM01".to_vec();
	bytes.extend(sha2::Sha256::digest(&payload));
	bytes.extend(payload);
	std::fs::write(path, bytes).unwrap();
}

#[rstest]
#[tokio::test]
async fn memory_restore_withholds_removed_primary_messages_and_prunes_expired_archive_files(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_server::{database::native, semantic::services::memory_recovery as recovery};
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let participant = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	let message = Uuid::now_v7();
	let body = "撤回する元メッセージ / Primary message to withdraw";
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
	let id = Uuid::now_v7();
	let mut quoted = content(body);
	quoted.evidence = vec![Evidence::Message {
		id: message,
		revision: 1,
		digest: aidash_domain::semantic::indexing::content_digest(body),
	}];
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&participant.bank,
			Change::Add {
				id,
				content: quoted,
			},
		),
	)
	.await
	.unwrap();
	let directory = database.recovery_directory.path();
	let archive = recovery::backup(&store, directory, participant.bank.clone())
		.await
		.unwrap();
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::delete()
			.from_table(Alias::new("messages"))
			.and_where(Expr::col("id").eq(Expr::value(message)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let report = recovery::restore(&store, directory, &archive)
		.await
		.unwrap();
	assert_eq!((report.restored, report.withheld), (0, 1));
	let text: String = native::query_scalar(
		&Query::select()
			.column(Alias::new("text"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert!(
		text.is_empty(),
		"restore clears quotes whose primary evidence was removed"
	);
	assert!(
		memory::list(
			&store,
			&Actor::Operator,
			memory::ReadBank {
				provider: reference("p"),
				bank: participant.bank,
			}
		)
		.await
		.unwrap()
		.is_empty()
	);

	let expired = archive.with_file_name("expired-fixture.cbor");
	let bytes = std::fs::read(&archive).unwrap();
	assert_eq!(&bytes[..8], b"AIDMEM01");
	let mut encoded: ciborium::value::Value = ciborium::de::from_reader(&bytes[40..]).unwrap();
	let ciborium::value::Value::Map(fields) = &mut encoded else {
		panic!("typed archive map")
	};
	for (key, value) in fields {
		if key.as_text() == Some("created_at") {
			*value = ciborium::value::Value::Text(
				(chrono::Utc::now() - chrono::Duration::days(9)).to_rfc3339(),
			);
		}
		if key.as_text() == Some("expires_at") {
			*value = ciborium::value::Value::Text(
				(chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339(),
			);
		}
	}
	use sha2::Digest;
	let mut payload = vec![];
	ciborium::ser::into_writer(&encoded, &mut payload).unwrap();
	let mut expired_bytes = b"AIDMEM01".to_vec();
	expired_bytes.extend(sha2::Sha256::digest(&payload));
	expired_bytes.extend(payload);
	std::fs::write(&expired, expired_bytes).unwrap();
	assert!(
		recovery::restore(&store, directory, &expired)
			.await
			.is_err()
	);
	assert!(expired.is_file());
	recovery::prune_expired(&store, directory).unwrap();
	assert!(
		!expired.exists(),
		"expiry physically removes the managed CBOR file"
	);
	assert!(
		archive.is_file(),
		"a still-live archive keeps its declared retention horizon"
	);
}

#[cfg(unix)]
#[rstest]
#[tokio::test]
async fn memory_restore_process_sigkill_keeps_the_external_gate_closed_until_validation(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_server::{database::native, semantic::services::memory_recovery as recovery};
	use axum::{Json, Router, routing::post};
	use std::os::unix::process::ExitStatusExt;
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, Router::new().route("/v1/embeddings", post(|Json(input): Json<serde_json::Value>| async move {
			Json(json!({"model":input["model"],"data":[{"index":0,"embedding":[1.,0.1,0.]}],"usage":{"prompt_tokens":1}}))
		}))).await.unwrap();
	});
	let database = database.await;
	let (store, _, workspace) = setup_endpoint(&database, bounds, &endpoint).await;
	let participant = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	let id = Uuid::now_v7();
	let admitted = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&participant.bank,
			Change::Add {
				id,
				content: content("Validate after interrupted restore / 復元中断後に検証"),
			},
		),
	)
	.await
	.unwrap();
	let directory = database.recovery_directory.path();
	let archive = recovery::backup(&store, directory, participant.bank.clone())
		.await
		.unwrap();
	// Startup initializes statement triggers. Hold the file boundary first,
	// then acquire the row after startup reaches recovery's independent lock.
	let boundary = std::fs::File::options()
		.read(true)
		.write(true)
		.open(directory.join("ledger.lock"))
		.unwrap();
	boundary.lock().unwrap();
	let deployment = tempfile::tempdir().unwrap();
	let log_path = deployment.path().join("interrupted-memory-restore.log");
	let mut command = recovery_deployment::deployment_command(&database.url, deployment.path());
	command
		.kill_on_drop(true)
		.env("AIDASH_NODE_ID", &store.node_id)
		.env("AIDASH_MEMORY_RECOVERY_DIR", directory)
		.args(["memory-recovery", "restore", "--directory"])
		.arg(directory)
		.arg("--archive")
		.arg(&archive)
		.stdout(std::fs::File::create(&log_path).unwrap())
		.stderr(
			std::fs::File::options()
				.append(true)
				.open(&log_path)
				.unwrap(),
		);
	let mut child = command.spawn().unwrap();
	let startup = tokio::time::timeout(std::time::Duration::from_secs(30), async {
		loop {
			assert!(
				child.try_wait().unwrap().is_none(),
				"restore startup failed: {}",
				std::fs::read_to_string(&log_path).unwrap()
			);
			let pid = child.id().unwrap();
			#[cfg(target_os = "linux")]
			let entered = std::fs::read_dir(format!("/proc/{pid}/fd"))
				.unwrap()
				.flatten()
				.filter_map(|entry| std::fs::read_link(entry.path()).ok())
				.any(|path| path == directory.join("ledger.lock"));
			#[cfg(not(target_os = "linux"))]
			let entered = tokio::process::Command::new("lsof")
				.args(["-a", "-p", &pid.to_string(), "-Fn"])
				.arg(directory.join("ledger.lock"))
				.output()
				.await
				.unwrap()
				.status
				.success();
			if entered {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
		}
	})
	.await;
	assert!(
		startup.is_ok(),
		"shipped restore must reach the recovery file boundary after startup migrations: {}",
		std::fs::read_to_string(&log_path).unwrap()
	);
	let mut held = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("workspaces"))
			.and_where(Expr::col("id").eq(Expr::value(workspace)))
			.lock(reinhardt::query::LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *held)
	.await
	.unwrap();

	boundary.unlock().unwrap();

	tokio::time::timeout(std::time::Duration::from_secs(30), async {
		loop {
			assert!(
				child.try_wait().unwrap().is_none(),
				"restore exited before its gate closed: {}",
				std::fs::read_to_string(&log_path).unwrap()
			);
			let read = memory::ReadBank {
				provider: reference("p"),
				bank: participant.bank.clone(),
			};
			if matches!(
				tokio::time::timeout(
					std::time::Duration::from_millis(100),
					memory::list(&store, &Actor::Operator, read)
				)
				.await,
				Ok(Err(aidash_server::Error::SemanticUnavailable))
			) {
				break;
			}

			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
		}
	})
	.await
	.expect("restore must close the independent gate before waiting for the held database row");
	child.start_kill().unwrap();
	let killed = child.wait().await.unwrap();
	assert_eq!(
		killed.signal(),
		Some(9),
		"the shipped restore process was actually killed by SIGKILL"
	);
	held.rollback().await.unwrap();
	let reopened = recovery::attach(store.clone(), directory.to_owned()).unwrap();
	let read = memory::ReadBank {
		provider: reference("p"),
		bank: participant.bank,
	};
	assert!(
		memory::list(&reopened, &Actor::Operator, read.clone())
			.await
			.is_err()
	);
	let report = recovery::restore(&reopened, directory, &archive)
		.await
		.unwrap();
	assert_eq!((report.restored, report.withheld), (1, 0));
	assert_eq!(
		memory::list(&reopened, &Actor::Operator, read)
			.await
			.unwrap(),
		admitted
	);
	server.abort();
}

fn reference(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
}
fn entry(kind: &str, id: &str, config: serde_json::Value) -> Entry {
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id,"ja":id},"description":{"en":"fixture"},
		"capabilities":[],"languages":["en","ja"],"config":config})).unwrap()
}
#[fixture]
fn bounds() -> Bounds {
	Bounds {
		max_unit_bytes: 8192,
		max_input_bytes: 16384,
		max_units: 16,
		max_candidates: 8,
		max_entities: 8,
		max_evidence: 8,
		max_links: 8,
		max_graph_hops: 3,
		max_graph_visits: 32,
		max_results: 4,
		max_context_tokens: 8192,
		max_model_calls: 4,
		max_model_tokens: 8192,
		max_cost_micros: 10000,
		max_retries: 2,
		max_call_seconds: 30,
	}
}
async fn setup(database: &DatabaseFixture, bounds: Bounds) -> (Store, Registry, Uuid) {
	setup_endpoint(database, bounds, "http://127.0.0.1:9/v1").await
}
async fn setup_endpoint(
	database: &DatabaseFixture,
	bounds: Bounds,
	endpoint: &str,
) -> (Store, Registry, Uuid) {
	setup_endpoint_flags(database, bounds, endpoint, (false, false, false)).await
}
async fn setup_endpoint_flags(
	database: &DatabaseFixture,
	bounds: Bounds,
	endpoint: &str,
	flags: (bool, bool, bool),
) -> (Store, Registry, Uuid) {
	setup_endpoint_retention(database, bounds, endpoint, flags, None).await
}
async fn setup_endpoint_retention(
	database: &DatabaseFixture,
	bounds: Bounds,
	endpoint: &str,
	flags: (bool, bool, bool),
	unit_max_age_days: Option<u32>,
) -> (Store, Registry, Uuid) {
	let store = Store::from_pool(
		database.connection.clone().into_postgres().unwrap(),
		"aidash://native-memory".into(),
	)
	.await
	.unwrap();
	let store = aidash_server::semantic::services::memory_recovery::initialize(
		&store,
		database.recovery_directory.path().to_owned(),
	)
	.await
	.unwrap();
	let registry = Registry::new(store.pool.clone(), &store.node_id).unwrap();
	for (kind, id, config) in [
		(
			"model",
			"m",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":endpoint,"credential_env":null,
			"context_window":32768,"max_output_tokens":8192,"modalities":["text"],"cost":{}}),
		),
		(
			"embedding",
			"e",
			json!({"provider":"openai","endpoint":endpoint,"credential_env":null,"model":"fixture","model_version":"1.0.0","dimensions":3}),
		),
		("reranker", "r", json!({"provider":"rrf"})),
		("tokenizer", "t", json!({"provider":"utf8_upper_bound"})),
		(
			"memory",
			"p",
			serde_json::to_value(ProviderConfig {
				engine: EngineKind::HindsightRust,
				policy: Policy {
					extraction: reference("m"),
					derivation: reference("m"),
					reflection: reference("m"),
					embedding: reference("e"),
					reranker: reference("r"),
					tokenizer: reference("t"),
					semantic_link_min_similarity_millionths: 700_000,
					retention: Retention {
						unit_max_age_days,
						candidate_days: 7,
						history_days: 30,
						history_versions: 16,
						model_result_days: 7,
						backup_days: 7,
						purge_after_seconds: 60,
						purge_batch: 32,
						max_unit_records: 128.max(bounds.max_units),
						max_model_operations: 1024,
					},
					prices: Prices {
						extraction: Rate {
							input_per_million: 0,
							output_per_million: 0,
						},
						derivation: Rate {
							input_per_million: 0,
							output_per_million: 0,
						},
						reflection: Rate {
							input_per_million: 0,
							output_per_million: 0,
						},
						embedding: Rate {
							input_per_million: 0,
							output_per_million: 0,
						},
						reranker: Rate {
							input_per_million: 0,
							output_per_million: 0,
						},
					},
					bounds,
					learn_from_runs: flags.0,
					maintain_observations: flags.1,
					refresh_mental_models: flags.2,
				},
			})
			.unwrap(),
		),
		(
			"agent",
			"a",
			json!({"model":reference("m"),"instructions":"Use current evidence","tools":[],"skills":[],"memory":reference("p"),"allow_memory_write":true}),
		),
	] {
		registry.register(entry(kind, id, config)).await.unwrap();
	}
	let workspace = store
		.create_workspace("Memory", "Keep current evidence")
		.await
		.unwrap();
	let mut db = database.lease.handle();
	let bundle: aidash_domain::policy::PolicyBundle =
		serde_json::from_value(json!({"tenant":"acme"})).unwrap();
	AuthorizationBundle::objects()
		.create_with_conn(
			&mut db,
			&AuthorizationBundle::build()
				.tenant("acme")
				.revision(1)
				.document(Json(serde_json::to_value(bundle).unwrap()))
				.finish(),
		)
		.await
		.unwrap();
	AuthorizationWorkspace::objects()
		.create_with_conn(
			&mut db,
			&AuthorizationWorkspace::build()
				.workspace_id(workspace.id)
				.tenant("acme")
				.owner_subject("operator")
				.finish(),
		)
		.await
		.unwrap();
	aidash_server::semantic::service::configure(
		&store,
		workspace.id,
		aidash_server::semantic::ConfigureIndex {
			expected_revision: 0,
			spec: aidash_server::semantic::IndexSpec {
				embedding: serde_json::from_value(json!({"provider":"openai","endpoint":endpoint,"credential_env":null,"model":"fixture","model_version":"1.0.0","dimensions":3})).unwrap(),
				vector: aidash_domain::semantic::VectorConfig { provider: "postgres".into(), endpoint: "local".into(), credential_env: None },
				enabled: true, auto_context: false, max_sources: 64, max_results: 4,
				max_result_tokens: 8192, max_input_bytes: 16384,
			},
		},
	).await.unwrap();
	(store, registry, workspace.id)
}
fn content(text: &str) -> Content {
	Content {
		mental_model: None,
		text: text.into(),
		kind: Kind::World,
		learning: Learning::Fact,
		verification: Verification::Unverified,
		occurred: None,
		entities: vec![],
		evidence: vec![],
		links: vec![],
	}
}
fn mutation(bank: &Bank, change: Change) -> Mutation {
	Mutation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		changes: vec![change],
	}
}

#[rstest]
#[tokio::test]
async fn old_memory_archive_cannot_revive_deleted_corrected_or_dependent_bodies(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_server::{database::native, semantic::services::memory_recovery as recovery};
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let participant = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	let bank = participant.bank;
	let a = Uuid::now_v7();
	let b = Uuid::now_v7();
	let d = Uuid::now_v7();
	let first = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: a,
				content: content("東京の削除対象"),
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
			Change::Add {
				id: b,
				content: content("Before correction"),
			},
		),
	)
	.await
	.unwrap();
	let mut derived = content("Quoted 東京の削除対象");
	derived.kind = Kind::World;
	derived.evidence = vec![first.evidence()];
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: d,
				content: derived,
			},
		),
	)
	.await
	.unwrap();
	let directory = database.recovery_directory.path();
	let archive = recovery::backup(&store, directory, bank.clone())
		.await
		.unwrap();
	let mut shared = bank.clone();
	shared.participant = None;
	let published = Uuid::now_v7();
	let operation = mutation(
		&shared,
		Change::Add {
			id: published,
			content: first.content.clone(),
		},
	);
	memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: operation.operation_id,
			provider: reference("p"),
			bank: shared.clone(),
			action: memory::Action::Publish {
				source: first.evidence(),
				mutation: operation,
			},
		},
	)
	.await
	.unwrap();
	let shared_archive = recovery::backup(&store, directory, shared.clone())
		.await
		.unwrap();
	// A later CAS failure rolls back the entire batch without advancing any
	// external revision. A valid subsequent backup must still be possible.
	let failed = Mutation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		changes: vec![
			Change::Correct {
				id: b,
				expected_revision: 1,
				content: content("Must roll back"),
			},
			Change::Correct {
				id: a,
				expected_revision: 99,
				content: content("Wrong CAS"),
			},
		],
	};
	assert!(
		memory::mutate(&store, &Actor::Operator, failed)
			.await
			.is_err()
	);
	recovery::backup(&store, directory, bank.clone())
		.await
		.unwrap();
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Correct {
				id: b,
				expected_revision: 1,
				content: content("After correction"),
			},
		),
	)
	.await
	.unwrap();
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Delete {
				id: a,
				expected_revision: 1,
			},
		),
	)
	.await
	.unwrap();
	recovery::prepare_restore(&store, directory).unwrap();
	// Rewind only memory bodies. Current origin/control/authority/primary
	// evidence tables and the independently retained ledger remain live.
	let mut tx = store.pool.begin().await.unwrap();
	for (id, text) in [
		(a, "東京の削除対象"),
		(b, "Before correction"),
		(d, "Quoted 東京の削除対象"),
	] {
		native::query(
			&Query::update()
				.table(Alias::new("memory_units"))
				.value(Alias::new("text"), text)
				.value(Alias::new("revision"), 1_i64)
				.value(Alias::new("deleted"), false)
				.value(Alias::new("stale"), false)
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
	}
	tx.commit().await.unwrap();
	let report = recovery::restore(&store, directory, &archive)
		.await
		.unwrap();
	assert_eq!(report.restored, 0);
	assert_eq!(report.withheld, 3);
	let shared_report = recovery::restore(&store, directory, &shared_archive)
		.await
		.unwrap();
	assert_eq!(shared_report.restored, 0);
	assert_eq!(shared_report.withheld, 1);
	let shared_body: String = native::query_scalar(
		&Query::select()
			.column(Alias::new("text"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("id").eq(Expr::value(published)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(
		shared_body, "",
		"old shared archives cannot restore withdrawn quotations"
	);
	let purge_revision: i64 = native::query_scalar(
		&Query::select()
			.column(Alias::new("revision"))
			.from(Alias::new("memory_purge_jobs"))
			.and_where(Expr::col("unit_id").eq(Expr::value(a)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	let unit_revision: i64 = native::query_scalar(
		&Query::select()
			.column(Alias::new("revision"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("id").eq(Expr::value(a)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(
		purge_revision, unit_revision,
		"restore preserves the durable deletion receipt"
	);
	recovery::backup(&store, directory, bank.clone())
		.await
		.unwrap();
	let rows = native::query(
		&Query::select()
			.columns(["id", "revision", "text", "deleted", "stale"].map(Alias::new))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("id").is_in([a, b, d].map(Expr::value)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&store.pool)
	.await
	.unwrap();
	assert_eq!(rows.len(), 3);
	for row in rows {
		assert!(row.try_get::<i64>("revision").unwrap() >= 2);
		assert_eq!(row.try_get::<String>("text").unwrap(), "");
		assert!(row.try_get::<bool>("deleted").unwrap() || row.try_get::<bool>("stale").unwrap());
	}
	let memory::Outcome::Settings(Some(_)) = memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank,
			action: memory::Action::Settings,
		},
	)
	.await
	.unwrap() else {
		panic!("validated empty bank should reopen")
	};
}

#[rstest]
#[tokio::test]
async fn memory_restore_rebuilds_missing_vectors_and_checks_open_epoch_reads(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_server::{database::native, semantic::services::memory_recovery as recovery};
	use axum::{Json, Router, routing::post};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, Router::new().route("/v1/embeddings", post(|Json(input): Json<serde_json::Value>| async move {
			Json(json!({"model":input["model"],"data":[{"index":0,"embedding":[1.,0.1,0.]}],"usage":{"prompt_tokens":1}}))
		}))).await.unwrap();
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
	let id = Uuid::now_v7();
	let original = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("東京の地下鉄 / Tokyo subway"),
			},
		),
	)
	.await
	.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let directory = database.recovery_directory.path();
	let archive = recovery::backup(&store, directory, bank.clone())
		.await
		.unwrap();
	let mut tx = store.pool.begin().await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_units"))
			.value(Alias::new("text"), "unfenced database snapshot")
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	native::query(
		&Query::delete()
			.from_table(Alias::new("semantic_vectors"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let read = memory::ReadBank {
		bank: bank.clone(),
		provider: reference("p"),
	};
	assert!(
		matches!(
			memory::list(&store, &Actor::Operator, read.clone()).await,
			Err(aidash_server::Error::SemanticUnavailable)
		),
		"an open epoch still checks the external body fence"
	);
	let report = recovery::restore(&store, directory, &archive)
		.await
		.unwrap();
	assert_eq!(report.restored, 1);
	assert_eq!(report.withheld, 0);
	assert_eq!(
		memory::list(&store, &Actor::Operator, read).await.unwrap(),
		original
	);
	let memory::Outcome::Recall(Recall::Ready { units }) = memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank,
			action: memory::Action::Recall {
				query: RecallQuery {
					text: "地下鉄 subway".into(),
					time: None,
					kinds: vec![],
					max_tokens: 8192,
				},
			},
		},
	)
	.await
	.unwrap() else {
		panic!("restored native index must be searchable")
	};
	assert_eq!(units, original);
	server.abort();
}

#[rstest]
#[tokio::test]
async fn memory_restore_gate_survives_reopen_and_missing_external_state_never_initializes(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_server::semantic::services::memory_recovery as recovery;
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let participant = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	let bank = participant.bank;
	memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			bank: bank.clone(),
			provider: reference("p"),
			action: memory::Action::Candidates,
		},
	)
	.await
	.unwrap();
	let directory = database.recovery_directory.path();
	let archive = recovery::backup(&store, directory, bank.clone())
		.await
		.unwrap();
	let query = || memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		action: memory::Action::Settings,
	};
	let epoch = recovery::prepare_restore(&store, directory).unwrap();
	assert!(matches!(
		memory::operate(&store, &Actor::Operator, query()).await,
		Err(aidash_server::Error::SemanticUnavailable)
	));
	let reopened = Store::from_pool(store.pool.driver().clone(), store.node_id.clone())
		.await
		.unwrap();
	let reopened = recovery::initialize_if_missing(&reopened, directory.to_owned())
		.await
		.unwrap();

	assert!(matches!(
		memory::operate(&reopened, &Actor::Operator, query()).await,
		Err(aidash_server::Error::SemanticUnavailable)
	));
	let report = recovery::restore(&reopened, directory, &archive)
		.await
		.unwrap();
	assert_eq!(report.epoch, epoch);
	memory::operate(&reopened, &Actor::Operator, query())
		.await
		.unwrap();
	let bytes = std::fs::read(directory.join("ledger.cbor")).unwrap();
	std::fs::write(directory.join("ledger.cbor"), &bytes[..bytes.len() / 2]).unwrap();
	assert!(matches!(
		memory::operate(&reopened, &Actor::Operator, query()).await,
		Err(aidash_server::Error::SemanticUnavailable)
	));
	assert!(
		recovery::initialize(&reopened, directory.to_owned())
			.await
			.is_err()
	);
	assert!(
		recovery::initialize_if_missing(&reopened, directory.to_owned())
			.await
			.is_err()
	);
	std::fs::remove_file(directory.join("ledger.cbor")).unwrap();
	assert!(
		recovery::initialize(&reopened, directory.to_owned())
			.await
			.is_err()
	);
	assert!(
		recovery::initialize_if_missing(&reopened, directory.to_owned())
			.await
			.is_err()
	);
	assert!(matches!(
		memory::operate(&reopened, &Actor::Operator, query()).await,
		Err(aidash_server::Error::SemanticUnavailable)
	));
	assert!(
		recovery::restore(&reopened, directory, &archive)
			.await
			.is_err()
	);
}

#[rstest]
#[tokio::test]
async fn shared_policy_changes_are_idempotent_and_compare_the_observed_revision(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = Bank {
		home: store.node_id.clone(),
		tenant: "acme".into(),
		workspace,
		participant: None,
	};
	let input = memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		action: memory::Action::ConfigureBank {
			expected_revision: 0,
		},
	};
	let memory::Outcome::Settings(Some(first)) =
		memory::operate(&store, &Actor::Operator, input.clone())
			.await
			.unwrap()
	else {
		panic!()
	};
	let memory::Outcome::Settings(Some(replay)) =
		memory::operate(&store, &Actor::Operator, input.clone())
			.await
			.unwrap()
	else {
		panic!()
	};
	assert_eq!(first.revision, 1);
	assert_eq!(replay.revision, first.revision);
	let mut changed = input.clone();
	changed.action = memory::Action::ConfigureBank {
		expected_revision: 1,
	};
	assert!(
		memory::operate(&store, &Actor::Operator, changed.clone())
			.await
			.is_err()
	);
	changed.operation_id = Uuid::now_v7();
	let memory::Outcome::Settings(Some(second)) =
		memory::operate(&store, &Actor::Operator, changed)
			.await
			.unwrap()
	else {
		panic!()
	};
	assert_eq!(second.revision, 2);
	assert!(
		memory::operate(&store, &Actor::Operator, input)
			.await
			.is_err(),
		"superseded retries cannot restore old policy"
	);
}

#[rstest]
#[tokio::test]
async fn crash_recovery_preserves_japanese_search_and_purge_erases_shared_quotes(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_server::database::native;
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let participant = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	let bank = participant.bank;
	let id = Uuid::now_v7();
	let original = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("東京の地下鉄は便利です。 Tokyo subway is useful."),
			},
		),
	)
	.await
	.unwrap()
	.remove(0);
	let mut shared = bank.clone();
	shared.participant = None;
	let published_id = Uuid::now_v7();
	let operation = Uuid::now_v7();
	memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: operation,
			provider: reference("p"),
			bank: shared.clone(),
			action: memory::Action::Publish {
				source: original.evidence(),
				mutation: Mutation {
					operation_id: operation,
					provider: reference("p"),
					bank: shared.clone(),
					changes: vec![Change::Add {
						id: published_id,
						content: original.content.clone(),
					}],
				},
			},
		},
	)
	.await
	.unwrap();
	let settings: String = native::query_scalar(
		&Query::select()
			.expr(reinhardt::query::SimpleExpr::FunctionCall(
				Alias::new("current_setting").into_iden(),
				vec![Expr::value("pgroonga.enable_crash_safe").into()],
			))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(settings, "on");
	let store = database.kill_and_restart(&store).await;
	let hits: Vec<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_units"))
			.and_where(reinhardt::query::SimpleExpr::CustomWithExpr(
				"? &@~ ?".into(),
				vec![Expr::col("text").into(), Expr::value("地下鉄").into()],
			))
			.and_where(Expr::col("deleted").eq(false))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&store.pool)
	.await
	.unwrap();
	assert!(hits.contains(&id) && hits.contains(&published_id));
	let deletion = mutation(
		&bank,
		Change::Delete {
			id,
			expected_revision: 1,
		},
	);
	memory::mutate(&store, &Actor::Operator, deletion.clone())
		.await
		.unwrap();
	let shared_units = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			provider: reference("p"),
			bank: shared,
		},
	)
	.await
	.unwrap();
	assert!(
		shared_units.is_empty(),
		"selected copies are excluded before physical purge"
	);
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_purge_jobs"))
			.value(Alias::new("purge_after"), chrono::Utc::now())
			.and_where(Expr::col("unit_id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let copied_body: String = native::query_scalar(
		&Query::select()
			.column(Alias::new("text"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("id").eq(Expr::value(published_id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert!(copied_body.is_empty());
	let remaining: i64 = native::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(Expr::col("unit_id").into()))
			.from(Alias::new("memory_history"))
			.and_where(Expr::col("unit_id").is_in([Expr::value(id), Expr::value(published_id)]))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(remaining, 0);
	let store = database.kill_and_restart(&store).await;
	let removed = memory::mutate(&store, &Actor::Operator, deletion)
		.await
		.unwrap();
	assert!(removed[0].deleted);
	assert!(
		memory::mutate(
			&store,
			&Actor::Operator,
			mutation(
				&bank,
				Change::Add {
					id,
					content: content("reintroduced")
				}
			)
		)
		.await
		.is_err()
	);
}

#[rstest]
#[tokio::test]
async fn extracted_causal_batch_is_atomic_and_replays_stable_admitted_identities(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use axum::{Json, Router, routing::post};
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};
	let calls = Arc::new(AtomicUsize::new(0));
	let served = calls.clone();
	let app = Router::new().route("/v1/chat/completions", post(move |Json(input): Json<serde_json::Value>| {
		let served = served.clone();
		async move {
			served.fetch_add(1, Ordering::SeqCst);
			let context: serde_json::Value = serde_json::from_str(input["messages"][1]["content"].as_str().unwrap()).unwrap();
			assert_eq!(context["mode"], "retain");
			let evidence: Vec<Evidence> = serde_json::from_value(context["evidence"].clone()).unwrap();
			let mut rain = content("大雨が降った。 Heavy rain fell.");
			rain.evidence = evidence.clone();
			let mut delay = content("雨で列車が遅れた。 Rain delayed the train.");
			delay.evidence = evidence;
			let output = extraction::Extraction { facts: vec![rain, delay], causal: vec![extraction::CausalRelation { cause: 0, effect: 1, weight: 0.9 }] };
			Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":serde_json::to_string(&output).unwrap()}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
		}
	}));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
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
	let seed = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("An admitted report / 採用済みの報告"),
			},
		),
	)
	.await
	.unwrap()
	.remove(0);
	let operation = memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		action: memory::Action::Retain {
			text: "Heavy rain delayed the train. 大雨で列車が遅れた。".into(),
			evidence: vec![seed.evidence()],
		},
	};
	let memory::Outcome::Units(first) =
		memory::operate(&store, &Actor::Operator, operation.clone())
			.await
			.unwrap()
	else {
		panic!("retention must return admitted units")
	};
	let memory::Outcome::Units(replay) = memory::operate(&store, &Actor::Operator, operation)
		.await
		.unwrap()
	else {
		panic!("replay must return the same units")
	};
	assert_eq!(first, replay);
	assert_eq!(first.len(), 2);
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	assert_eq!(
		first[0].content.links,
		vec![Link {
			target: first[1].id,
			revision: 1,
			kind: LinkKind::Causes,
			weight: 0.9
		}]
	);
	assert_eq!(
		first[1].content.links,
		vec![Link {
			target: first[0].id,
			revision: 1,
			kind: LinkKind::CausedBy,
			weight: 0.9
		}]
	);
	assert!(
		first
			.iter()
			.all(|unit| unit.content.verification == Verification::Unverified)
	);
	let a = Uuid::now_v7();
	let b = Uuid::now_v7();
	let mut linked = content("This batch must roll back.");
	linked.links.push(Link {
		target: b,
		revision: 1,
		kind: LinkKind::Causes,
		weight: 1.,
	});
	let failed = Mutation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		changes: vec![
			Change::Add {
				id: a,
				content: linked,
			},
			Change::Add {
				id: b,
				content: content("Never admitted"),
			},
			Change::Correct {
				id: seed.id,
				expected_revision: seed.revision + 1,
				content: content("Stale caller"),
			},
		],
	};
	assert!(
		memory::mutate(&store, &Actor::Operator, failed)
			.await
			.is_err()
	);
	let current = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			bank,
			provider: reference("p"),
		},
	)
	.await
	.unwrap();
	assert_eq!(current.len(), 3);
	assert!(current.iter().all(|unit| unit.id != a && unit.id != b));
	assert_eq!(
		current
			.iter()
			.find(|unit| unit.id == seed.id)
			.unwrap()
			.revision,
		seed.revision
	);
	server.abort();
}

#[rstest]
#[tokio::test]
async fn unit_cas_receipts_participant_separation_and_derived_fences(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let participant = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	let clone = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	assert_ne!(participant.id, clone.id);
	let bank = participant.bank;
	let id = Uuid::now_v7();
	let add = mutation(
		&bank,
		Change::Add {
			id,
			content: content("東京では地下鉄で移動する。"),
		},
	);
	let (first, replay) = tokio::join!(
		memory::mutate(&store, &Actor::Operator, add.clone()),
		memory::mutate(&store, &Actor::Operator, add.clone())
	);
	assert_eq!(first.unwrap(), replay.unwrap());
	let mut changed = add.clone();
	changed.changes = vec![Change::Add {
		id,
		content: content("Different request"),
	}];
	assert!(
		memory::mutate(&store, &Actor::Operator, changed)
			.await
			.is_err()
	);
	assert!(
		memory::list(
			&store,
			&Actor::Operator,
			memory::ReadBank {
				provider: reference("p"),
				bank: clone.bank
			}
		)
		.await
		.unwrap()
		.is_empty()
	);
	let mut derived = content("Use the Tokyo subway.");
	derived.kind = Kind::World;
	derived.evidence = vec![Evidence::Unit {
		bank: bank.clone(),
		id,
		revision: 1,
	}];
	let observation = Uuid::now_v7();
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: observation,
				content: derived,
			},
		),
	)
	.await
	.unwrap();
	let a = mutation(
		&bank,
		Change::Correct {
			id,
			expected_revision: 1,
			content: content("東京では徒歩で移動する。"),
		},
	);
	let b = mutation(
		&bank,
		Change::Correct {
			id,
			expected_revision: 1,
			content: content("東京ではバスで移動する。"),
		},
	);
	let (a, b) = tokio::join!(
		memory::mutate(&store, &Actor::Operator, a),
		memory::mutate(&store, &Actor::Operator, b)
	);
	assert_ne!(
		a.is_ok(),
		b.is_ok(),
		"only one caller-observed revision may win"
	);
	let visible = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			provider: reference("p"),
			bank: bank.clone(),
		},
	)
	.await
	.unwrap();
	assert_eq!(visible.len(), 1);
	assert_eq!(visible[0].revision, 2);
	assert_eq!(visible[0].id, id);
	let entries = aidash_server::semantic::service::entries(&store, &Actor::Operator, workspace)
		.await
		.unwrap();
	let source = entries.iter().find(|entry| entry.id == id).unwrap();
	assert_eq!(
		source.source,
		serde_json::to_value(aidash_domain::semantic::Source::Unit { id }).unwrap()
	);
	assert_eq!(source.revision, 2);
	assert_eq!(source.metadata["unit_revision"], 2);
	assert!(
		!entries.iter().any(|entry| entry.id == observation),
		"stale bodies must be withheld from delivery"
	);
	let mut projection_tx = aidash_server::database::native::begin(&store.pool)
		.await
		.unwrap();
	let stale: String = aidash_server::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("semantic_entries"))
			.and_where(Expr::col("id").eq(Expr::value(observation)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut *projection_tx)
	.await
	.unwrap();
	assert_eq!(stale, "REVOKED");
	projection_tx.rollback().await.unwrap();
	assert!(
		memory::mutate(&store, &Actor::Operator, add).await.is_err(),
		"an old receipt cannot deliver an obsolete body"
	);
	let mut foreign = bank.clone();
	foreign.home = "aidash://executor".into();
	assert!(
		memory::list(
			&store,
			&Actor::Operator,
			memory::ReadBank {
				provider: reference("p"),
				bank: foreign
			}
		)
		.await
		.is_err()
	);
	let removed = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Delete {
				id,
				expected_revision: 2,
			},
		),
	)
	.await
	.unwrap();
	assert!(removed[0].deleted);
	assert!(removed[0].content.text.is_empty());
	let mut tx = aidash_server::database::native::begin(&store.pool)
		.await
		.unwrap();
	let rows = aidash_server::database::native::query(
		&Query::select()
			.columns(["deleted", "state"].map(Alias::new))
			.from(Alias::new("semantic_entries"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *tx)
	.await
	.unwrap();
	assert!(rows.try_get::<bool>("deleted").unwrap());
	assert_eq!(rows.try_get::<String>("state").unwrap(), "DELETED");
	tx.rollback().await.unwrap();
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
async fn pgvector_filters_scope_and_preserves_immutable_point_generations(
	#[future] database: DatabaseFixture,
) {
	use aidash_domain::semantic::{VectorConfig, VectorFilter};
	let database = database.await;
	let store = Store::from_pool(
		database.connection.into_postgres().unwrap(),
		"aidash://vectors".into(),
	)
	.await
	.unwrap();
	let index = aidash_server::bootstrap::semantic_transport(&store);
	let config = VectorConfig {
		provider: "postgres".into(),
		endpoint: "local".into(),
		credential_env: None,
	};
	index
		.ensure_collection(&config, "fixture", 3)
		.await
		.unwrap();
	assert!(
		index
			.ensure_collection(&config, "fixture", 4)
			.await
			.is_err()
	);
	let workspace = Uuid::now_v7();
	let other = Uuid::now_v7();
	let ids: Vec<_> = (0..3).map(|_| Uuid::now_v7()).collect();
	for (id, ws, vector) in [
		(ids[0], workspace, vec![1., 0., 0.]),
		(ids[1], workspace, vec![0., 1., 0.]),
		(ids[2], other, vec![1., 0., 0.]),
	] {
		index
			.upsert(
				&config,
				"fixture",
				id,
				&vector,
				json!({"workspace_id":ws,"tenant":"acme","entry_id":id,"revision":1,"index_revision":1}),
			)
			.await
			.unwrap();
	}
	let filter = VectorFilter {
		workspace,
		tenant: "acme",
		allowed: &ids,
	};
	let results = index
		.query(&config, "fixture", &[0.9, 0.1, 0.], filter, 2048)
		.await
		.unwrap();
	assert_eq!(results.iter().map(|p| p.id).collect::<Vec<_>>(), ids[..2]);
	let denied = VectorFilter {
		workspace,
		tenant: "other",
		allowed: &ids,
	};
	assert!(
		index
			.query(&config, "fixture", &[1., 0., 0.], denied, 3)
			.await
			.unwrap()
			.is_empty()
	);
	assert!(
		index
			.upsert(
				&config,
				"fixture",
				ids[0],
				&[1., 0., 0.],
				json!({"workspace_id":other,"tenant":"acme"})
			)
			.await
			.is_err()
	);
	assert!(
		index
			.upsert(
				&config,
				"fixture",
				Uuid::now_v7(),
				&[0., 0., 0.],
				json!({"workspace_id":workspace,"tenant":"acme"})
			)
			.await
			.is_err()
	);
	index
		.delete_point(&config, "fixture", ids[0])
		.await
		.unwrap();
	assert!(!index.present(&config, "fixture", &ids[..2]).await.unwrap());
	index.delete_collection(&config, "fixture").await.unwrap();
}

#[rstest]
#[tokio::test]
async fn four_arm_recall_is_native_bounded_and_model_calls_are_memoized(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use axum::{Json, Router, routing::post};
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};
	let calls = Arc::new(AtomicUsize::new(0));
	let served = calls.clone();
	let app = Router::new().route("/v1/embeddings",post(move |Json(input):Json<serde_json::Value>| {
        let served = served.clone();
        async move { served.fetch_add(1,Ordering::SeqCst); Json(json!({"model":input["model"],"data":[{"index":0,"embedding":[1.0,0.1,0.0]}],"usage":{"prompt_tokens":1,"total_tokens":1}})) }
    }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	});
	let database = database.await;
	let (store, registry, workspace) = setup_endpoint(&database, bounds, &endpoint).await;
	let participant = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	let bank = participant.bank;
	let id = Uuid::now_v7();
	let added = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("東京の地下鉄は便利です。 Tokyo subway is useful."),
			},
		),
	)
	.await
	.unwrap();
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		loop {
			aidash_server::semantic::worker::sweep(&store)
				.await
				.unwrap();
			let state: String = aidash_server::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("state"))
					.from(Alias::new("semantic_entries"))
					.and_where(Expr::col("id").eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&store.pool)
			.await
			.unwrap();
			assert_ne!(state, "ERROR", "native vector indexing failed");
			assert_ne!(state, "REVOKED", "native vector indexing authority failed");
			if state == "READY" {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
	})
	.await
	.expect("the native vector projection must become ready");
	let operation = memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		action: memory::Action::Recall {
			query: RecallQuery {
				text: "東京 地下鉄 subway".into(),
				time: None,
				kinds: vec![],
				max_tokens: 8192,
			},
		},
	};
	let memory::Outcome::Recall(Recall::Ready { units }) =
		memory::operate(&store, &Actor::Operator, operation.clone())
			.await
			.unwrap()
	else {
		panic!("native recall must return the admitted bilingual unit");
	};
	assert_eq!(units, added);
	assert_eq!(
		calls.load(Ordering::SeqCst),
		2,
		"one indexing call and one Recall embedding"
	);
	let _ = memory::operate(&store, &Actor::Operator, operation)
		.await
		.unwrap();
	assert_eq!(
		calls.load(Ordering::SeqCst),
		2,
		"an exact retry reuses its durable model attempt"
	);
	let no_space = memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		action: memory::Action::Recall {
			query: RecallQuery {
				text: "東京".into(),
				time: None,
				kinds: vec![],
				max_tokens: 1,
			},
		},
	};
	assert!(matches!(
		memory::operate(&store, &Actor::Operator, no_space)
			.await
			.unwrap(),
		memory::Outcome::Recall(Recall::NoSpace)
	));
	// The immutable Registry relationship protects roles from deletion.
	let delete = Query::delete()
		.from_table(Alias::new("registry"))
		.and_where(Expr::col("id").eq("e"))
		.to_string(PostgresQueryBuilder);
	assert!(
		aidash_server::database::native::query(&delete)
			.execute(&store.pool)
			.await
			.is_err()
	);
	let _ = registry;
	let shared = Bank {
		participant: None,
		..bank.clone()
	};
	let publish = mutation(
		&shared,
		Change::Add {
			id: Uuid::now_v7(),
			content: added[0].content.clone(),
		},
	);
	let published = memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: publish.operation_id,
			provider: reference("p"),
			bank: shared.clone(),
			action: memory::Action::Publish {
				source: added[0].evidence(),
				mutation: publish,
			},
		},
	)
	.await
	.unwrap();
	assert!(matches!(published,memory::Outcome::Units(ref units) if units.len()==1));
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Correct {
				id,
				expected_revision: 1,
				content: content("Tokyo subway correction"),
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
				bank: shared
			}
		)
		.await
		.unwrap()
		.is_empty(),
		"publication depends on the exact original source"
	);
	// No arbitrary JSON memory table survives fresh schema creation.
	let tables = Query::select()
		.column(Alias::new("table_name"))
		.from((Alias::new("information_schema"), Alias::new("tables")))
		.and_where(Expr::col("table_name").is_in(["memory", "semantic_agent_memory"]))
		.to_string(PostgresQueryBuilder);
	assert!(
		aidash_server::database::native::query(&tables)
			.fetch_all(&store.pool)
			.await
			.unwrap()
			.is_empty()
	);
	server.abort();
}

#[rstest]
#[tokio::test]
async fn home_run_reads_survive_reindex_but_the_writer_is_invalidated_by_its_own_correction(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_domain::{NewTask, qualified_agent};
	use aidash_server::{authorization::Authorization, config::Config, federation::Federation};
	use axum::{Json, Router, routing::post};
	use std::sync::Arc;
	let id = Uuid::now_v7();
	let captured = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
	let received = captured.clone();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let provider = tokio::spawn(async move {
		let model = post(move |Json(input): Json<serde_json::Value>| {
			let received = received.clone();
			async move {
				let context: serde_json::Value =
					serde_json::from_str(input["messages"][1]["content"].as_str().unwrap())
						.unwrap();
				let mut calls = received.lock().unwrap();
				calls.push(context);
				let (name, arguments) = if calls.len() == 1 {
					(
						"memory_recall",
						json!({"text":"東京 subway","time":null,"kinds":[],"max_tokens":8192}),
					)
				} else {
					(
						"memory_mutate",
						json!({"changes":[Change::Correct { id, expected_revision: 1, content: content("歩いて移動する。 Walk instead.") }]}),
					)
				};
				Json(
					json!({"choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":format!("native-{}",calls.len()),"type":"function","function":{"name":name,"arguments":serde_json::to_string(&arguments).unwrap()}}]}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}),
				)
			}
		});
		axum::serve(listener, Router::new().route("/v1/chat/completions",model).route("/v1/embeddings", post(|Json(input): Json<serde_json::Value>| async move {
            Json(json!({"model":input["model"],"data":[{"index":0,"embedding":[1.,0.1,0.]}],"usage":{"prompt_tokens":1}}))
        }))).await.unwrap();
	});
	let database = database.await;
	let (store, registry, workspace) = setup_endpoint(&database, bounds, &endpoint).await;
	let authorization = Authorization {
		pool: store.pool.clone(),
	};
	let owner = qualified_agent(&store.node_id, "a", "1.0.0");
	let policy = json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},owner:{"kind":"agent"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]});
	authorization
		.replace(
			"acme",
			1,
			serde_json::from_value(policy).unwrap(),
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
	let issued = authorization
		.issue_credential("acme", "alice", 3600, "operator")
		.await
		.unwrap();
	let actor = authorization.authenticate(&issued.token).await.unwrap();
	let Actor::Subject(identity) = &actor else {
		panic!("subject fixture");
	};
	let participant = memory::create_participant(
		&store,
		&actor,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	memory::mutate(
		&store,
		&actor,
		mutation(
			&participant.bank,
			Change::Add {
				id,
				content: content("東京の地下鉄を利用する。 Use the subway in Tokyo."),
			},
		),
	)
	.await
	.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let task = store
		.create_task(
			workspace,
			&NewTask {
				title: "Tokyo transport".into(),
				description: "移動方法を確認する".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"alice",
			None,
		)
		.await
		.unwrap();
	memory::assign_participant(
		&store,
		&actor,
		workspace,
		task.id,
		memory::AssignmentChange {
			task_revision: task.revision,
			expected: None,
			target: Some(memory::AssignedParticipant {
				participant_id: participant.id,
				participant_revision: participant.revision,
			}),
		},
	)
	.await
	.unwrap();
	let f = Federation {
		sandbox: Default::default(),
		registry,
		config: Config {
			node_id: store.node_id.clone(),
			endpoint: "http://127.0.0.1:1".into(),
			database_url: database.url.clone(),
			nats_url: "nats://127.0.0.1:1".into(),
			api_token: "fixture-operator".into(),
			web_dir: "web/dist".into(),
			lease_seconds: 30,
			oidc: None,
		},
		store,
		client: reqwest::Client::new(),
		notify: Arc::new(tokio::sync::Notify::new()),
	};
	aidash_server::authorization::execution::claim(
		&f,
		identity,
		task.id,
		task.revision,
		&reference("a"),
	)
	.await
	.unwrap();
	let run = f.store.runs().await.unwrap().remove(0);
	let worker = aidash_server::harness::Harness {
		federation: f.clone(),
	};
	for _ in 0..4 {
		tokio::time::timeout(std::time::Duration::from_secs(15), worker.worker_once())
			.await
			.expect("native inference and explicit recall must not deadlock")
			.unwrap();
		if !captured.lock().unwrap().is_empty()
			&& f.store.run(run.id).await.unwrap().phase().as_str() == "READY"
		{
			break;
		}
	}
	assert_eq!(captured.lock().unwrap().len(), 1);
	let first = captured.lock().unwrap()[0]["current"]["semantic_memory"].clone();
	assert_eq!(
		first["memory"]["binding"]["bank"]["participant"],
		json!(participant.id)
	);
	assert_eq!(
		first["memory"]["banks"][0]["recall"]["units"][0]["id"],
		json!(id)
	);
	let latest = "最新の入力: 地下鉄の料金も確認してください。";
	f.store
		.accept_run_message(run.id, "alice", latest, "latest", 4096)
		.await
		.unwrap();
	let message = f
		.store
		.run_inputs(run.id)
		.await
		.unwrap()
		.into_iter()
		.find(|input| input.idempotency_key == "latest")
		.unwrap()
		.message_id
		.unwrap();
	let index = aidash_server::semantic::service::get_index(&f.store, &actor, workspace)
		.await
		.unwrap();
	let configuration = serde_json::from_value(index.spec.clone()).unwrap();
	aidash_server::semantic::service::configure(
		&f.store,
		workspace,
		aidash_server::semantic::ConfigureIndex {
			expected_revision: index.revision,
			spec: configuration,
		},
	)
	.await
	.unwrap();
	let reindex = memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: participant.bank.clone(),
		action: memory::Action::Reindex {
			expected_index_revision: index.revision + 1,
		},
	};
	memory::operate(&f.store, &actor, reindex.clone())
		.await
		.unwrap();
	memory::operate(&f.store, &actor, reindex).await.unwrap();
	aidash_server::semantic::worker::sweep(&f.store)
		.await
		.unwrap();
	aidash_server::authorization::execution::details(&f, identity, run.id)
		.await
		.expect("index-only generation cannot invalidate canonical Run reads");
	for _ in 0..4 {
		tokio::time::timeout(std::time::Duration::from_secs(15), worker.worker_once())
			.await
			.expect("native correction must release its own read locks")
			.unwrap();
		if memory::list(
			&f.store,
			&actor,
			memory::ReadBank {
				bank: participant.bank.clone(),
				provider: reference("p"),
			},
		)
		.await
		.unwrap()[0]
			.revision == 2
		{
			break;
		}
	}
	let frames = captured.lock().unwrap().clone();
	assert_eq!(
		frames.len(),
		2,
		"Run error={:?}; last tool={:?}",
		f.store.run(run.id).await.unwrap().error,
		frames.last().map(|f| &f["history"])
	);
	let second = captured.lock().unwrap()[1]["current"]["semantic_memory"].clone();
	assert_eq!(
		second["memory"]["boundary"]["inputs"][0]["id"],
		json!(message)
	);
	assert!(
		aidash_server::authorization::execution::details(&f, identity, run.id)
			.await
			.is_err(),
		"current Run disclosure cannot expose consumed obsolete source context"
	);
	let current = memory::list(
		&f.store,
		&actor,
		memory::ReadBank {
			bank: participant.bank,
			provider: reference("p"),
		},
	)
	.await
	.unwrap();
	assert_eq!(
		current[0].revision, 2,
		"the unit mutation committed before invalidating its caller"
	);
	provider.abort();
}

#[rstest]
#[case("pending")]
#[case("running")]
#[case("blocked")]
#[case("failed")]
#[tokio::test]
async fn durable_derived_jobs_refresh_questions_and_never_revive_deleted_sources(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] queued_state: &str,
) {
	use axum::{Json, Router, routing::post};
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};
	let calls = Arc::new(AtomicUsize::new(0));
	let served = calls.clone();
	let app=Router::new().route("/v1/chat/completions",post(move |Json(input):Json<serde_json::Value>| {let calls=served.clone();async move {
  calls.fetch_add(1,Ordering::SeqCst);
  let context:serde_json::Value=serde_json::from_str(input["messages"][1]["content"].as_str().unwrap()).unwrap();
  let units:Vec<Unit>=serde_json::from_value(context["units"].clone()).unwrap();
  let mut result=content(&format!("根拠: {}",units[0].content.text));
  result.kind=serde_json::from_value(context["kind"].clone()).unwrap();
  result.mental_model=serde_json::from_value(context["mental_model"].clone()).unwrap();
  result.evidence=units.iter().map(Unit::evidence).collect();
  Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":serde_json::to_string(&result).unwrap()}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
 }})).route("/v1/embeddings",post(|Json(input):Json<serde_json::Value>|async move {Json(json!({"model":input["model"],"data":[{"index":0,"embedding":[1.,0.1,0.]}],"usage":{"prompt_tokens":1}}))}));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	});
	let database = database.await;
	let (store, _, workspace) =
		setup_endpoint_flags(&database, bounds, &endpoint, (false, true, true)).await;
	let participant = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	let bank = participant.bank;
	let id = Uuid::now_v7();
	let first = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("東京では地下鉄を利用する。 Use the subway in Tokyo."),
			},
		),
	)
	.await
	.unwrap()
	.remove(0);
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let all = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			provider: reference("p"),
			bank: bank.clone(),
		},
	)
	.await
	.unwrap();
	assert_eq!(all.len(), 2);
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	let operation = Uuid::now_v7();
	let target = Uuid::now_v7();
	let mut draft = first.content.clone();
	draft.kind = Kind::MentalModel;
	draft.mental_model = Some(MentalModel {
		question: "東京での移動方針は？ How should I travel in Tokyo?".into(),
		automatic_refresh: true,
	});
	draft.evidence = vec![first.evidence()];
	let memory::Outcome::Units(model) = memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: operation,
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::Derive {
				kind: Kind::MentalModel,
				sources: vec![first.evidence()],
				mutation: Mutation {
					operation_id: operation,
					provider: reference("p"),
					bank: bank.clone(),
					changes: vec![Change::Add {
						id: target,
						content: draft,
					}],
				},
			},
		},
	)
	.await
	.unwrap() else {
		panic!()
	};
	assert_eq!(model[0].revision, 1);
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Correct {
				id,
				expected_revision: 1,
				content: content("東京では歩く。 Walk in Tokyo."),
			},
		),
	)
	.await
	.unwrap();
	use aidash_server::database::native;
	let job: Uuid = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_engine_jobs"))
			.and_where(Expr::col("kind").eq("mental_model"))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_engine_jobs"))
			.value(Alias::new("state"), queued_state)
			.value(Alias::new("attempts"), 1_i32)
			.value(Alias::new("claim"), Some(Uuid::now_v7()))
			.and_where(Expr::col("id").eq(Expr::value(job)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Correct {
				id,
				expected_revision: 2,
				content: content("東京では自転車を使う。 Cycle in Tokyo."),
			},
		),
	)
	.await
	.unwrap();
	let queued = native::query(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("memory_engine_jobs"))
			.and_where(Expr::col("id").eq(Expr::value(job)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(queued.try_get::<String>("state").unwrap(), "pending");
	assert_eq!(queued.try_get::<i32>("attempts").unwrap(), 0);
	assert_eq!(queued.try_get::<Option<Uuid>>("claim").unwrap(), None);
	let input: serde_json::Value = queued.try_get("input").unwrap();
	assert_eq!(
		input["revision"], 2,
		"the stale question still has its original queued revision"
	);
	assert_eq!(input["sources"][0]["revision"], 3);
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let all = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			provider: reference("p"),
			bank: bank.clone(),
		},
	)
	.await
	.unwrap();
	let model = all.iter().find(|u| u.id == target).unwrap();
	assert_eq!(
		model.revision, 3,
		"source invalidation and refreshed answer each have a canonical revision"
	);
	assert!(model.content.text.contains("自転車"));
	assert_eq!(model.content.verification, Verification::Unverified);
	let refreshed_calls = calls.load(Ordering::SeqCst);
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	assert_eq!(
		calls.load(Ordering::SeqCst),
		refreshed_calls,
		"completed jobs cannot repeat model calls"
	);
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Delete {
				id,
				expected_revision: 3,
			},
		),
	)
	.await
	.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	assert_eq!(calls.load(Ordering::SeqCst), refreshed_calls);
	let all = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			provider: reference("p"),
			bank: bank.clone(),
		},
	)
	.await
	.unwrap();
	assert!(
		all.is_empty(),
		"deleted evidence cannot be reconstructed by automatic refresh"
	);
	let memory::Outcome::Jobs(jobs) = memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank,
			action: memory::Action::Jobs { after: None },
		},
	)
	.await
	.unwrap() else {
		panic!()
	};
	assert_eq!(
		jobs.items.len(),
		6,
		"each source correction records its bounded observation and repair triggers"
	);
	assert_eq!(
		jobs.items.iter().filter(|j| j.state == "complete").count(),
		4
	);
	let obsolete: Vec<_> = jobs.items.iter().filter(|j| j.state == "blocked").collect();
	assert_eq!(
		obsolete.len(),
		2,
		"superseded observation inputs are safely blocked"
	);
	assert!(obsolete.iter().all(|j| j.kind == "observation"
		&& j.last_error.as_deref() == Some("authority_or_source_changed")));
	assert_eq!(
		jobs.items.iter().find(|j| j.id == job).unwrap().state,
		"complete"
	);
	server.abort();
}

#[rstest]
#[tokio::test]
async fn learning_rejects_uncertain_effects_and_reads_complete_canonical_results(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
) {
	use aidash_server::database::native;
	use axum::{Json, Router, routing::post};
	use std::sync::{Arc, Mutex};
	let captured = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
	let capture = captured.clone();
	let app = Router::new().route("/v1/chat/completions", post(move |Json(input): Json<serde_json::Value>| {
        let capture = capture.clone();
        async move {
            let input: serde_json::Value = serde_json::from_str(input["messages"][1]["content"].as_str().unwrap()).unwrap();
            let mut result = content("Learned from a complete journal. 完全な記録からの候補。");
            result.kind = Kind::Experience;
            result.evidence = serde_json::from_value(input["evidence"].clone()).unwrap();
            let mut pending = result.clone();
            pending.learning = Learning::Procedure;
            pending.text = "Pending procedure from the same journal / 同じ記録に基づく未承認の手順".into();
            capture.lock().unwrap().push(input);
            Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":serde_json::to_string(&json!({"facts":[result,pending],"causal":[]})).unwrap()}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
        }
    }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	});
	let database = database.await;
	// This fixture includes complete journal bodies and the structured extraction
	// schema. Admission still fails explicitly if their whole request exceeds
	// the pinned allowance; do not rely on a truncated journal to make it fit.
	bounds.max_model_tokens = 32_768;
	let (store, _, workspace) =
		setup_endpoint_flags(&database, bounds, &endpoint, (true, false, false)).await;

	let authorization = aidash_server::authorization::Authorization {
		pool: store.pool.clone(),
	};
	let owner = aidash_domain::qualified_agent(&store.node_id, "a", "1.0.0");
	authorization.replace("acme",1,serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},owner:{"kind":"agent"}},"policies":[{"id":"learning","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]})).unwrap(),"operator").await.unwrap();
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
	let Actor::Subject(_identity) = authorization.authenticate(&credential.token).await.unwrap()
	else {
		panic!("learning authority");
	};
	let participant = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	let source_id = Uuid::now_v7();
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&participant.bank,
			Change::Add {
				id: source_id,
				content: content("東京の根拠。 Original admitted source."),
			},
		),
	)
	.await
	.unwrap();
	let task = store
		.create_task(
			workspace,
			&aidash_domain::NewTask {
				title: "Canonical learning".into(),
				description: "Read complete results".into(),
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
	native::query(
		&Query::insert()
			.into_table(Alias::new("runs"))
			.columns(
				[
					"id",
					"task_id",
					"workspace_id",
					"home_node",
					"agent_id",
					"agent_version",
					"phase",
					"pending",
					"revision",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(id))
					.expr(Expr::value(task.id))
					.expr(Expr::value(workspace))
					.expr(Expr::value(&store.node_id))
					.expr(Expr::value("a"))
					.expr(Expr::value("1.0.0"))
					.expr(Expr::value("COMPLETED"))
					.expr(Expr::value(pending))
					.expr(Expr::value(1_i64))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_run_reads"))
			.columns(["run_id", "unit_id", "revision"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(id))
					.expr(Expr::value(source_id))
					.expr(Expr::value(1_i64))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	native::query(
		&Query::insert()
			.into_table(Alias::new("run_task_snapshots"))
			.columns(
				[
					"id",
					"run_id",
					"task_revision",
					"step",
					"input_seq",
					"body",
					"captured_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(Uuid::now_v7()))
					.expr(Expr::value(id))
					.expr(Expr::value(task.revision))
					.expr(Expr::value(0_i32))
					.expr(Expr::value(0_i64))
					.expr(Expr::value(serde_json::to_value(&task).unwrap()))
					.expr(Expr::value(chrono::Utc::now()))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("tasks"))
			.value(
				Alias::new("description"),
				"Edited after execution; never substitute this text.",
			)
			.and_where(Expr::col("id").eq(Expr::value(task.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();

	let full_result = format!("{}Exact tail: 結果の末尾。", "record ".repeat(180));
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_run_bindings"))
			.columns(
				[
					"run_id",
					"participant_id",
					"participant_revision",
					"provider_id",
					"provider_version",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(id))
					.expr(Expr::value(participant.id))
					.expr(Expr::value(participant.revision))
					.expr(Expr::value("p"))
					.expr(Expr::value("1.0.0"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	native::query(
		&Query::insert()
			.into_table(Alias::new("invocations"))
			.columns(
				[
					"idempotency_key",
					"run_id",
					"tool",
					"input",
					"status",
					"result",
					"replay_safe",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value("canonical"))
					.expr(Expr::value(id))
					.expr(Expr::value("echo"))
					.expr(Expr::value(json!({"input":"complete"})))
					.expr(Expr::value("UNCERTAIN"))
					.expr(Expr::value(json!({"body":full_result})))
					.expr(Expr::value(false))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let mut db = database.lease.handle();
	aidash_server::apps::identity::models::AuthorizationExecution::objects()
		.create_with_conn(
			&mut db,
			&aidash_server::apps::identity::models::AuthorizationExecution::build()
				.run_id(id)
				.task_id(task.id)
				.workspace_id(workspace)
				.tenant("acme")
				.credential_id(credential.credential.id)
				.root_subject("alice")
				.subject_chain(vec![
					"alice".into(),
					aidash_domain::qualified_agent(&store.node_id, "a", "1.0.0"),
				])
				.finish(),
		)
		.await
		.unwrap();

	let run = store.run(id).await.unwrap();
	let proof = Evidence::Run {
		id,
		revision: run.revision,
		digest: aidash_domain::semantic::indexing::content_digest(
			&serde_json::to_string(&run).unwrap(),
		),
	};
	let operation = memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: participant.bank.clone(),
		action: memory::Action::Learn { run: proof.clone() },
	};
	assert!(
		memory::operate(&store, &Actor::Operator, operation.clone())
			.await
			.is_err()
	);
	assert!(
		captured.lock().unwrap().is_empty(),
		"uncertain effects must be rejected before extraction"
	);
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("invocations"))
			.value(Alias::new("status"), "COMPLETED")
			.and_where(Expr::col("run_id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let memory::Outcome::Candidates(candidates) =
		memory::operate(&store, &Actor::Operator, operation)
			.await
			.unwrap()
	else {
		panic!("learning must return review candidates")
	};
	assert_eq!(candidates.len(), 2);
	assert_eq!(candidates[0].content.verification, Verification::Unverified);
	let extracted: serde_json::Value =
		serde_json::from_str(captured.lock().unwrap()[0]["text"].as_str().unwrap()).unwrap();
	assert_eq!(
		extracted["journal"]["invocations"][0]["result"]["body"],
		full_result
	);
	assert_eq!(
		extracted["journal"]["observed_tasks"][0]["body"]["description"],
		"Read complete results"
	);
	assert!(
		!captured.lock().unwrap()[0]["text"]
			.as_str()
			.unwrap()
			.contains("Edited after execution")
	);

	assert!(
		full_result.len() > 1024,
		"fixture must exceed the UI preview limit"
	);
	assert!(
		memory::list(
			&store,
			&Actor::Operator,
			memory::ReadBank {
				provider: reference("p"),
				bank: participant.bank.clone()
			}
		)
		.await
		.unwrap()
		.len() == 1,
		"only the original source is admitted; Run extraction remains a candidate"
	);
	let reviewed_id = Uuid::now_v7();
	let actor = authorization.authenticate(&credential.token).await.unwrap();
	let mut review = memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: participant.bank.clone(),
		action: memory::Action::Review {
			id: candidates[0].id,
			expected_revision: candidates[0].revision,
			mutation: Some(mutation(
				&participant.bank,
				Change::Add {
					id: reviewed_id,
					content: candidates[0].content.clone(),
				},
			)),
		},
	};
	if let memory::Action::Review {
		mutation: Some(mutation),
		..
	} = &review.action
	{
		review.operation_id = mutation.operation_id;
	}
	let memory::Outcome::Reviewed(Some(admitted)) = memory::operate(&store, &actor, review.clone())
		.await
		.unwrap()
	else {
		panic!("human must admit one reviewed unit");
	};
	assert_eq!(admitted.id, reviewed_id);
	let memory::Outcome::Reviewed(Some(replayed)) =
		memory::operate(&store, &actor, review).await.unwrap()
	else {
		panic!("review replay must preserve the admitted result");
	};
	assert_eq!(replayed, admitted);
	let origins: Vec<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("run_id"))
			.from(Alias::new("memory_unit_run_origins"))
			.and_where(Expr::col("unit_id").eq(Expr::value(reviewed_id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&store.pool)
	.await
	.unwrap();
	assert_eq!(
		origins,
		vec![id],
		"human admission preserves the Run's origin instead of operator credit"
	);
	memory::mutate(
		&store,
		&actor,
		mutation(
			&participant.bank,
			Change::Correct {
				id: reviewed_id,
				expected_revision: 1,
				content: content("Human correction / 人による修正"),
			},
		),
	)
	.await
	.unwrap();
	let after: Vec<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("run_id"))
			.from(Alias::new("memory_unit_run_origins"))
			.and_where(Expr::col("unit_id").eq(Expr::value(reviewed_id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&store.pool)
	.await
	.unwrap();
	assert_eq!(
		after, origins,
		"later human edits cannot erase origin-owned budget lineage"
	);
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&participant.bank,
			Change::Correct {
				id: source_id,
				expected_revision: 1,
				content: content("Corrected current source. 修正済みの根拠。"),
			},
		),
	)
	.await
	.unwrap();
	assert!(
		memory::operate(
			&store,
			&Actor::Operator,
			memory::Operation {
				operation_id: Uuid::now_v7(),
				provider: reference("p"),
				bank: participant.bank.clone(),
				action: memory::Action::Learn { run: proof }
			}
		)
		.await
		.is_err(),
		"a Run's consumed unit revision remains a learning dependency even for operator extraction"
	);
	assert_eq!(
		captured.lock().unwrap().len(),
		1,
		"withdrawn Run provenance must be rejected before another model call"
	);
	let memory::Outcome::Candidates(hidden) = memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: participant.bank.clone(),
			action: memory::Action::Candidates,
		},
	)
	.await
	.unwrap() else {
		panic!("candidate review")
	};
	assert_eq!(
		hidden.len(),
		1,
		"admitted candidates leave the pending queue"
	);
	assert_eq!(hidden[0].id, candidates[1].id);
	assert_eq!(hidden[0].state, CandidateState::Invalidated);
	assert!(hidden[0].content.text.is_empty());
	let rejection = memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: participant.bank,
		action: memory::Action::Review {
			id: hidden[0].id,
			expected_revision: hidden[0].revision,
			mutation: None,
		},
	};
	for _ in 0..2 {
		assert!(matches!(
			memory::operate(&store, &actor, rejection.clone())
				.await
				.unwrap(),
			memory::Outcome::Reviewed(None)
		));
	}
	let mut reused = rejection;
	if let memory::Action::Review {
		expected_revision, ..
	} = &mut reused.action
	{
		*expected_revision += 1;
	}
	assert!(matches!(
		memory::operate(&store, &actor, reused).await,
		Err(aidash_server::Error::Conflict(_))
	));
	server.abort();
}

#[rstest]
#[case::entity_group(true)]
#[case::semantic_group(false)]
#[tokio::test]
async fn observation_consolidation_keeps_conflicts_and_recomputes_surviving_evidence(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] with_entities: bool,
) {
	use axum::{Json, Router, routing::post};
	use std::sync::{
		Arc, Mutex,
		atomic::{AtomicBool, Ordering},
	};
	let requests = Arc::new(Mutex::new(Vec::<Vec<Unit>>::new()));
	let capture = requests.clone();
	let supported = Arc::new(AtomicBool::new(false));
	let claim = supported.clone();
	let app=Router::new().route("/v1/chat/completions",post(move |Json(input):Json<serde_json::Value>| {
        let capture=capture.clone(); let claim=claim.clone(); async move {
            let input:serde_json::Value=serde_json::from_str(input["messages"][1]["content"].as_str().unwrap()).unwrap();
            if input.get("mandatory").is_some() {
                let mandatory:Vec<Unit>=serde_json::from_value(input["mandatory"].clone()).unwrap();
                let candidates:Vec<Unit>=serde_json::from_value(input["candidates"].clone()).unwrap();
                let selected:Vec<Evidence>=mandatory.iter().chain(&candidates).map(Unit::evidence).collect();
                return Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":serde_json::to_string(&selected).unwrap()}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}));
            }
            let units:Vec<Unit>=serde_json::from_value(input["units"].clone()).unwrap();
            let mut result=content(&units.iter().map(|unit|unit.content.text.clone()).collect::<Vec<_>>().join("\n"));
            result.kind=Kind::Observation; result.evidence=units.iter().map(Unit::evidence).collect();
            if claim.load(Ordering::SeqCst) {result.verification=Verification::Supported;}
            capture.lock().unwrap().push(units);
            Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":serde_json::to_string(&result).unwrap()}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
        }
    })).route("/v1/embeddings",post(|Json(input):Json<serde_json::Value>|async move {Json(json!({"model":input["model"],"data":[{"index":0,"embedding":[1.,0.1,0.]}],"usage":{"prompt_tokens":1}}))}));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	});
	let database = database.await;
	let (store, _, workspace) =
		setup_endpoint_flags(&database, bounds, &endpoint, (false, true, false)).await;
	let participant = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	let bank = participant.bank;
	let ids = [Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()];
	let mut texts = vec![
		"Alice lives in Tokyo. アリスは東京に住んでいる。",
		"Alice does not live in Tokyo. アリスは東京に住んでいない。",
	];
	texts.push(texts[0]);
	let mut observation = None;
	for (index, text) in texts.iter().enumerate() {
		let mut body = content(text);
		body.entities = if with_entities {
			vec![Entity {
				name: "Alice".into(),
				category: "person".into(),
				aliases: vec![],
			}]
		} else {
			vec![]
		};
		memory::mutate(
			&store,
			&Actor::Operator,
			mutation(
				&bank,
				Change::Add {
					id: ids[index],
					content: body,
				},
			),
		)
		.await
		.unwrap();
		aidash_server::semantic::worker::sweep(&store)
			.await
			.unwrap();
		let units = memory::list(
			&store,
			&Actor::Operator,
			memory::ReadBank {
				bank: bank.clone(),
				provider: reference("p"),
			},
		)
		.await
		.unwrap();
		let observations: Vec<_> = units
			.iter()
			.filter(|unit| unit.content.kind == Kind::Observation)
			.collect();
		assert_eq!(
			observations.len(),
			1,
			"one stable observation consolidates the complete entity or semantic group"
		);
		if let Some(id) = observation {
			assert_eq!(observations[0].id, id);
		} else {
			observation = Some(observations[0].id);
		}
		assert_eq!(observations[0].content.evidence.len(), index + 1);
		assert_eq!(
			requests.lock().unwrap().len(),
			index + 1,
			"duplicate triggers cannot synthesize the same source group again"
		);
		if index > 0 {
			assert!(
				observations[0].content.text.contains(texts[0])
					&& observations[0].content.text.contains(texts[1]),
				"conflicting evidence remains available and unverified"
			);
		}
	}
	supported.store(true, Ordering::SeqCst);
	let mut body = content("Alice lives in Osaka. アリスは大阪に住んでいる。");
	body.entities = vec![Entity {
		name: "Alice".into(),
		category: "person".into(),
		aliases: vec![],
	}];
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Correct {
				id: ids[1],
				expected_revision: 1,
				content: body,
			},
		),
	)
	.await
	.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let units = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			bank: bank.clone(),
			provider: reference("p"),
		},
	)
	.await
	.unwrap();
	assert!(
		units
			.iter()
			.all(|unit| unit.content.kind != Kind::Observation),
		"a model cannot restore the stale observation by claiming verification"
	);
	supported.store(false, Ordering::SeqCst);
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Delete {
				id: ids[0],
				expected_revision: 1,
			},
		),
	)
	.await
	.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let units = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			bank: bank.clone(),
			provider: reference("p"),
		},
	)
	.await
	.unwrap();
	let observations: Vec<_> = units
		.iter()
		.filter(|unit| unit.content.kind == Kind::Observation)
		.collect();
	assert_eq!(observations.len(), 1);
	assert_ne!(
		observations[0].id,
		observation.unwrap(),
		"the deleted earliest seed cannot be resurrected"
	);
	assert_eq!(observations[0].content.evidence.len(), 2);
	assert!(
		observations[0]
			.content
			.evidence
			.iter()
			.all(|proof| !matches!(proof,Evidence::Unit{id,..} if *id==ids[0]))
	);
	let count = requests.lock().unwrap().len();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	assert_eq!(
		requests.lock().unwrap().len(),
		count,
		"completed repair jobs do not repeat synthesis"
	);
	server.abort();
}

#[rstest]
#[tokio::test]
async fn listing_enforces_the_provenance_budget_separately_from_bank_unit_count(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use aidash_server::apps::registry::models::Definition;
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
	let mut previous = None;
	let mut last = Uuid::nil();
	for _ in 0..4 {
		let mut value = content("Evidence chain exceeds the reduced listing budget");
		if let Some(evidence) = previous {
			value.evidence = vec![evidence];
		}
		last = Uuid::now_v7();
		let units = memory::mutate(
			&store,
			&Actor::Operator,
			mutation(
				&bank,
				Change::Add {
					id: last,
					content: value,
				},
			),
		)
		.await
		.unwrap();
		previous = Some(units[0].evidence());
	}
	// Fixture a reduced current policy after admission, without discarding history.
	let mut db = database.lease.handle();
	let mut provider = Definition::objects()
		.filter(Definition::field_id().eq("p".to_string()))
		.filter(Definition::field_version().eq("1.0.0".to_string()))
		.get_with_db(&mut db)
		.await
		.unwrap();
	let limits = &mut provider.metadata.0["config"]["policy"]["bounds"];
	limits["max_graph_visits"] = json!(2);
	Definition::objects()
		.update_with_conn(&mut db, &provider)
		.await
		.unwrap();
	let read = memory::ReadBank {
		provider: reference("p"),
		bank,
	};
	assert!(matches!(
		memory::list(&store, &Actor::Operator, read.clone()).await,
		Err(aidash_server::Error::Invalid(message)) if message == "memory evidence traversal exceeds its bound"
	));
	assert!(
		matches!(
			aidash_server::semantic::worker::sweep(&store).await,
			Err(aidash_server::Error::Invalid(message)) if message == "memory evidence traversal exceeds its bound"
		),
		"semantic indexing must enforce the same reduced provenance cap"
	);
	provider.metadata.0["config"]["policy"]["bounds"]["max_graph_visits"] = json!(8);
	Definition::objects()
		.update_with_conn(&mut db, &provider)
		.await
		.unwrap();
	let units = memory::list(&store, &Actor::Operator, read).await.unwrap();
	assert_eq!(units.len(), 4);
	assert!(units.iter().any(|unit| unit.id == last));
}

#[rstest]
#[tokio::test]
async fn participant_memory_requires_an_enabled_provider_initially_and_after_upgrade(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, registry, workspace) = setup(&database, bounds).await;
	let disabled =
		json!({"model":reference("m"),"instructions":"Memory is disabled","tools":[],"skills":[]});
	registry
		.register(entry("agent", "disabled", disabled.clone()))
		.await
		.unwrap();
	let initial = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("disabled"),
		},
	)
	.await
	.unwrap();
	let enabled = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&enabled.bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("Previously enabled memory"),
			},
		),
	)
	.await
	.unwrap();
	let mut version = entry("agent", "a", disabled);
	version.version = "1.1.0".into();
	registry.register(version).await.unwrap();
	let upgraded = memory::upgrade_participant(
		&store,
		&Actor::Operator,
		enabled.bank,
		memory::UpgradeParticipant {
			agent: EntityRef {
				id: "a".into(),
				version: "1.1.0".into(),
			},
			expected_revision: 1,
		},
	)
	.await
	.unwrap();
	for bank in [initial.bank, upgraded.bank] {
		let read = memory::list(
			&store,
			&Actor::Operator,
			memory::ReadBank {
				provider: reference("p"),
				bank: bank.clone(),
			},
		)
		.await;
		assert!(matches!(read, Err(aidash_server::Error::Conflict(_))));
		let write = memory::mutate(
			&store,
			&Actor::Operator,
			mutation(
				&bank,
				Change::Add {
					id: Uuid::now_v7(),
					content: content("Must not be accepted"),
				},
			),
		)
		.await;
		assert!(matches!(write, Err(aidash_server::Error::Conflict(_))));
		let recall = memory::operate(
			&store,
			&Actor::Operator,
			memory::Operation {
				operation_id: Uuid::now_v7(),
				provider: reference("p"),
				bank,
				action: memory::Action::Recall {
					query: RecallQuery {
						text: "Memory".into(),
						time: None,
						kinds: vec![],
						max_tokens: 1024,
					},
				},
			},
		)
		.await;
		assert!(
			matches!(recall, Err(aidash_server::Error::Conflict(_))),
			"disabled participants cannot call model roles"
		);
	}
}
