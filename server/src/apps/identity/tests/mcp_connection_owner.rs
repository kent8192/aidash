//! The persisted chain root, rather than the executing agent, owns MCP credentials.
use super::common;
use aidash_server::apps::identity::repositories::oauth_connections::owner_of_run as owner;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::json;

#[rstest::rstest]
#[tokio::test]
async fn mcp_connection_owner_is_only_the_local_runs_chain_root_person(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, _, _) = fixture.runtime.parts();
	let (_, token, _) = common::bootstrap(&f, &fixture.application, "http://127.0.0.1:9").await;
	let (status, created) = common::request(&fixture.application, &token, "POST", "/api/conversations",
		json!({"title":"MCP owner", "goal":"Call a personal MCP server", "target":{"id":"research","version":"1.0.0"},"target_kind":"agent"})).await;
	assert_eq!(status, 200, "{created}");
	let scope = aidash_application::tenancy::TenancyScope::tenant("acme").unwrap();
	let mut run = f.store.runs(&scope).await.unwrap().pop().unwrap();
	let found = owner(&f.store, &run).await.unwrap().unwrap();
	assert_eq!(found.tenant, "acme");
	assert_eq!(found.subject, "alice");
	let home = run.home_node.clone();
	run.home_node = "aidash://remote/home".into();
	assert!(owner(&f.store, &run).await.unwrap().is_none());
	run.home_node = home;
	let id = run.id;
	run.id = uuid::Uuid::new_v4();
	assert!(
		owner(&f.store, &run).await.unwrap().is_none(),
		"calls without a Run grant have no person"
	);
	run.id = id;
	let authorization = aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	};
	let snapshot = authorization.snapshot("acme").await.unwrap();
	let mut bundle = snapshot.bundle;
	bundle.subjects.insert(
		"mcp-service".into(),
		aidash_domain::policy::Subject {
			kind: aidash_domain::policy::SubjectKind::Service,
			groups: Default::default(),
			roles: Default::default(),
			attributes: json!({}),
			enabled: true,
			delegated_by: None,
		},
	);
	authorization
		.replace("acme", snapshot.revision, bundle, "alice")
		.await
		.unwrap();
	let mut tx = f.store.pool.begin(&scope).await.unwrap();
	let query = Query::update()
		.table(Alias::new("authorization_execution"))
		.value(Alias::new("root_subject"), "mcp-service")
		.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run.id)))
		.to_string(PostgresQueryBuilder);
	aidash_server::database::native::query(&query)
		.execute(&mut *tx)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	assert!(
		owner(&f.store, &run).await.unwrap().is_none(),
		"a service root cannot borrow a person's connection"
	);
}
