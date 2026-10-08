//! Two live Nodes exercise canonical native context and both allowance owners.
use super::*;

async fn first_native_context(p: &Pair) -> Value {
	for _ in 0..4 {
		p.step().await;
		if !p.requests.lock().await.is_empty() || p.run().await.control.as_str() == "PAUSED" {
			break;
		}
	}
	let calls = p.requests.lock().await;
	assert_eq!(calls.len(), 1, "{:?}", p.run().await);
	let input: Value =
		serde_json::from_str(calls[0]["messages"][1]["content"].as_str().unwrap()).unwrap();
	for tool in calls[0]["tools"].as_array().unwrap() {
		assert!(
			!["memory_recall", "memory_reflect", "memory_mutate"]
				.contains(&tool["function"]["name"].as_str().unwrap())
		);
	}
	let local_deliveries: i64 = aidash_server::database::native::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::Func::count(
				reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk).into(),
			))
			.from(reinhardt::query::Alias::new("memory_unit_retention"))
			.and_where(reinhardt::query::Expr::col("deliveries").gt(0_i64))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.scalar_one(&p.a.store.pool)
	.await
	.unwrap();
	assert_eq!(
		local_deliveries, 0,
		"federated Delivery cannot increment local Usage"
	);
	input["current"]["semantic_memory"].clone()
}

#[rstest::rstest]
#[tokio::test]
async fn remote_native_provenance_at_exact_admitted_bound_keeps_the_grant_visible(
	#[future(awt)]
	#[with(true, false, false, false, true, (false, false, 2))]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	let fixture = p.native.as_ref().unwrap();
	let workspace = p.a.store.task(p.task).await.unwrap().workspace_id;
	let route = format!("/api/workspaces/{workspace}/memory/units/mutate");
	let mut evidence = vec![];
	for name in ["First exact-bound support", "Second exact-bound support"] {
		let (status, body) = request(
			&p.aa,
			&p.token,
			"POST",
			&route,
			json!({
				"operation_id":Uuid::now_v7(),"provider":fixture["provider"],"bank":fixture["bank"],
				"changes":[{"operation":"add","id":Uuid::now_v7(),"content":{
					"text":name,"kind":"world","learning":"fact","verification":"unverified",
					"occurred":null,"entities":[],"evidence":[],"links":[]
				}}]
			}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		let unit: aidash_domain::memory::Unit = serde_json::from_value(body[0].clone()).unwrap();
		evidence.push(unit.evidence());
	}
	let (status, body) = request(&p.aa, &p.token, "POST", &route, json!({
		"operation_id":Uuid::now_v7(),"provider":fixture["provider"],"bank":fixture["bank"],
		"changes":[{"operation":"correct","id":fixture["private"],"expected_revision":1,"content":{
			"text":"Native private claim with exactly two provenance visits","kind":"world","learning":"fact",
			"verification":"unverified","occurred":null,"entities":[],"evidence":evidence,"links":[]
		}}]
	})).await;
	assert_eq!(status, 200, "{body}");
	let root = &body[0];
	assert_eq!(root["revision"], 2);
	for _ in 0..4 {
		aidash_server::semantic::worker::sweep(&p.a.store)
			.await
			.unwrap();
	}
	let receipt = first_native_context(&p).await;
	let units = receipt["memory"]["banks"][0]["recall"]["units"]
		.as_array()
		.unwrap();
	assert!(
		units
			.iter()
			.any(|unit| unit["id"] == root["id"] && unit["revision"] == 2),
		"{receipt}"
	);
	let recorded: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(Expr::col("unit_id").into()))
			.from(Alias::new("memory_remote_reads"))
			.and_where(Expr::col("grant_id").eq(Expr::value(p.grant)))
			.and_where(Expr::col("unit_id").eq(Expr::value(
				Uuid::parse_str(root["id"].as_str().unwrap()).unwrap(),
			)))
			.and_where(Expr::col("revision").eq(2_i64))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.a.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(
		recorded, 1,
		"the exact-bound root must enter the grant journal"
	);
	let (status, body) = request(
		&p.ba,
		&p.receiver_token,
		"GET",
		&format!("/api/runs/{}", p.admission),
		Value::Null,
	)
	.await;
	assert_eq!(
		status, 200,
		"journal visibility must preserve the admitted provenance bound: {body}"
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn native_receipt_finalization_failure_rolls_back_all_grant_dependencies(
	#[future(awt)]
	#[with(true, false, false, false, true)]
	scoped_pair: Pair,
) {
	use reinhardt::query::{TableConstraint, types::IntoIden};
	let p = scoped_pair;
	let pool = p.a.store.pool.driver();
	// A real database CHECK fails only the final COMPLETED attempt update,
	// after native dependencies have been inserted into the completion transaction.
	let statement = Query::alter_table()
		.table(Alias::new("semantic_remote_attempts"))
		.add_constraint(TableConstraint::Check {
			name: Some(Alias::new("fixture_completion_failure").into_iden()),
			expr: Expr::col("state").ne("COMPLETED"),
		})
		.to_string(PostgresQueryBuilder);
	sqlx::query(&statement).execute(pool).await.unwrap();
	for _ in 0..4 {
		p.step().await;
	}
	assert!(
		p.requests.lock().await.is_empty(),
		"failed finalization must not deliver context to inference"
	);
	let reads: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(Expr::col("unit_id").into()))
			.from(Alias::new("memory_remote_reads"))
			.and_where(Expr::col("grant_id").eq(Expr::value(p.grant)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(pool)
	.await
	.unwrap();
	assert_eq!(
		reads, 0,
		"abandoned native reads roll back with receipt completion"
	);
	let operations: Vec<(String, Option<Value>, Option<String>)> = sqlx::query_as(
		&Query::select()
			.columns(["state", "receipt", "error"].map(Alias::new))
			.from(Alias::new("semantic_remote_operations"))
			.and_where(Expr::col("grant_id").eq(Expr::value(p.grant)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(pool)
	.await
	.unwrap();
	assert!(!operations.is_empty());
	assert!(
		operations
			.iter()
			.all(|(state, receipt, error)| state == "WAITING"
				&& receipt.is_none()
				&& error.as_deref() == Some("unavailable")),
		"{operations:?}"
	);
	let aborted: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(Expr::col("id").into()))
			.from(Alias::new("semantic_remote_attempts"))
			.and_where(Expr::col("state").eq("ABORTED"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(pool)
	.await
	.unwrap();
	assert!(
		aborted > 0,
		"the real completion error was recorded for retry"
	);
	p.close().await;
}

async fn assert_receiver_native_cache_erased(p: &Pair) {
	let receipts: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(
				Expr::col("operation_id").into(),
			))
			.from(Alias::new("semantic_remote_receipts"))
			.and_where(Expr::col("run_id").eq(Expr::value(p.admission)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.b.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(
		receipts, 0,
		"receiver quotation cache rows are physically removed"
	);
	let operations: Vec<(String, Option<Value>)> = sqlx::query_as(
		&Query::select()
			.columns(["state", "receipt"].map(Alias::new))
			.from(Alias::new("semantic_remote_operations"))
			.and_where(Expr::col("admission_id").eq(Expr::value(p.admission)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(p.b.store.pool.driver())
	.await
	.unwrap();
	assert!(
		!operations.is_empty(),
		"permanent operation identities survive cache cleanup"
	);
	assert!(
		operations
			.iter()
			.all(|(state, body)| state == "INVALIDATED" && body.is_none())
	);
}

#[rstest::rstest]
#[tokio::test]
async fn native_remote_context_cache_expiry_retries_physical_failure_after_adapter_recreation(
	#[future(awt)]
	#[with(true, false, false, false, true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	first_native_context(&p).await;
	let pool = p.b.store.pool.driver();
	// Fixture-only DDL: a foreign key injects a real PostgreSQL deletion
	// failure. No production trigger, lease or authority check is disabled.
	sqlx::query("CREATE TABLE fixture_receiver_cleanup_guard (operation_id UUID PRIMARY KEY REFERENCES semantic_remote_receipts(operation_id) ON DELETE RESTRICT)")
		.execute(pool).await.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("fixture_receiver_cleanup_guard"))
			.columns([Alias::new("operation_id")])
			.from_subquery(
				Query::select()
					.column(Alias::new("operation_id"))
					.from(Alias::new("semantic_remote_receipts"))
					.and_where(Expr::col("run_id").eq(Expr::value(p.admission)))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool)
	.await
	.unwrap();
	let past = chrono::Utc::now() - chrono::Duration::seconds(1);
	sqlx::query(
		&Query::update()
			.table(Alias::new("memory_receiver_caches"))
			.value(Alias::new("expires_at"), past)
			.value(Alias::new("next_attempt"), past)
			.and_where(Expr::col("run_id").eq(Expr::value(p.admission)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool)
	.await
	.unwrap();
	aidash_server::semantic::worker::sweep(&p.b.store)
		.await
		.unwrap();
	let failed: (String, bool, i32, Option<String>) = sqlx::query_as(
		&Query::select()
			.columns(["state", "invalidated", "attempts", "last_error"].map(Alias::new))
			.from(Alias::new("memory_receiver_caches"))
			.and_where(Expr::col("run_id").eq(Expr::value(p.admission)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(pool)
	.await
	.unwrap();
	assert_eq!(
		failed,
		(
			"pending".into(),
			true,
			1,
			Some("receiver_storage_unavailable".into())
		)
	);
	assert_eq!(p.requests.lock().await.len(), 1);
	let (status, metadata) = request(
		&p.ba,
		&p.receiver_token,
		"GET",
		&format!("/api/runs/{}/management", p.admission),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{metadata}");
	assert_eq!(metadata["memory_cleanup"]["state"], "pending", "{metadata}");
	assert_eq!(metadata["memory_cleanup"]["attempts"], 1);
	assert_eq!(metadata["memory_cleanup"]["invalidated"], true);
	assert!(metadata.get("context").is_none() && metadata.get("pending").is_none());
	sqlx::query("DROP TABLE fixture_receiver_cleanup_guard")
		.execute(pool)
		.await
		.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("memory_receiver_caches"))
			.value(Alias::new("next_attempt"), past)
			.and_where(Expr::col("run_id").eq(Expr::value(p.admission)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool)
	.await
	.unwrap();
	let reopened = aidash_server::store::Store::from_pool(
		p.b.store.pool.driver().clone(),
		p.b.store.node_id.clone(),
	)
	.await
	.unwrap();
	aidash_server::semantic::worker::sweep(&reopened)
		.await
		.unwrap();
	assert_receiver_native_cache_erased(&p).await;
	let (status, metadata) = request(
		&p.ba,
		&p.receiver_token,
		"GET",
		&format!("/api/runs/{}/management", p.admission),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{metadata}");
	assert_eq!(metadata["memory_cleanup"]["state"], "purged", "{metadata}");
	p.step().await;
	assert_eq!(p.run().await.control.as_str(), "PAUSED");
	assert_eq!(
		p.requests.lock().await.len(),
		1,
		"expired caches cannot feed another inference"
	);
	let (status, body) = request(
		&p.ba,
		&p.receiver_token,
		"GET",
		&format!("/api/runs/{}/semantic", p.admission),
		Value::Null,
	)
	.await;
	assert_eq!(status, 409, "{body}");
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn ordinary_native_remote_context_uses_home_units_and_deletion_stops_inference(
	#[future(awt)]
	#[with(true, false, false, false, true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	let fixture = p.native.as_ref().unwrap();
	let receipt = first_native_context(&p).await;
	assert!(
		receipt["result"]["matches"].as_array().unwrap().is_empty(),
		"native Agents must not union unconfigured legacy memory sources"
	);
	let banks = receipt["memory"]["banks"].as_array().unwrap();
	assert_eq!(banks.len(), 2, "{receipt}");
	let ids = ["private", "shared"];
	for (bank, key) in banks.iter().zip(ids) {
		assert_eq!(bank["bank"]["home"], p.a.config.node_id);
		assert_eq!(bank["recall"]["status"], "ready", "{receipt}");
		let units = bank["recall"]["units"].as_array().unwrap();
		assert_eq!(units.len(), 1);
		assert_eq!(units[0]["id"], fixture[key]);
		assert_eq!(units[0]["bank"], bank["bank"]);
		assert_eq!(units[0]["content"]["verification"], "unverified");
	}
	assert_eq!(
		receipt["binding"]["native"]["participant"]["bank"],
		fixture["bank"]
	);
	let recorded: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("memory_remote_reads"))
			.and_where(Expr::col("grant_id").eq(Expr::value(p.grant)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.a.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(recorded, 2);
	let workspace = p.a.store.task(p.task).await.unwrap().workspace_id;
	let (status, index) = request(
		&p.aa,
		&p.token,
		"GET",
		&format!("/api/workspaces/{workspace}/semantic/index"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{index}");
	let (status, body) = request(
		&p.aa,
		&p.a.config.api_token,
		"POST",
		&format!("/api/workspaces/{workspace}/semantic/index"),
		json!({"expected_revision":index["revision"],"spec":index["spec"]}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		&p.ba,
		&p.receiver_token,
		"GET",
		&format!("/api/runs/{}", p.admission),
		Value::Null,
	)
	.await;
	assert_eq!(
		status, 200,
		"an index rebuild must preserve canonical native context: {body}"
	);
	let (status, body) = request(&p.aa, &p.token, "POST", &format!("/api/workspaces/{workspace}/memory/units/mutate"), json!({"operation_id":Uuid::new_v4(), "provider":fixture["provider"], "bank":banks[1]["bank"], "changes":[{"operation":"delete", "id":fixture["shared"], "expected_revision":1}]})).await;
	assert_eq!(status, 200, "{body}");
	p.step().await;
	assert_eq!(
		p.run().await.control.as_str(),
		"PAUSED",
		"{:?}",
		p.run().await
	);
	assert_eq!(
		p.requests.lock().await.len(),
		1,
		"withdrawn memory must not reach another inference"
	);
	assert_receiver_native_cache_erased(&p).await;
	let (status, body) = request(
		&p.ba,
		&p.receiver_token,
		"GET",
		&format!("/api/runs/{}/semantic", p.admission),
		Value::Null,
	)
	.await;
	assert_eq!(status, 403, "{body}");
	assert!(!body.to_string().contains("Native shared claim"));
	let (status, management) = request(
		&p.ba,
		&p.receiver_token,
		"GET",
		&format!("/api/runs/{}/management", p.admission),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{management}");
	assert_eq!(management["semantic_reason"], "invalidated", "{management}");
	sqlx::query(
		&Query::update()
			.table(Alias::new("memory_purge_jobs"))
			.value(Alias::new("purge_after"), chrono::Utc::now())
			.value(Alias::new("next_attempt"), chrono::Utc::now())
			.and_where(Expr::col("unit_id").eq(Expr::value(
				serde_json::from_value::<Uuid>(fixture["shared"].clone()).unwrap(),
			)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(p.a.store.pool.driver())
	.await
	.unwrap();
	aidash_server::semantic::worker::sweep(&p.a.store)
		.await
		.unwrap();
	let retained: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("semantic_remote_operations"))
			.and_where(Expr::col("grant_id").eq(Expr::value(p.grant)))
			.and_where(Expr::col("receipt").is_not_null())
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.a.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(
		retained, 0,
		"physical cleanup also erases Home receipt quotation bodies"
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn generated_native_remote_context_has_fresh_home_identity_and_committed_origin_usage(
	#[future(awt)]
	#[with(true, true, false, false, true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	*p.semantic.as_ref().unwrap().reservations.lock().await = Some(ReservationCheck {
		pools: vec![
			p.a.store.pool.driver().clone(),
			p.b.store.pool.driver().clone(),
		],
		dispatcher: p.a.store.pool.driver().clone(),
		grant: p.grant,
		admission: p.admission,
		purpose: "retrieval",
	});
	let receipt = first_native_context(&p).await;
	let banks = receipt["memory"]["banks"].as_array().unwrap();
	assert_eq!(banks.len(), 2);
	assert_ne!(
		banks[0]["bank"]["participant"],
		p.native.as_ref().unwrap()["selection"]["participant"]
	);
	assert_eq!(banks[0]["recall"]["status"], "empty", "{receipt}");
	assert_eq!(banks[1]["recall"]["status"], "ready", "{receipt}");
	assert_eq!(
		banks[1]["recall"]["units"][0]["id"],
		p.native.as_ref().unwrap()["shared"]
	);
	for node in [&p.a, &p.b] {
		let usage: Vec<(String, i64, Option<i64>)> = sqlx::query_as(
			&Query::select()
				.columns(["state", "reserved_tokens", "reported_tokens"].map(Alias::new))
				.from(Alias::new("generation_remote_usage"))
				.and_where(Expr::col("grant_id").eq(Expr::value(p.grant)))
				.and_where(Expr::col("purpose").eq("memory"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(node.store.pool.driver())
		.await
		.unwrap();
		assert!(
			!usage.is_empty(),
			"native recall must debit {}",
			node.config.node_id
		);
		assert!(
			usage
				.iter()
				.all(|(state, reserved, reported)| state == "SETTLED"
					&& *reserved > 0
					&& *reported == Some(1)),
			"{usage:?}"
		);
	}
	let workspace = p.a.store.task(p.task).await.unwrap().workspace_id;
	let (status,body) = request(&p.aa,&p.token,"POST",&format!("/api/workspaces/{workspace}/memory/units/mutate"),
		json!({"operation_id":Uuid::now_v7(),"provider":p.native.as_ref().unwrap()["provider"],"bank":banks[1]["bank"],
			"changes":[{"operation":"delete","id":p.native.as_ref().unwrap()["shared"],"expected_revision":1}]})).await;
	assert_eq!(status, 200, "{body}");
	p.step().await;
	assert_eq!(p.run().await.control.as_str(), "PAUSED");
	assert_eq!(
		p.requests.lock().await.len(),
		1,
		"a generated remote Run stops after source withdrawal"
	);
	assert_receiver_native_cache_erased(&p).await;
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn native_remote_run_reads_use_the_pinned_provenance_limit_above_1024(
	#[future(awt)]
	#[with(true, false, false, false, true, (true, false, 32))]
	scoped_pair: Pair,
) {
	use aidash_domain::memory::{
		Bank, Change, Content, Evidence, Kind, Learning, Mutation, Unit, Verification,
	};
	let p = scoped_pair;
	first_native_context(&p).await;
	// This case isolates the declared provenance bound. Coverage makes its
	// deliberate 1596-visit graph slower than the normal HTTP/peer deadlines.
	let deadline = std::time::Duration::from_secs(180);
	p.aa.context
		.set_singleton(reinhardt::di::KeyedFactoryOutput::<
			reinhardt::di::SelfKey<aidash_server::http::Protection>,
			aidash_server::http::Protection,
		>::new(aidash_server::http::Protection::new(
			aidash_server::http::Settings {
				timeout: deadline,
				..Default::default()
			},
		)));
	let fixture = p.native.as_ref().unwrap();
	let mut bank: Bank = serde_json::from_value(fixture["bank"].clone()).unwrap();
	bank.participant = None;
	let provider: aidash_domain::registry::EntityRef =
		serde_json::from_value(fixture["provider"].clone()).unwrap();
	let original = Evidence::Unit {
		bank: bank.clone(),
		id: serde_json::from_value(fixture["shared"].clone()).unwrap(),
		revision: 1,
	};
	let mut support = vec![original];
	let mut last = Uuid::nil();
	// A Fibonacci DAG stays within the 16-unit bank while requiring 1596 visits.
	for _ in 0..14 {
		last = Uuid::now_v7();
		let mutation = Mutation {
			operation_id: Uuid::now_v7(),
			provider: provider.clone(),
			bank: bank.clone(),
			changes: vec![Change::Add {
				id: last,
				content: Content {
					text: "Current bounded Run provenance".into(),
					kind: Kind::World,
					learning: Learning::Fact,
					verification: Verification::Unverified,
					mental_model: None,
					occurred: None,
					entities: vec![],
					evidence: support.clone(),
					links: vec![],
				},
			}],
		};
		let (status, body) = request(
			&p.aa,
			&p.token,
			"POST",
			&format!("/api/workspaces/{}/memory/units/mutate", bank.workspace),
			serde_json::to_value(mutation).unwrap(),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		let saved: Vec<Unit> = serde_json::from_value(body).unwrap();
		support.push(saved[0].evidence());
		if support.len() > 2 {
			support.remove(0);
		}
	}
	// The fixture journal records the exact admitted unit consumed by the Run.
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("memory_remote_reads"))
			.columns(["grant_id", "unit_id", "revision"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(p.grant))
					.expr(Expr::value(last))
					.expr(Expr::value(1_i64))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(p.a.store.pool.driver())
	.await
	.unwrap();
	// Exercise the Home peer verifier directly: a deliberate >1024-visit DAG
	// isolates the policy boundary from the receiver's ten-second transport cap.
	let response = reqwest::Client::new()
		.post(format!(
			"{}/federation/v0.1/scoped/dependencies/verify",
			p.a.config.endpoint
		))
		.timeout(deadline)
		.bearer_auth(std::env::var("AIDASH_SECRET_TEST_PEER").unwrap())
		.header("x-aidash-node", &p.b.config.node_id)
		.header("x-aidash-protocol", "0.2")
		.json(&json!({"tenant":"acme","subject":"alice","reference":{
			"kind":"grant","node_id":p.a.config.node_id,"execution_node":p.b.config.node_id,
			"grant_id":p.grant,"admission_id":p.admission,
		}}))
		.send()
		.await
		.unwrap();
	let status = response.status();
	let body: Value = response.json().await.unwrap();
	assert_eq!(status.as_u16(), 200, "{body}");
	assert_eq!(
		body["visible"], true,
		"a valid graph above 1024 must remain disclosable at the Home boundary: {body}"
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn native_remote_journal_validates_admitted_reads_above_1024_and_checks_later_pages(
	#[future(awt)]
	#[with(true, false, false, false, true, (false, true, 32))]
	scoped_pair: Pair,
) {
	use aidash_domain::memory::{Bank, Change, Content, Mutation, Unit};
	use aidash_server::apps::knowledge::services::native_memory as memory;
	let p = scoped_pair;
	// Isolate the lifetime read bound from the ordinary HTTP/peer deadlines,
	// including under coverage instrumentation, as in the large graph case.
	let deadline = std::time::Duration::from_secs(180);
	p.aa.context
		.set_singleton(reinhardt::di::KeyedFactoryOutput::<
			reinhardt::di::SelfKey<aidash_server::http::Protection>,
			aidash_server::http::Protection,
		>::new(aidash_server::http::Protection::new(
			aidash_server::http::Settings {
				timeout: deadline,
				..Default::default()
			},
		)));
	let actor = aidash_server::authorization::Authorization {
		pool: p.a.store.pool.clone(),
	}
	.authenticate(&p.token)
	.await
	.unwrap();
	let fixture = p.native.as_ref().unwrap();
	let bank: Bank = serde_json::from_value(fixture["bank"].clone()).unwrap();
	let provider: aidash_domain::registry::EntityRef =
		serde_json::from_value(fixture["provider"].clone()).unwrap();
	let selected: Vec<_> = memory::list(
		&p.a.store,
		&actor,
		memory::ReadBank {
			provider: provider.clone(),
			bank: bank.clone(),
		},
	)
	.await
	.unwrap()
	.into_iter()
	.filter(|unit| (1000..2025).contains(&unit.id.as_u128()))
	.collect();
	assert_eq!(selected.len(), 1025);
	// Model a grant's previous Runs through the same durable dependency schema.
	let mut tx = aidash_server::database::native::begin(&p.a.store.pool)
		.await
		.unwrap();
	let mut insert = Query::insert();
	insert
		.into_table(Alias::new("memory_remote_reads"))
		.columns(["grant_id", "unit_id", "revision"].map(Alias::new));
	for unit in &selected {
		insert.values_panic([
			reinhardt::query::IntoValue::into_value(p.grant),
			reinhardt::query::IntoValue::into_value(unit.id),
			reinhardt::query::IntoValue::into_value(unit.revision),
		]);
	}
	aidash_server::database::native::query(&insert.to_string(PostgresQueryBuilder))
		.execute(&mut *tx)
		.await
		.unwrap();
	tx.commit().await.unwrap();

	let count: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(Expr::col("unit_id").into()))
			.from(Alias::new("memory_remote_reads"))
			.and_where(Expr::col("grant_id").eq(Expr::value(p.grant)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.a.store.pool.driver())
	.await
	.unwrap();
	assert!(
		count > 1024,
		"the policy admits more dependencies than the old fixed ceiling"
	);
	let verify = || {
		reqwest::Client::new()
			.post(format!(
				"{}/federation/v0.1/scoped/dependencies/verify",
				p.a.config.endpoint
			))
			.timeout(deadline)
			.bearer_auth(std::env::var("AIDASH_SECRET_TEST_PEER").unwrap())
			.header("x-aidash-node", &p.b.config.node_id)
			.header("x-aidash-protocol", "0.2")
			.json(&json!({"tenant":"acme","subject":"alice","reference":{
            "kind":"grant","node_id":p.a.config.node_id,"execution_node":p.b.config.node_id,
            "grant_id":p.grant,"admission_id":p.admission}}))
	};
	let body: Value = verify().send().await.unwrap().json().await.unwrap();
	assert_eq!(body["visible"], true, "{body}");
	let last: &Unit = selected.last().unwrap();
	memory::mutate(
		&p.a.store,
		&actor,
		Mutation {
			operation_id: Uuid::now_v7(),
			provider,
			bank,
			changes: vec![Change::Correct {
				id: last.id,
				expected_revision: last.revision,
				content: Content {
					text: "Corrected late-page dependency".into(),
					..last.content.clone()
				},
			}],
		},
	)
	.await
	.unwrap();
	let body: Value = verify().send().await.unwrap().json().await.unwrap();
	assert_eq!(
		body["visible"], false,
		"a corrected later-page revision invalidates the grant: {body}"
	);
	p.close().await;
}
