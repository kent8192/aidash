//! Native Deployment Administrator staging and exact review selection on disposable services.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_server::authorization::{Authorization, policy::PolicyBundle};
use common::request;
use rstest::rstest;
use serde_json::json;

use uuid::Uuid;

#[rstest]
#[tokio::test]
async fn deployment_administrator_host_packages_remain_pending_until_the_exact_set_is_approved(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,
) {
	let (f, url, database) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	let bundle:PolicyBundle=serde_json::from_value(json!({"tenant":"owner","subjects":{"viewer":{"kind":"user"}},"policies":[{"id":"all","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]})).unwrap();
	common::tenant_root_in(f.store.pool.driver(), "owner").await;
	authorization
		.replace("owner", 0, bundle, "deployment_administrator")
		.await
		.unwrap();
	let viewer = authorization
		.issue_credential("owner", "viewer", 3600, "deployment_administrator")
		.await
		.unwrap()
		.token;
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

#[rstest]
#[tokio::test]
async fn research_provider_packages_install_and_activate_like_host_packages(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,
) {
	// Arrange: no Brave profile and no page reading are configured on this Node.
	let (f, url, database) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	let bundle: PolicyBundle =
		serde_json::from_value(json!({"tenant":"owner","subjects":{},"policies":[]})).unwrap();
	common::tenant_root_in(f.store.pool.driver(), "owner").await;
	authorization
		.replace("owner", 0, bundle, "deployment_administrator")
		.await
		.unwrap();
	// Act
	let (status, pending) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/marketplace/host-packages",
		json!({"tenant":"owner","groups":["research_brave","research_page","research_fixture"],"idempotency_key":Uuid::new_v4()}),
	)
	.await;
	// Assert: only the configured provider is staged; the others say why not.
	assert_eq!(status, 200, "{pending}");
	for unavailable in ["research_brave", "research_page"] {
		assert!(
			pending["unavailable"][unavailable]
				.as_str()
				.is_some_and(|reason| reason.contains("PROVIDER_UNAVAILABLE")),
			"{pending}"
		);
	}
	let installations = pending["installations"].as_array().unwrap();
	// Installation assigns new entry IDs; the descriptors identify the provider.
	let mut staged = installations
		.iter()
		.map(|row| {
			let entry = &row["entry"];
			format!(
				"{} {} {}",
				entry["kind"].as_str().unwrap(),
				entry["config"]["provider"].as_str().unwrap_or("-"),
				entry["config"]["operation"].as_str().unwrap_or("-")
			)
		})
		.collect::<Vec<_>>();
	staged.sort();
	assert_eq!(
		staged,
		[
			"bundle - -",
			"tool research.fixture@1 fetch",
			"tool research.fixture@1 find",
			"tool research.fixture@1 search"
		]
	);
	let selection = json!({"tenant":"owner",
		"installations":installations.iter().map(|r|json!({"installation":r["installation"]["id"],"revision":r["revision"],"digest":r["digest"],"expected_activation_revision":0})).collect::<Vec<_>>(),
		"approvals":installations.iter().map(|r|json!({"reference":{"id":r["entry"]["id"],"version":r["entry"]["version"]},"expected_catalog_revision":0})).collect::<Vec<_>>()});
	let (status, activated) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/marketplace/approval-sets",
		selection,
	)
	.await;
	assert_eq!(status, 200, "{activated}");
	assert_eq!(authorization.catalog("owner").await.unwrap().len(), 4);
	drop(app);
	common::cleanup(f, &url, &database).await;
}
