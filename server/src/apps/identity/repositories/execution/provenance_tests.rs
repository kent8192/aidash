//! Source retention cannot turn a Federated descendant into Local authority.
use super::*;
use crate::apps::execution::test_database::{DatabaseFixture, database};
use aidash_application::authorization::Snapshot;
use rstest::rstest;
use serde_json::json;

#[rstest]
#[case::pruned_execution("run")]
#[case::pruned_home("home")]
#[tokio::test]
async fn node_purge_pruned_source_keeps_descendant_admission_federated(
	#[future] database: DatabaseFixture,
	#[case] scenario: &str,
) {
	let database = database.await;
	let pool = database.pool().await;
	let driver = pool.fixture_driver();
	let store = Store::from_pool(
		database.connection.clone().into_postgres().unwrap(),
		aidash_domain::nodes::DeploymentId::parse("aidash://origin-fixture").unwrap(),
	)
	.await
	.unwrap();
	let node = aidash_domain::nodes::NodeName::derived("acme").to_string();
	let document = json!({"tenant":"acme","subjects":{"reader":{"kind":"user"}}});
	sqlx::query("INSERT INTO tenants (name,personal,created_at,created_by_kind) VALUES ('acme',false,clock_timestamp(),'deployment_administrator')").execute(driver).await.unwrap();
	sqlx::query(
		"INSERT INTO node_names (name,tenant,created_at) VALUES ($1,'acme',clock_timestamp())",
	)
	.bind(&node)
	.execute(driver)
	.await
	.unwrap();
	sqlx::query("INSERT INTO nodes (name,tenant,label,is_default,created_at,created_by_kind) VALUES ($1,'acme','Default',true,clock_timestamp(),'deployment_administrator')").bind(&node).execute(driver).await.unwrap();
	sqlx::query(
		"INSERT INTO authorization_bundles (tenant,revision,document) VALUES ('acme',1,$1)",
	)
	.bind(&document)
	.execute(driver)
	.await
	.unwrap();
	let workspace = Uuid::new_v4();
	sqlx::query("INSERT INTO workspaces (id,tenant,node,title,goal,state,revision) VALUES ($1,'acme',$2,'Origin','Origin','{}',1)").bind(workspace).bind(&node).execute(driver).await.unwrap();
	sqlx::query("INSERT INTO authorization_workspaces (workspace_id,tenant,owner_subject) VALUES ($1,'acme','reader')").bind(workspace).execute(driver).await.unwrap();
	let sibling = aidash_domain::nodes::NodeName::derived("sibling").to_string();
	sqlx::query(
		"INSERT INTO node_names(name,tenant,created_at) VALUES ($1,'acme',clock_timestamp())",
	)
	.bind(&sibling)
	.execute(driver)
	.await
	.unwrap();
	sqlx::query("INSERT INTO nodes(name,tenant,label,is_default,created_at,created_by_kind) VALUES ($1,'acme','Sibling',false,clock_timestamp(),'deployment_administrator')").bind(&sibling).execute(driver).await.unwrap();
	let sibling_workspace = Uuid::new_v4();
	sqlx::query("INSERT INTO workspaces(id,tenant,node,title,goal,state,revision) VALUES ($1,'acme',$2,'Sibling','Sibling','{}',1)").bind(sibling_workspace).bind(&sibling).execute(driver).await.unwrap();
	sqlx::query("INSERT INTO authorization_workspaces(workspace_id,tenant,owner_subject) VALUES ($1,'acme','reader')").bind(sibling_workspace).execute(driver).await.unwrap();
	let foreign_workspace = Uuid::new_v4();
	let foreign_node = aidash_domain::nodes::NodeName::derived("other").to_string();
	sqlx::query("INSERT INTO tenants(name,personal,created_at,created_by_kind) VALUES ('other',false,clock_timestamp(),'deployment_administrator')").execute(driver).await.unwrap();
	sqlx::query(
		"INSERT INTO node_names(name,tenant,created_at) VALUES ($1,'other',clock_timestamp())",
	)
	.bind(&foreign_node)
	.execute(driver)
	.await
	.unwrap();
	sqlx::query("INSERT INTO nodes(name,tenant,label,is_default,created_at,created_by_kind) VALUES ($1,'other','Other',true,clock_timestamp(),'deployment_administrator')").bind(&foreign_node).execute(driver).await.unwrap();
	sqlx::query("INSERT INTO workspaces(id,tenant,node,title,goal,state,revision) VALUES ($1,'other',$2,'Other','Other','{}',1)").bind(foreign_workspace).bind(&foreign_node).execute(driver).await.unwrap();
	sqlx::query("INSERT INTO authorization_bundles(tenant,revision,document) VALUES ('other',1,'{\"tenant\":\"other\"}')").execute(driver).await.unwrap();
	sqlx::query("INSERT INTO authorization_workspaces(workspace_id,tenant,owner_subject) VALUES ($1,'other','reader')").bind(foreign_workspace).execute(driver).await.unwrap();
	let credential = Uuid::new_v4();
	sqlx::query("INSERT INTO authorization_credentials (id,tenant,subject,token_hash,created_at,expires_at,issued_by) VALUES ($1,'acme','reader',$2,clock_timestamp(),clock_timestamp()+interval '1 day','fixture')").bind(credential).bind(vec![1u8;32]).execute(driver).await.unwrap();
	let parent = Uuid::new_v4();
	let child = Uuid::new_v4();
	let source = Uuid::new_v4();
	for task in [parent, child] {
		sqlx::query("INSERT INTO tasks (id,tenant,workspace_id,title,description,status,revision,created_by) VALUES ($1,'acme',$2,'Origin','Origin','COMPLETED',1,'reader')").bind(task).bind(if task==child { sibling_workspace } else { workspace }).execute(driver).await.unwrap();
	}
	let pending = aidash_domain::run_state::encode(
		&aidash_domain::run_state::RunState::Completed(aidash_domain::run_state::TerminalState {}),
		&aidash_domain::run_state::RecoveryState::default(),
	)
	.unwrap();
	sqlx::query("INSERT INTO runs (id,tenant,task_id,workspace_id,node,home_node,agent_id,agent_version,phase,pending,revision) VALUES ($1,'acme',$2,$3,$4,'aidash://origin-fixture','fixture','1.0.0','COMPLETED',$5,1)").bind(source).bind(parent).bind(workspace).bind(&sibling).bind(pending).execute(driver).await.unwrap();
	let grant_record = ExecutionGrant {
		run_id: source,
		task_id: parent,
		workspace_id: workspace,
		tenant: "acme".into(),
		credential_id: credential,
		root_subject: "reader".into(),
		subject_chain: vec!["reader".into(), "fixture-agent".into()],
	};
	let mut tx = pool
		.begin(&TenancyScope::tenant("acme").unwrap())
		.await
		.unwrap();
	for invalid in [
		ExecutionGrant {
			workspace_id: sibling_workspace,
			..grant_record.clone()
		},
		ExecutionGrant {
			tenant: "other".into(),
			..grant_record.clone()
		},
	] {
		assert!(matches!(
			crate::apps::identity::repositories::admission::persist_grant_in(&mut *tx, &invalid)
				.await,
			Err(crate::Error::Forbidden)
		));
	}
	// Broad fixture scope proves explicit source Tenant filtering, not just RLS.
	let mut broad = pool
		.begin(&aidash_application::tenancy::TEST_FIXTURE)
		.await
		.unwrap();
	for absent_or_foreign in [Uuid::new_v4(), foreign_workspace] {
		assert!(matches!(
			crate::apps::identity::models::AuthorizationRunRead::record_sources(
				&mut *broad,
				source,
				absent_or_foreign,
				&[("task".into(), parent)]
			)
			.await,
			Err(crate::Error::Forbidden)
		));
	}
	broad.rollback().await.unwrap();
	crate::apps::identity::repositories::admission::persist_grant_in(&mut *tx, &grant_record)
		.await
		.unwrap();
	crate::apps::identity::models::AuthorizationRunRead::record_sources(
		&mut *tx,
		source,
		workspace,
		&[("task".into(), parent)],
	)
	.await
	.unwrap();
	store
		.record_output_in(&mut tx, Some(source), workspace, "task", parent)
		.await
		.unwrap();
	tx.commit().await.unwrap();

	sqlx::query("INSERT INTO authorization_task_origins (task_id,tenant,source_run_id,root_subject,subject_chain) VALUES ($1,'acme',$2,'reader',ARRAY['reader','fixture-agent'])").bind(child).bind(source).execute(driver).await.unwrap();
	let grant = Uuid::new_v4();
	sqlx::query("INSERT INTO authorization_remote_grants (id,tenant,task_id,task_revision,workspace_id,node,node_id,credential_id,root_subject,subject_chain,inspection,expires_at,revoked,semantic) VALUES ($1,'acme',$2,1,$3,$4,'aidash://peer',$5,'reader',ARRAY['reader'],'{}',clock_timestamp()+interval '1 day',false,'{\"mode\":\"disabled\"}')").bind(grant).bind(parent).bind(workspace).bind(node).bind(credential).execute(driver).await.unwrap();
	sqlx::query("INSERT INTO authorization_remote_outputs (grant_id,tenant,workspace_id,resource_kind,resource_id) VALUES ($1,'acme',$2,'task',$3)").bind(grant).bind(workspace).bind(parent).execute(driver).await.unwrap();
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: credential,
		tenant: "acme".into(),
		subject: "reader".into(),
	};
	for pruned in [false, true] {
		if pruned {
			if scenario == "run" {
				sqlx::query("DELETE FROM authorization_run_outputs WHERE run_id=$1")
					.bind(source)
					.execute(driver)
					.await
					.unwrap();
				sqlx::query("DELETE FROM authorization_run_reads WHERE run_id=$1")
					.bind(source)
					.execute(driver)
					.await
					.unwrap();
				sqlx::query("DELETE FROM authorization_execution WHERE run_id=$1")
					.bind(source)
					.execute(driver)
					.await
					.unwrap();
				sqlx::query("DELETE FROM runs WHERE id=$1")
					.bind(source)
					.execute(driver)
					.await
					.unwrap();
			} else {
				sqlx::query("DELETE FROM authorization_remote_outputs WHERE grant_id=$1")
					.bind(grant)
					.execute(driver)
					.await
					.unwrap();
				sqlx::query("DELETE FROM authorization_remote_grants WHERE id=$1")
					.bind(grant)
					.execute(driver)
					.await
					.unwrap();
				sqlx::query("DELETE FROM tasks WHERE id=$1")
					.bind(parent)
					.execute(driver)
					.await
					.unwrap();
				sqlx::query("DELETE FROM authorization_workspaces WHERE workspace_id=$1")
					.bind(workspace)
					.execute(driver)
					.await
					.unwrap();
				sqlx::query("DELETE FROM workspaces WHERE id=$1")
					.bind(workspace)
					.execute(driver)
					.await
					.unwrap();
				let retained:i64=sqlx::query_scalar("SELECT count(*) FROM authorization_execution e JOIN authorization_run_reads r USING (run_id) WHERE run_id=$1 AND e.task_id=$2 AND r.resource_id=$2").bind(source).bind(parent).fetch_one(driver).await.unwrap();
				assert_eq!(
					retained, 1,
					"Run-owned authority and read provenance outlive Home"
				);
			}
		}
		let tx = pool
			.begin(&TenancyScope::tenant("acme").unwrap())
			.await
			.unwrap();
		let mut access = Access::from_transaction(
			tx,
			&pool,
			"aidash://origin-fixture",
			None,
			&identity,
			Snapshot {
				revision: 1,
				bundle: serde_json::from_value(document.clone()).unwrap(),
			},
		);
		if pruned && scenario == "home" {
			assert!(
				store
					.record_output_in(&mut access.tx, Some(source), workspace, "task", parent)
					.await
					.is_err(),
				"store output writer also rejects missing live source"
			);
			assert!(
				access.workspace(workspace).await.is_err(),
				"missing Home still denies current authority"
			);
			assert!(
				crate::apps::identity::models::AuthorizationRunRead::record_sources(
					&mut *access.tx,
					source,
					workspace,
					&[("task".into(), parent)]
				)
				.await
				.is_err(),
				"history is not live Workspace existence"
			);
		}
		let federated = GrantScope {
			access: &mut access,
		}
		.federated_origin(child)
		.await
		.unwrap();
		access.finish(Ok(())).await.unwrap();
		assert!(
			federated,
			"source pruned={pruned} must retain Remote admission restrictions"
		);
	}
	let retained: (Uuid,String,Vec<String>) = sqlx::query_as("SELECT source_run_id,root_subject,subject_chain FROM authorization_task_origins WHERE task_id=$1").bind(child).fetch_one(driver).await.unwrap();
	assert_eq!(
		retained,
		(
			source,
			"reader".into(),
			vec!["reader".into(), "fixture-agent".into()]
		)
	);
}
