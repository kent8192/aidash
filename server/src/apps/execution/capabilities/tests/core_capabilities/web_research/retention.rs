//! Retention and Evidence Revocation of Research evidence through the real
//! Harness and HTTP API (ADR 0045, ADR 0046).
use super::*;

impl Research {
	/// Bob, a member of the Tenant whose policy refuses him `research.revoke`
	/// and `retention.update`; returns his token.
	async fn refuse_bob(&mut self) -> String {
		self.c.policy["policies"].as_array_mut().unwrap().push(json!({"id":"bob-keeps-evidence","effect":"deny","subjects":{"ids":["bob"]},"actions":["research.revoke","retention.update"],"resources":{"kinds":["research","retention"]}}));
		let (status, value) = request(
			&self.c.app,
			&self.c.f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":3,"bundle":self.c.policy}),
		)
		.await;
		assert_eq!(status, 200, "{value}");
		let (status, bob) = request(
			&self.c.app,
			&self.c.f.config.api_token,
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"bob"}),
		)
		.await;
		assert_eq!(status, 200, "{bob}");
		bob["token"].as_str().unwrap().to_owned()
	}
	/// A completed Run whose answer cites the page, with the cited reference.
	async fn cited(&self) -> (Run, String) {
		let run = self.delegate().await;
		self.until_completed(run.id).await;
		let messages = self.published("messages", run.id).await;
		let answer = messages
			.iter()
			.filter_map(Value::as_str)
			.find(|content| content.contains("[[cite:"))
			.unwrap_or_else(|| panic!("{messages:?}"));
		(run, evidence_ref(answer).unwrap())
	}
	/// One column of every row of `table` for the Run, as JSON.
	async fn column(&self, table: &str, expression: &str, run: Uuid) -> Vec<Value> {
		use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
		let sql = Query::select()
			.expr(Expr::cust(expression))
			.from(Alias::new(table))
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder);
		sqlx::query_scalar(&sql)
			.fetch_all(self.c.f.store.pool.driver())
			.await
			.unwrap()
	}
}

#[rstest::rstest]
#[tokio::test]
async fn revocation_scrubs_every_stored_copy_and_keeps_an_audit_record(
	#[future]
	#[with(Default::default(), citing_router())]
	research_fixture: Research,
) {
	// Arrange: a completed Run cited the page it read.
	let mut r = research_fixture.await;
	let (run, reference) = r.cited().await;
	let evidence = format!("/api/runs/{}/evidence/{reference}", run.id);
	let (status, _) = request(&r.c.app, &r.c.token, "GET", &evidence, json!(null)).await;
	assert_eq!(status, 200);
	let bob = r.refuse_bob().await;
	let revoke = format!("/api/runs/{}/research-revocation", run.id);

	// Act and assert: Bob may read the Run but not revoke its evidence, and a
	// reason is required.
	let (status, body) = request(
		&r.c.app,
		&bob,
		"POST",
		&revoke,
		json!({"reason":"removal request"}),
	)
	.await;
	assert_eq!(status, 403, "{body}");
	let (status, body) = request(
		&r.c.app,
		&r.c.token,
		"POST",
		&revoke,
		json!({"reason":"  "}),
	)
	.await;
	assert_eq!(status, 400, "{body}");
	let (status, record) = request(
		&r.c.app,
		&r.c.token,
		"POST",
		&revoke,
		json!({"reason":"The rights holder asked us to remove the page."}),
	)
	.await;
	assert_eq!(status, 200, "{record}");
	assert_eq!(record["run_id"], json!(run.id));
	assert_eq!(record["tenant"], "acme");
	assert_eq!(record["revoked_by"], "subject:acme/alice");
	assert_eq!(record["removed"]["snapshots"], 1);
	assert_eq!(record["removed"]["fragments"], 1);
	assert_eq!(record["removed"]["sources"], 1);
	assert!(
		record["removed"]["snapshot_digests"][0]
			.as_str()
			.unwrap()
			.starts_with("sha256:")
	);
	// Revocation is irreversible: a second one returns the first record.
	let (status, again) = request(
		&r.c.app,
		&r.c.f.config.api_token,
		"POST",
		&revoke,
		json!({"reason":"again"}),
	)
	.await;
	assert_eq!(status, 200, "{again}");
	assert_eq!(again, record);

	// The evidence can no longer be read: the uniform absence.
	let (status, body) = request(&r.c.app, &r.c.token, "GET", &evidence, json!(null)).await;
	assert_eq!(status, 404);
	assert_eq!(body, json!({"error":"evidence unavailable"}));
	// Stored copies are tombstones; identities, retrieval times and digests stay.
	assert_eq!(
		r.column("research_snapshots", "jsonb_build_array(lines, text_bytes, title, purge_reason, text_digest LIKE 'sha256:%', fetched_at IS NOT NULL)", run.id).await,
		[json!([[], 0, null, "revocation", true, true])]
	);
	assert_eq!(
		r.column("research_fragments", "to_jsonb(text)", run.id)
			.await,
		[json!("")]
	);
	assert_eq!(
		r.column("research_sources", "jsonb_build_array(title, url)", run.id)
			.await,
		[json!(["", PAGE])]
	);
	for result in r
		.column("research_attempts", "result", run.id)
		.await
		.into_iter()
		.chain(
			r.column("invocations", "result", run.id)
				.await
				.into_iter()
				.filter(|result| result["operation"] == "fetch"),
		) {
		assert_eq!(result["error"]["code"], "evidence_revoked", "{result}");
		assert!(result.get("data").is_none(), "{result}");
	}
	// The stored projection carries the placeholder; the journal keeps the
	// original until the Run is deleted.
	let stored = r.c.f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert!(stored.context.research_revoked);
	assert!(stored.context.events().any(|event| matches!(
		event,
		aidash_domain::context::ContextEvent::Tool { call, result }
			if call.name == "research_fetch"
				&& aidash_domain::context::ContextEvent::is_revoked_result(result)
	)));
	assert!(
		!serde_json::to_string(&stored.context)
			.unwrap()
			.contains("\"evidence_ref\""),
		"no delivered fragment survives in the projection"
	);

	// The audit record is read by revokers and Deployment Administrators only.
	let records = "/api/tenants/acme/research-revocations";
	let (status, list) = request(&r.c.app, &r.c.token, "GET", records, json!(null)).await;
	assert_eq!(status, 200, "{list}");
	assert_eq!(list, json!([record]));
	let (status, list) = request(
		&r.c.app,
		&r.c.f.config.api_token,
		"GET",
		records,
		json!(null),
	)
	.await;
	assert_eq!((status, list.as_array().map(Vec::len)), (200, Some(1)));
	let (status, _) = request(&r.c.app, &bob, "GET", records, json!(null)).await;
	assert_eq!(status, 403);
	r.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn revocation_closes_a_pending_disclosure_and_the_run_continues_without_research(
	#[future] research_fixture: Research,
) {
	// Arrange: the Run waits for a Web disclosure approval.
	let mut r = research_fixture.await;
	let run = r.delegate().await;
	r.require_disclosure_approval().await;
	let approval = r.until_waiting(run.id).await;

	// Act: a Deployment Administrator revokes without reading any content.
	let (status, record) = request(
		&r.c.app,
		&r.c.f.config.api_token,
		"POST",
		&format!("/api/runs/{}/research-revocation", run.id),
		json!({"reason":"The agreement with the source ended."}),
	)
	.await;
	assert_eq!(status, 200, "{record}");
	assert_eq!(record["revoked_by"], "deployment-administrator");

	// Assert: the waiting call's stored invocation, an approval_required
	// result without `limits`, no longer names the query.
	let stored = r
		.column("invocations", "jsonb_build_array(result, input)", run.id)
		.await
		.into_iter()
		.filter(|row| row[0]["contract"] == aidash_domain::research::RESULT_CONTRACT)
		.collect::<Vec<_>>();
	assert!(!stored.is_empty(), "the waiting call was stored");
	for row in stored {
		assert_eq!(row[0]["error"]["code"], "evidence_revoked", "{row}");
		assert_eq!(
			row[1],
			aidash_domain::context::ContextEvent::revoked_arguments(),
			"{row}"
		);
	}

	// The approval is closed, so the Run continues; its next search
	// returns evidence_revoked and nothing is sent.
	let (_, approvals) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		"/api/capabilities/approvals",
		json!(null),
	)
	.await;
	let card = approvals["items"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["id"] == json!(approval))
		.unwrap_or_else(|| panic!("{approvals}"));
	assert_eq!(card["state"], "closed");
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
	loop {
		Harness {
			federation: r.c.f.clone(),
		}
		.worker_once()
		.await
		.unwrap();
		let stored = r.c.f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
		let revoked = serde_json::to_value(stored.context.events().collect::<Vec<_>>())
			.unwrap()
			.as_array()
			.unwrap()
			.iter()
			// The refusal, or the placeholder that replaces it from the next step.
			.any(|event| {
				event["result"]["error"]["code"] == "evidence_revoked"
					|| event["result"]["revoked"] == true
			});
		if revoked {
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"no evidence_revoked result: phase={} error={:?}",
			stored.phase(),
			stored.error
		);
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
	}
	assert_eq!(r.attempts(run.id).await, 0, "nothing was sent");
	r.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_tenant_shortens_its_periods_and_the_sweep_purges_content_already_kept(
	#[future]
	#[with(Default::default(), citing_router())]
	research_fixture: Research,
) {
	// Arrange: a completed Run cited the page it read.
	let mut r = research_fixture.await;
	let (run, reference) = r.cited().await;
	let bob = r.refuse_bob().await;
	let path = "/api/tenants/acme/retention";
	let (status, rules) = request(&r.c.app, &r.c.token, "GET", path, json!(null)).await;
	assert_eq!(status, 200, "{rules}");
	let defaults =
		json!({"document_snapshots":{"after_run_seconds":86400,"max_age_seconds":604800}});
	assert_eq!(rules["deployment"], defaults);
	assert_eq!(rules["tenant_rules"], json!(null));
	assert_eq!(rules["effective"], defaults);

	// Act and assert: a Tenant cannot keep longer than the Deployment, Bob
	// may not change the periods, and Alice shortens them.
	let longer = json!({"rules":{"document_snapshots":{"after_run_seconds":86401,"max_age_seconds":604800}}});
	let (status, body) = request(&r.c.app, &r.c.token, "PUT", path, longer).await;
	assert_eq!(status, 400, "{body}");
	let shorter =
		json!({"rules":{"document_snapshots":{"after_run_seconds":0,"max_age_seconds":3600}}});
	let (status, body) = request(&r.c.app, &bob, "PUT", path, shorter.clone()).await;
	assert_eq!(status, 403, "{body}");
	let (status, rules) = request(&r.c.app, &r.c.token, "PUT", path, shorter.clone()).await;
	assert_eq!(status, 200, "{rules}");
	assert_eq!(rules["effective"], shorter["rules"]);

	// The change applies to content already kept at the next sweep.
	let purged = aidash_server::research::sweep_retention(&r.c.f.store)
		.await
		.unwrap();
	assert_eq!(purged, 1);
	assert_eq!(
		r.column(
			"research_snapshots",
			"jsonb_build_array(lines, purge_reason, text_digest LIKE 'sha256:%')",
			run.id
		)
		.await,
		[json!([[], "retention", true])]
	);
	// The delivered passage still supports the citation; only the snapshot
	// context around it is gone.
	let (status, evidence) = request(
		&r.c.app,
		&r.c.token,
		"GET",
		&format!("/api/runs/{}/evidence/{reference}", run.id),
		json!(null),
	)
	.await;
	assert_eq!(status, 200, "{evidence}");
	assert_eq!(evidence["text"], "Alpha");
	assert_eq!(evidence["context"], json!(null));
	assert_eq!(
		aidash_server::research::sweep_retention(&r.c.f.store)
			.await
			.unwrap(),
		0,
		"a Purged snapshot is purged once"
	);

	// A Deployment Administrator clears the periods; history is newest first
	// and read only by those who may change the periods.
	let (status, rules) = request(
		&r.c.app,
		&r.c.f.config.api_token,
		"PUT",
		path,
		json!({"rules":null}),
	)
	.await;
	assert_eq!(status, 200, "{rules}");
	assert_eq!(rules["effective"], defaults);
	let history = "/api/tenants/acme/retention-history";
	let (status, changes) = request(&r.c.app, &r.c.token, "GET", history, json!(null)).await;
	assert_eq!(status, 200, "{changes}");
	assert_eq!(changes[0]["rules"], json!(null));
	assert_eq!(changes[0]["changed_by"], "deployment-administrator");
	assert_eq!(changes[1]["rules"], shorter["rules"]);
	assert_eq!(changes[1]["changed_by"], "subject:acme/alice");
	let (status, _) = request(&r.c.app, &bob, "GET", history, json!(null)).await;
	assert_eq!(status, 403);
	let (status, _) = request(&r.c.app, &bob, "GET", path, json!(null)).await;
	assert_eq!(status, 200, "every subject of the Tenant reads the periods");
	r.close().await;
}

const REPORT: &str = "https://research-fixture.example/annual-report.pdf";

/// The fixture PDF, whose second page carries no text (ADR 0053).
fn report() -> &'static [u8] {
	aidash_integrations::research::fixture::ANNUAL_REPORT_PDF
}

#[rstest::rstest]
#[tokio::test]
async fn a_retained_original_goes_with_its_snapshot_contents(
	#[future]
	#[with(Default::default(), citing_router_for(REPORT))]
	research_fixture: Research,
) {
	// Arrange: a completed Run read a PDF whose second page carries no text.
	let r = research_fixture.await;
	let (run, _) = r.cited().await;
	assert_eq!(
		r.column(
			"research_snapshots",
			"jsonb_build_array(page_count, pages_without_text, original_retained, original_bytes)",
			run.id
		)
		.await,
		[json!([3, [2], true, report().len()])]
	);
	assert_eq!(
		r.column(
			"research_originals",
			"to_jsonb(octet_length(bytes))",
			run.id
		)
		.await,
		[json!(report().len())]
	);

	// Act: the Tenant shortens its periods and the sweep purges the snapshot.
	let shorter =
		json!({"rules":{"document_snapshots":{"after_run_seconds":0,"max_age_seconds":3600}}});
	let (status, rules) = request(
		&r.c.app,
		&r.c.token,
		"PUT",
		"/api/tenants/acme/retention",
		shorter,
	)
	.await;
	assert_eq!(status, 200, "{rules}");
	assert_eq!(
		aidash_server::research::sweep_retention(&r.c.f.store)
			.await
			.unwrap(),
		1
	);

	// Assert: the original went with the contents; its digest stays.
	assert!(
		r.column("research_originals", "to_jsonb(snapshot_id)", run.id)
			.await
			.is_empty()
	);
	assert_eq!(
		r.column(
			"research_snapshots",
			"jsonb_build_array(original_retained, original_digest LIKE 'sha256:%', purge_reason)",
			run.id
		)
		.await,
		[json!([false, true, "retention"])]
	);
	r.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_evidence_revocation_removes_retained_originals(
	#[future]
	#[with(Default::default(), citing_router_for(REPORT))]
	research_fixture: Research,
) {
	// Arrange: a completed Run read a PDF and retained its original.
	let r = research_fixture.await;
	let (run, _) = r.cited().await;
	assert_eq!(
		r.column(
			"research_originals",
			"to_jsonb(octet_length(bytes))",
			run.id
		)
		.await,
		[json!(report().len())]
	);

	// Act
	let (status, record) = request(
		&r.c.app,
		&r.c.token,
		"POST",
		&format!("/api/runs/{}/research-revocation", run.id),
		json!({"reason":"The rights holder asked us to remove the report."}),
	)
	.await;

	// Assert
	assert_eq!(status, 200, "{record}");
	assert!(
		r.column("research_originals", "to_jsonb(snapshot_id)", run.id)
			.await
			.is_empty()
	);
	assert_eq!(
		r.column(
			"research_snapshots",
			"jsonb_build_array(original_retained, original_digest LIKE 'sha256:%', purge_reason)",
			run.id
		)
		.await,
		[json!([false, true, "revocation"])]
	);
	r.close().await;
}
