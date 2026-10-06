//! Native operator staging and exact review selection on disposable services.
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_server::authorization::{Authorization, policy::PolicyBundle};
use common::{TestEnvironment, request, test_environment};
use rstest::rstest;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

#[rstest]
#[tokio::test]
async fn configured_host_defaults_are_pending_only_for_atomic_new_tenant_creation(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (mut f, url, database) = common::setup(&environment).await;
	f.config.default_host_packages = vec!["task_assign".into(), "shell".into()];
	let app = common::application(f.clone()).await;
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	let bundle = json!({"tenant":"new-owner","subjects":{},"policies":[]});
	let create = json!({"expected_revision":0,"bundle":bundle});
	// A failed provisioning gate also rolls back the policy and its history.
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/new-owner",
			create.clone()
		)
		.await
		.0,
		403
	);
	assert!(authorization.snapshot("new-owner").await.is_err());
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"PUT",
			"/api/marketplace/compatibility",
			json!({"enabled":true,"expected_revision":1,"compatible_instances_confirmed":true})
		)
		.await
		.0,
		200
	);
	let (status, created) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/new-owner",
		create.clone(),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	assert_eq!(created["revision"], 1);
	let pending = &created["pending_host_packages"];
	assert!(pending["unavailable"]["shell"].is_string());
	let installations = pending["installations"].as_array().unwrap();
	assert_eq!(installations.len(), 2);
	assert!(
		installations
			.iter()
			.all(|row| row["installation"]["active_revision"].is_null() && row["approved"] == false)
	);
	assert!(authorization.catalog("new-owner").await.unwrap().is_empty());
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/new-owner",
			create
		)
		.await
		.0,
		409
	);
	let (status, updated) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/new-owner",
		json!({"expected_revision":1,"bundle":bundle}),
	)
	.await;
	assert_eq!(status, 200, "{updated}");
	assert_eq!(updated["revision"], 2);
	assert!(updated.get("pending_host_packages").is_none());
	// A subsequent application instance performs no tenant provisioning.
	drop(app);
	let restarted = common::application(f.clone()).await;
	assert!(authorization.catalog("new-owner").await.unwrap().is_empty());
	use reinhardt::query::{Alias, PostgresQueryBuilder, Query, QueryStatementBuilder as _};
	let sql = Query::select()
		.column(Alias::new("document"))
		.from(Alias::new("marketplace_installations"))
		.order_by(Alias::new("key"), reinhardt::query::Order::Asc)
		.to_string(PostgresQueryBuilder);
	let retained: Vec<serde_json::Value> = aidash_server::database::native::query_scalar(&sql)
		.scalar_all(&f.store.pool)
		.await
		.unwrap();
	assert_eq!(retained.len(), 2);
	for row in installations {
		let installation = retained
			.iter()
			.find(|document| document["id"] == row["installation"]["id"])
			.unwrap();
		assert_eq!(installation, &row["installation"]);
	}
	drop(restarted);
	common::cleanup(f, &url, &database).await;
}

#[rstest]
#[tokio::test]
async fn operator_host_packages_remain_pending_until_the_exact_set_is_approved(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (f, url, database) = common::setup(&environment).await;
	let app = common::application(f.clone()).await;
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	let bundle:PolicyBundle=serde_json::from_value(json!({"tenant":"owner","subjects":{"viewer":{"kind":"user"}},"policies":[{"id":"all","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]})).unwrap();
	authorization
		.replace("owner", 0, bundle, "operator")
		.await
		.unwrap();
	let viewer = authorization
		.issue_credential("owner", "viewer", 3600, "operator")
		.await
		.unwrap()
		.token;
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"PUT",
			"/api/marketplace/compatibility",
			json!({"enabled":true,"expected_revision":1,"compatible_instances_confirmed":true})
		)
		.await
		.0,
		200
	);
	let input = json!({"tenant":"owner","groups":["task_assign"],"idempotency_key":Uuid::new_v4()});
	assert_eq!(
		request(
			&app,
			&viewer,
			"POST",
			"/api/marketplace/host-packages",
			input.clone()
		)
		.await
		.0,
		403
	);
	let (status, pending) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/marketplace/host-packages",
		input.clone(),
	)
	.await;
	assert_eq!(status, 200, "{pending}");
	let installations = pending["installations"].as_array().unwrap();
	assert_eq!(installations.len(), 2);
	assert!(
		installations
			.iter()
			.all(|r| r["installation"]["active_revision"].is_null() && r["approved"] == false)
	);
	assert!(authorization.catalog("owner").await.unwrap().is_empty());
	let (status, replay) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/marketplace/host-packages",
		input,
	)
	.await;
	assert_eq!(status, 200, "{replay}");
	assert_eq!(replay, pending);
	let selection = json!({"tenant":"owner",
		"installations":installations.iter().map(|r|json!({"installation":r["installation"]["id"],"revision":r["revision"],"digest":r["digest"],"expected_activation_revision":0})).collect::<Vec<_>>(),
		"approvals":installations.iter().map(|r|json!({"reference":{"id":r["entry"]["id"],"version":r["entry"]["version"]},"expected_catalog_revision":0})).collect::<Vec<_>>()});
	let mut stale = selection.clone();
	stale["installations"][1]["digest"] = json!("changed");
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/marketplace/approval-sets",
			stale
		)
		.await
		.0,
		409
	);
	assert!(authorization.catalog("owner").await.unwrap().is_empty());
	let (status, activated) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/marketplace/approval-sets",
		selection.clone(),
	)
	.await;
	assert_eq!(status, 200, "{activated}");
	assert_eq!(activated.as_array().unwrap().len(), 2);
	assert!(
		activated
			.as_array()
			.unwrap()
			.iter()
			.all(|r| r["active_revision"] == 1 && r["activation_revision"] == 1)
	);
	assert_eq!(authorization.catalog("owner").await.unwrap().len(), 2);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/marketplace/approval-sets",
			selection
		)
		.await
		.0,
		409
	);
	drop(app);
	common::cleanup(f, &url, &database).await;
}
