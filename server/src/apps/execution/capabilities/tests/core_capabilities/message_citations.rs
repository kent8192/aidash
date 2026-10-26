//! A thread message cites an exact File version and keeps it until released.
use super::*;
use base64::Engine;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};

async fn post(
	c: &CoreFixture,
	workspace: Uuid,
	content: &str,
	citations: &[Value],
) -> (u16, Value) {
	request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/thread-messages"),
		json!({
			"content": content,
			"idempotency_key": Uuid::new_v4(),
			"file_versions": citations,
		}),
	)
	.await
}

async fn history(c: &CoreFixture, workspace: Uuid, token: &str) -> Value {
	let (status, value) = request(
		&c.app,
		token,
		"GET",
		&format!("/api/workspaces/{workspace}/message-history?limit=100"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{value}");
	value
}

fn message<'a>(history: &'a Value, content: &str) -> &'a Value {
	history["messages"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["message"]["content"] == content)
		.unwrap_or_else(|| panic!("message {content:?} not in {history}"))
}

async fn count(c: &CoreFixture, sql: &str) -> i64 {
	sqlx::query_scalar(sql)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap()
}

fn decoded(value: &Value) -> Vec<u8> {
	base64::engine::general_purpose::STANDARD
		.decode(value["data"].as_str().unwrap())
		.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn a_cited_version_survives_an_overwrite_and_downloads_its_old_bytes(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (status, created) = request(
		&c.app,
		&c.token,
		"POST",
		&path_for_patch(run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"report.html":null},"patch":"*** Begin Patch\n*** Add File: report.html\n+<html>old</html>\n*** End Patch"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let file: Uuid = serde_json::from_value(area["manifest"][0]["file_id"].clone()).unwrap();
	let digest = area["manifest"][0]["digest"].clone();
	let workspace = run.workspace_id;
	let cit = json!([{"area_id":area_id,"file_id":file}]);
	let (status, posted) = post(&c, workspace, "Please correct the page", &cit).await;
	assert_eq!(status, 200, "{posted}");
	assert_eq!(posted["citations"].as_array().unwrap().len(), 1);
	assert_eq!(posted["citations"][0]["file_id"], json!(file));
	assert_eq!(posted["citations"][0]["path"], "report.html");
	assert_eq!(posted["citations"][0]["digest"], digest);
	assert_eq!(posted["citations"][0]["available"], true);

	// A later publication replaces the version; the citation keeps the old one.
	let (status, replaced) = request(
		&c.app,
		&c.token,
		"POST",
		&path_for_patch(run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"preconditions":{"report.html":digest},"patch":"*** Begin Patch\n*** Update File: report.html\n@@\n-<html>old</html>\n+<html>new</html>\n*** End Patch"}),
	)
	.await;
	assert_eq!(status, 200, "{replaced}");
	let (_, replaced_area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_ne!(
		replaced_area["manifest"][0]["file_id"],
		json!(file),
		"the overwrite must create a new File version"
	);
	let history = history(&c, workspace, &c.token).await;
	let cited = &message(&history, "Please correct the page")["citations"][0];
	assert_eq!(cited["file_id"], json!(file));
	assert_eq!(cited["digest"], digest);
	assert_eq!(cited["available"], true);
	let (status, chunk) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/working-areas/{area_id}/files/{file}/download"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{chunk}");
	assert_eq!(decoded(&chunk), b"<html>old</html>\n");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_quota_refusal_leaves_no_message_and_no_hold(#[future] capability_fixture: CoreFixture) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (status, created) = request(
		&c.app,
		&c.token,
		"POST",
		&path_for_patch(run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"quota.txt":null},"patch":"*** Begin Patch\n*** Add File: quota.txt\n+quota\n*** End Patch"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let file: Uuid = serde_json::from_value(area["manifest"][0]["file_id"].clone()).unwrap();
	let limit = i64::try_from(c.f.store.capabilities.0.retained_bytes).unwrap();
	let update = Query::update()
		.table(Alias::new("core_quotas"))
		.value_expr(Alias::new("used_bytes"), Expr::value(limit + 1))
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value("acme")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	let (status, refusal) = post(
		&c,
		run.workspace_id,
		"Cite beyond the ceiling",
		&json!([{"area_id":area_id,"file_id":file}]),
	)
	.await;
	assert_eq!(status, 409, "{refusal}");
	assert!(
		refusal["error"]
			.as_str()
			.unwrap_or_default()
			.starts_with("FILE_VERSION_QUOTA_EXCEEDED"),
		"the refusal names the code: {refusal}"
	);
	let query = Query::select()
		.expr(reinhardt::query::Func::count(
			Expr::col(Alias::new("id")).into(),
		))
		.from(Alias::new("messages"))
		.and_where(Expr::col(Alias::new("content")).eq(Expr::value("Cite beyond the ceiling")))
		.to_string(PostgresQueryBuilder);
	assert_eq!(count(&c, &query).await, 0, "no message is left behind");
	let holds = Query::select()
		.expr(reinhardt::query::Func::count(
			Expr::col(Alias::new("id")).into(),
		))
		.from(Alias::new("core_retention_holds"))
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value("acme")))
		.to_string(PostgresQueryBuilder);
	assert_eq!(count(&c, &holds).await, 0, "no hold is left behind");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_viewer_without_download_authority_sees_no_digest(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (status, created) = request(
		&c.app,
		&c.token,
		"POST",
		&path_for_patch(run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"private.html":null},"patch":"*** Begin Patch\n*** Add File: private.html\n+<html>private</html>\n*** End Patch"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let file: Uuid = serde_json::from_value(area["manifest"][0]["file_id"].clone()).unwrap();
	let (status, posted) = post(
		&c,
		run.workspace_id,
		"Read this but do not download it",
		&json!([{"area_id":area_id,"file_id":file}]),
	)
	.await;
	assert_eq!(status, 200, "{posted}");
	let (_, bob) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	let token = bob["token"].as_str().unwrap();
	let history = history(&c, run.workspace_id, token).await;
	let cited = message(&history, "Read this but do not download it")["citations"][0].clone();
	assert_eq!(cited["available"], false);
	assert_eq!(cited["path"], "private.html");
	assert_eq!(cited["reason"], "forbidden");
	assert!(cited.get("digest").is_none(), "{cited}");
	assert!(cited.get("size").is_none(), "{cited}");
	assert!(cited.get("media_type").is_none(), "{cited}");
	assert!(cited.get("revision").is_none(), "{cited}");
	c.close().await;
}
