//! Exercise the sole production read writer in a real database, including a
//! deliberately broad fixture scope so RLS cannot hide a missing Tenant filter.
use super::super::fixture::{bounds, content, reference, setup};
use super::*;
use crate::apps::execution::test_database::{DatabaseFixture, database};
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_domain::{
	memory::{Bank, Unit},
	semantic::remote::{NativeBank, NativeRecall, NativeRequest, Provider},
};
use rstest::rstest;

#[rstest]
#[case::missing_source("missing")]
#[case::cross_tenant_source("foreign")]
#[case::historical_revision("history")]
#[tokio::test]
async fn node_purge_source_record_retains_tenant_existence_and_historical_revision(
	#[future] database: DatabaseFixture,
	bounds: aidash_domain::memory::Bounds,
	#[case] scenario: &str,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let pool = store.pool.fixture_driver();
	let node = aidash_domain::nodes::NodeName::derived("acme").to_string();
	let bank_id = Uuid::new_v4();
	let bank = Bank {
		home: store
			.deployment
			.node(aidash_domain::nodes::NodeName::parse(&node).unwrap())
			.to_string(),
		tenant: "acme".into(),
		workspace,
		participant: None,
	};
	sqlx::query(
		"INSERT INTO memory_banks (id,tenant,home,workspace_id,revision) VALUES ($1,'acme',$2,$3,1)",
	)
	.bind(bank_id)
	.bind(&bank.home)
	.bind(workspace)
	.execute(pool)
	.await
	.unwrap();
	let unit_id = Uuid::new_v4();
	let mut source_bank = bank_id;
	if scenario == "foreign" {
		let foreign = aidash_domain::nodes::NodeName::derived("other").to_string();
		let other_workspace = Uuid::new_v4();
		sqlx::query("INSERT INTO tenants (name,personal,created_at,created_by_kind) VALUES ('other',false,clock_timestamp(),'deployment_administrator')").execute(pool).await.unwrap();
		sqlx::query(
			"INSERT INTO node_names (name,tenant,created_at) VALUES ($1,'other',clock_timestamp())",
		)
		.bind(&foreign)
		.execute(pool)
		.await
		.unwrap();
		sqlx::query("INSERT INTO nodes (name,tenant,label,is_default,created_at,created_by_kind) VALUES ($1,'other','Other',true,clock_timestamp(),'deployment_administrator')").bind(&foreign).execute(pool).await.unwrap();
		sqlx::query("INSERT INTO workspaces (id,tenant,node,title,goal,state,revision) VALUES ($1,'other',$2,'Other','Other','{}',1)").bind(other_workspace).bind(foreign).execute(pool).await.unwrap();
		source_bank = Uuid::new_v4();
		sqlx::query(
			"INSERT INTO memory_banks (id,tenant,home,workspace_id,revision) VALUES ($1,'other',$2,$3,1)",
		)
		.bind(source_bank)
		.bind(&bank.home)
		.bind(other_workspace)
		.execute(pool)
		.await
		.unwrap();
	}
	if scenario != "missing" {
		let tenant = if scenario == "foreign" {
			"other"
		} else {
			"acme"
		};
		sqlx::query("INSERT INTO memory_units (id,tenant,bank_id,revision,text,kind,learning,verification,entities,evidence,links,learned_at,updated_at,deleted,stale) VALUES ($1,$2,$3,2,'body','world','fact','unverified','[]','[]','[]',clock_timestamp(),clock_timestamp(),false,false)").bind(unit_id).bind(tenant).bind(source_bank).execute(pool).await.unwrap();
	}
	let credential = Uuid::new_v4();
	sqlx::query("INSERT INTO authorization_credentials (id,tenant,subject,token_hash,created_at,expires_at,issued_by) VALUES ($1,'acme','reader',$2,clock_timestamp(),clock_timestamp()+interval '1 day','fixture')").bind(credential).bind(vec![1u8;32]).execute(pool).await.unwrap();
	let task = Uuid::new_v4();
	sqlx::query("INSERT INTO tasks (id,tenant,workspace_id,title,description,status,revision,created_by) VALUES ($1,'acme',$2,'reader','reader','OPEN',1,'reader')").bind(task).bind(workspace).execute(pool).await.unwrap();
	let grant = Uuid::new_v4();
	sqlx::query("INSERT INTO authorization_remote_grants (id,tenant,task_id,task_revision,workspace_id,node,node_id,credential_id,root_subject,subject_chain,inspection,expires_at,revoked,semantic) VALUES ($1,'acme',$2,1,$3,$4,'aidash://peer',$5,'reader',ARRAY['reader'],'{}',clock_timestamp()+interval '1 day',false,'{\"mode\":\"disabled\"}')").bind(grant).bind(task).bind(workspace).bind(node).bind(credential).execute(pool).await.unwrap();
	let provider = Provider {
		node_id: bank.home.clone(),
		entry: reference("p"),
		digest: "fixture".into(),
		configuration_digest: "fixture".into(),
	};
	let binding = NativeBinding {
		selection: NativeRequest {
			participant: Uuid::new_v4(),
			expected_revision: 1,
			provider: reference("p"),
		},
		generation: None,
		participant: aidash_domain::memory::Binding {
			bank: bank.clone(),
			participant_revision: 1,
			agent: reference("a"),
			provider: reference("p"),
		},
		agent: provider.clone(),
		banks: vec![NativeBank {
			bank: bank.clone(),
			provider,
			roles: vec![],
			max_model_tokens: 128,
			max_context_tokens: 128,
			cache_max_age_seconds: 60,
			cache_max_attempts: 2,
		}],
	};
	let now = chrono::Utc::now();
	let context = NativeContext {
		banks: vec![NativeRecall {
			bank: bank.clone(),
			provider: reference("p"),
			recall: Recall::Ready {
				units: vec![Unit {
					id: unit_id,
					bank,
					revision: 1,
					content: content("historical body"),
					learned_at: now,
					updated_at: now,
					deleted: false,
					stale: false,
				}],
			},
		}],
	};
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	let result = record(&mut tx, grant, &binding, &context, 4).await;
	if scenario == "history" {
		result.unwrap();
		let sql = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("tenant").eq(Expr::value("acme")))
			.and_where(Expr::col("id").eq(Expr::value(unit_id)))
			.lock(LockType::Update)
			.lock_behavior(reinhardt::query::LockBehavior::Nowait)
			.to_string(PostgresQueryBuilder);
		let blocked = sqlx::query(&sql).fetch_optional(pool).await.unwrap_err();
		assert_eq!(
			blocked.as_database_error().unwrap().code().as_deref(),
			Some("55P03"),
			"source KeyShare lives until read INSERT commits"
		);
		tx.commit().await.unwrap();
		sqlx::query("DELETE FROM memory_units WHERE tenant='acme' AND id=$1")
			.bind(unit_id)
			.execute(pool)
			.await
			.unwrap();
		let revision: i64 =
			sqlx::query_scalar("SELECT revision FROM memory_remote_reads WHERE grant_id=$1")
				.bind(grant)
				.fetch_one(pool)
				.await
				.unwrap();
		assert_eq!(
			revision, 1,
			"read identity is historical evidence, not a current revision FK"
		);
	} else {
		assert!(
			matches!(result, Err(Error::RemoteSemantic(Failure::Invalidated))),
			"{result:?}"
		);
		tx.rollback().await.unwrap();
		let count: i64 =
			sqlx::query_scalar("SELECT count(*) FROM memory_remote_reads WHERE grant_id=$1")
				.bind(grant)
				.fetch_one(pool)
				.await
				.unwrap();
		assert_eq!(count, 0);
	}
}

/// The real Home journal completes an Entry-only reply, then the receiver
/// pauses before persistence. Source erasure must win over that late reply.
#[rstest]
#[case::unchanged_receiver_fence(false, false, None)]
#[case::completed_local_home(false, true, None)]
#[case::late_reply_after_source_invalidation(true, true, None)]
#[case::prepare_only_receiver(false, false, Some("PENDING"))]
#[case::cancelled_reply(false, false, Some("CANCELLED"))]
#[case::paused_reply(false, false, Some("PAUSED"))]
#[case::active_reply(false, false, Some("ACTIVE"))]
#[case::waiting_reply(false, false, Some("WAITING"))]
#[case::different_ready_reply(false, false, Some("READY"))]
#[tokio::test]
async fn node_purge_entry_only_cache_invalidation_fences_real_reply_persistence(
	#[future] database: DatabaseFixture,
	bounds: aidash_domain::memory::Bounds,
	#[case] invalidated: bool,
	#[case] advanced: bool,
	#[case] blocked_state: Option<&str>,
) {
	use aidash_application::{
		ports::semantic::remote_journal::JournalRepository, semantic::remote_journal as journal,
	};
	use aidash_domain::semantic::{
		remote::{Binding, Boundary, Operation, Receipt, SourceRead},
		results::{Match, SearchResult},
	};
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let pool = store.pool.fixture_driver();
	let node = aidash_domain::nodes::NodeName::derived("acme");
	let home = store.deployment.node(node.clone()).to_string();
	let task = Uuid::new_v4();
	let run = Uuid::new_v4();
	let credential = Uuid::new_v4();
	let grant = Uuid::new_v4();
	sqlx::query("INSERT INTO authorization_credentials(id,tenant,subject,token_hash,created_at,expires_at,issued_by) VALUES ($1,'acme','reader',$2,clock_timestamp(),clock_timestamp()+interval '1 day','fixture')").bind(credential).bind(vec![9u8;32]).execute(pool).await.unwrap();
	sqlx::query("INSERT INTO tasks(id,tenant,workspace_id,title,description,status,revision,created_by) VALUES ($1,'acme',$2,'reader','reader','COMPLETED',1,'reader')").bind(task).bind(workspace).execute(pool).await.unwrap();
	let pending = aidash_domain::run_state::encode(
		&aidash_domain::run_state::RunState::Completed(aidash_domain::run_state::TerminalState {}),
		&aidash_domain::run_state::RecoveryState::default(),
	)
	.unwrap();
	sqlx::query("INSERT INTO runs(id,tenant,task_id,workspace_id,node,home_node,agent_id,agent_version,phase,pending,revision) VALUES ($1,'acme',$2,$3,$4,$5,'a','1.0.0','COMPLETED',$6,1)").bind(run).bind(task).bind(workspace).bind(node.as_str()).bind(&home).bind(pending).execute(pool).await.unwrap();
	sqlx::query("INSERT INTO authorization_remote_grants(id,tenant,task_id,task_revision,workspace_id,node,node_id,credential_id,root_subject,subject_chain,inspection,expires_at,revoked,semantic) VALUES ($1,'acme',$2,1,$3,$4,'aidash://peer',$5,'reader',ARRAY['reader'],'{}',clock_timestamp()+interval '1 day',false,'{\"mode\":\"disabled\"}')").bind(grant).bind(task).bind(workspace).bind(node.as_str()).bind(credential).execute(pool).await.unwrap();
	let entry = crate::semantic::service::put(
		&store,
		&crate::authorization::identity::Actor::DeploymentAdministrator,
		workspace,
		crate::semantic::PutEntry {
			key: "source".into(),
			expected_revision: 0,
			source: aidash_domain::semantic::Source::Memory {
				text: "source body".into(),
			},
			agent: None,
			metadata: serde_json::json!({}),
		},
	)
	.await
	.unwrap()
	.id;
	let binding = Binding::RequiredHome {
		home_lineage: vec![],
		execution_lineage: vec![],
		version: 1,
		index_revision: 1,
		index_digest: "fixture".into(),
		embedding: Box::new(Provider {
			node_id: home.clone(),
			entry: reference("e"),
			digest: "fixture".into(),
			configuration_digest: "fixture".into(),
		}),
		native: None,
		decider: None,
		summarizer: None,
	};
	let mut operation = Operation {
		id: Uuid::nil(),
		home_node: home.clone(),
		grant_id: grant,
		admission_id: run,
		boundary: Boundary {
			step: 0,
			input_sequence: 0,
			task_revision: 1,
			inputs_digest: "fixture".into(),
		},
		inputs: vec![],
		query: "source".into(),
		max_tokens: 2048,
		metadata: serde_json::json!({}),
	};
	operation.set_id().unwrap();
	let scope = aidash_application::tenancy::TenancyScope::tenant("acme").unwrap();
	let repository = crate::apps::knowledge::repositories::remote_journal::Repository {
		store: &store,
		scope: scope.clone(),
	};
	let prepared = journal::prepare(&repository, &operation, &binding)
		.await
		.unwrap();

	let receipt = Receipt {
		operation_id: operation.id,
		operation_digest: operation.digest().unwrap(),
		home_node: home,
		tenant: "acme".into(),
		workspace_id: workspace,
		grant_id: grant,
		admission_id: run,
		executor: "reader".into(),
		binding,
		retrieved_at: chrono::Utc::now(),
		query_truncated: false,
		candidate_digest: "fixture".into(),
		sources: vec![SourceRead {
			entry_id: entry,
			revision: 1,
			content_digest: crate::semantic::service::content_digest("source body"),
		}],
		result: SearchResult {
			workspace_id: workspace,
			index_revision: 1,
			model: "fixture".into(),
			model_version: "1".into(),
			matches: vec![Match {
				entry_id: entry,
				revision: 1,
				source: serde_json::json!({"text":"source body"}),
				agent: None,
				metadata: serde_json::json!({}),
				text: "source body".into(),
				score: 1.0,
			}],
			estimated_tokens: 16,
			truncated: false,
		},
		memory: None,
		estimated_tokens: 16,
	};
	if blocked_state != Some("PENDING") {
		let aidash_domain::semantic::remote::journal::Claim::Attempt(attempt) =
			journal::claim(&repository, operation.id).await.unwrap()
		else {
			panic!("new operation requires attempt")
		};
		journal::complete(&repository, &attempt, &receipt, "fixture")
			.await
			.unwrap();
	}
	let completed = repository.bound(operation.id).await.unwrap();
	if blocked_state != Some("PENDING") {
		assert!(
			completed.fence > prepared.fence,
			"real Home claim advances shared fence"
		);
	}
	if let Some(state) = blocked_state.filter(|state| *state != "PENDING") {
		sqlx::query("UPDATE semantic_remote_operations SET state=$2,receipt=NULL,error='retries_exhausted' WHERE id=$1").bind(operation.id).bind(state).execute(pool).await.unwrap();
		let mut tx = store.pool.begin(&scope).await.unwrap();
		let error = crate::apps::identity::services::peer::semantic::persist_reply_in(
			&store,
			&mut tx,
			run,
			&operation,
			&receipt,
			completed.fence,
		)
		.await
		.unwrap_err();
		let expected = match state {
			"CANCELLED" => Failure::Authority,
			"PAUSED" => Failure::RetriesExhausted,
			"READY" => Failure::Invalidated,
			_ => Failure::Pending,
		};
		assert!(
			matches!(error,Error::RemoteSemantic(actual) if actual==expected),
			"state={state}"
		);
		tx.rollback().await.unwrap();
		let saved: i64 = sqlx::query_scalar(
			"SELECT count(*) FROM semantic_remote_receipts WHERE operation_id=$1",
		)
		.bind(operation.id)
		.fetch_one(pool)
		.await
		.unwrap();
		assert_eq!(saved, 0, "refusal precedes body writes");
		return;
	}

	let expected = if advanced {
		prepared.fence
	} else {
		completed.fence
	};
	let mut tx = store.pool.begin(&scope).await.unwrap();
	crate::apps::identity::services::peer::semantic::persist_reply_in(
		&store, &mut tx, run, &operation, &receipt, expected,
	)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	if invalidated {
		let mut tx = store.pool.begin(&scope).await.unwrap();
		erase_entry_receipts_in(&mut *tx, "acme", entry)
			.await
			.unwrap();
		// The real Entry writer creates points. Purge erases these owned children
		// before their Entry; reproduce that ordering without weakening the FK.
		let (sql, values) = Query::delete()
			.from_table(Alias::new("semantic_points"))
			.and_where(Expr::col("tenant").eq(Expr::value("acme")))
			.and_where(Expr::col("entry_id").eq(Expr::value(entry)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, reinhardt::db::orm::execution::convert_values(values))
			.await
			.unwrap();
		crate::database::native::query(
			&Query::delete()
				.from_table(Alias::new("semantic_entries"))
				.and_where(Expr::col("id").eq(Expr::value(entry)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
		tx.commit().await.unwrap();
		// INVALIDATED is not an authorized retry allowance. The real resume owner
		// refuses it, and the already received old reply remains unusable.
		let mut tx = store.pool.begin(&scope).await.unwrap();
		assert!(
			crate::apps::knowledge::services::remote::journal::resume_in(
				&store, &mut tx, grant, run, workspace, "reader"
			)
			.await
			.is_err()
		);
		tx.rollback().await.unwrap();
		let mut tx = store.pool.begin(&scope).await.unwrap();
		assert!(matches!(
			crate::apps::identity::services::peer::semantic::persist_reply_in(
				&store, &mut tx, run, &operation, &receipt, expected
			)
			.await,
			Err(Error::RemoteSemantic(Failure::Invalidated))
		));
		tx.rollback().await.unwrap();
		let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM semantic_remote_reads WHERE grant_id=$1 AND entry_id=$2),(SELECT count(*) FROM semantic_remote_receipts WHERE operation_id=$3),(SELECT count(*) FROM semantic_remote_operations WHERE id=$3 AND receipt IS NULL AND state='INVALIDATED')").bind(grant).bind(entry).bind(operation.id).fetch_one(pool).await.unwrap();
		assert_eq!(
			counts,
			(1, 0, 1),
			"content-free source evidence stays; both body caches are erased"
		);
	}
	let native_reads: i64 =
		sqlx::query_scalar("SELECT count(*) FROM memory_remote_reads WHERE grant_id=$1")
			.bind(grant)
			.fetch_one(pool)
			.await
			.unwrap();
	assert_eq!(
		native_reads, 0,
		"Entry-only control cannot pass through Unit invalidation"
	);
}
