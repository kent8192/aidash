use crate::endpoint::{EndpointFixture, assert_json, assert_json_rejection, endpoint, workspace};
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::apps::knowledge::models::{
	SemanticCollection, SemanticEntry, SemanticHistory, SemanticIndexe, SemanticPoint,
};
use aidash_server::semantic::worker;
use chrono::{Duration, Utc};
use reinhardt::db::orm::Model;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use uuid::Uuid;

#[fixture]
fn index_spec() -> Value {
	json!({"embedding":{"provider":"openai","endpoint":"http://localhost:9","model":"fixture","model_version":"1","dimensions":8},"vector":{"provider":"postgres","endpoint":"local","credential_env":null},"enabled":true,"auto_context":false,"max_sources":10,"max_results":5,"max_result_tokens":128,"max_input_bytes":256})
}

#[rstest]
#[tokio::test]
async fn memory_byte_limit_and_index_revisions_are_enforced_over_http(
	#[future] endpoint: EndpointFixture,
	mut index_spec: Value,
) {
	// Arrange
	let app = endpoint.await;
	let member = crate::endpoint::tenant_subject(&app, "member").await;
	let workspace = workspace(&member, "Semantic limits").await;
	let base = format!(
		"/api/workspaces/{}/semantic",
		workspace["id"].as_str().unwrap()
	);
	let index_path = format!("{base}/index");
	let entries_path = format!("{base}/entries");
	assert_json(
		app.deployment_administrator
			.post(
				&index_path,
				&json!({"expected_revision":0,"spec":index_spec}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	// Act
	let oversized = member.post(&entries_path, &json!({"key":"memory","expected_revision":0,"source":{"kind":"memory","text":"あ".repeat(86)},"metadata":{}}), "json").await.unwrap();
	// Assert
	assert_json(oversized, 400);
	assert_eq!(
		assert_json(member.get(&entries_path).await.unwrap(), 200),
		json!([])
	);
	// Act
	let saved = assert_json(member.post(&entries_path, &json!({"key":"memory","expected_revision":0,"source":{"kind":"memory","text":"あ".repeat(80)},"metadata":{}}), "json").await.unwrap(), 200);
	index_spec["max_input_bytes"] = json!(128);
	let reduced = app
		.deployment_administrator
		.post(
			&index_path,
			&json!({"expected_revision":1,"spec":index_spec}),
			"json",
		)
		.await
		.unwrap();
	index_spec["max_sources"] = json!(0);
	let invalid_spec = app
		.deployment_administrator
		.post(
			&index_path,
			&json!({"expected_revision":1,"spec":index_spec}),
			"json",
		)
		.await
		.unwrap();
	// Assert
	assert_json(reduced, 409);
	assert_json(invalid_spec, 400);
	let unchanged = assert_json(member.get(&index_path).await.unwrap(), 200);
	assert_eq!(unchanged["revision"], 1);
	assert_eq!(unchanged["spec"]["max_input_bytes"], 256);
	let entries = assert_json(member.get(&entries_path).await.unwrap(), 200);
	assert_eq!(entries[0]["id"], saved["id"]);
}

#[rstest]
#[tokio::test]
async fn index_contract_validation_precedes_persistence_and_accepts_boundary_values(
	#[future] endpoint: EndpointFixture,
	index_spec: Value,
) {
	let app = endpoint.await;
	let member = crate::endpoint::tenant_subject(&app, "member").await;
	let workspace = workspace(&member, "Index contracts").await;
	let path = format!(
		"/api/workspaces/{}/semantic/index",
		workspace["id"].as_str().unwrap()
	);
	let mut invalid = vec![(json!({}), 422), (json!([]), 422), (Value::Null, 422)];
	for field in index_spec.as_object().unwrap().keys() {
		let mut missing = index_spec.clone();
		missing.as_object_mut().unwrap().remove(field);
		invalid.push((missing, 422));
		let mut null = index_spec.clone();
		null[field] = Value::Null;
		invalid.push((null, 422));
	}
	for (field, value, status) in [
		("/unexpected", json!(true), 422),
		("/max_results", json!(1.5), 422),
		("/max_sources", json!(-1), 422),
		("/max_input_bytes", json!("8192"), 422),
		("/enabled", json!("true"), 422),
		("/embedding", json!({}), 422),
		("/vector", json!({}), 422),
		("/embedding/provider", json!("unsupported"), 400),
		("/vector/provider", json!("pgvector"), 400),
		(
			"/embedding/endpoint",
			json!("http://user:password@localhost/v1"),
			400,
		),
		(
			"/embedding/endpoint",
			json!("http://localhost/v1?token=x"),
			400,
		),
		("/vector/endpoint", json!("file:///tmp/vectors"), 400),
		(
			"/vector/endpoint",
			json!("http://999.999.999.999/vectors"),
			400,
		),
		// ADR 0006: embeddings cannot name an environment key at all, so the
		// field is rejected while decoding, before validation.
		(
			"/embedding/credential_env",
			json!("AIDASH_SECRET_EMBEDDING"),
			422,
		),
		("/embedding/model", json!(" \t\u{2003}"), 400),
		("/embedding/model", json!("m".repeat(257)), 400),
		("/embedding/model_version", json!("v".repeat(129)), 400),
	] {
		let mut wrong = index_spec.clone();
		let (parent, key) = field.rsplit_once('/').unwrap();
		wrong.pointer_mut(parent).unwrap()[key] = value;
		invalid.push((wrong, status));
	}
	let bounds = [
		("max_sources", 1, 1024),
		("max_results", 1, 20),
		("max_result_tokens", 128, 32768),
		("max_input_bytes", 128, 32768),
	];
	for (field, min, max) in bounds {
		for value in [min - 1, max + 1] {
			let mut wrong = index_spec.clone();
			wrong[field] = json!(value);
			invalid.push((wrong, 400));
		}
	}
	for section in ["embedding", "vector"] {
		for field in index_spec[section]
			.as_object()
			.unwrap()
			.keys()
			.filter(|key| *key != "credential_env")
		{
			let mut missing = index_spec.clone();
			missing[section].as_object_mut().unwrap().remove(field);
			invalid.push((missing, 422));
			let mut null = index_spec.clone();
			null[section][field] = Value::Null;
			invalid.push((null, 422));
		}
		for (field, value) in [("unexpected", json!(true)), ("credential_env", json!(7))] {
			let mut wrong = index_spec.clone();
			wrong[section][field] = value;
			invalid.push((wrong, 422));
		}
	}
	for dimensions in [0, 8193] {
		let mut wrong = index_spec.clone();
		wrong["embedding"]["dimensions"] = json!(dimensions);
		invalid.push((wrong, 400));
	}
	for (spec, status) in invalid {
		let response = app
			.deployment_administrator
			.post(&path, &json!({"expected_revision":0,"spec":spec}), "json")
			.await
			.unwrap();
		if status == 422 {
			assert_json_rejection(response, 422);
		} else {
			assert_json(response, 400);
		}
	}
	let indexes = app
		.database
		.fixture(async |tx| Ok(SemanticIndexe::objects().all().all_with_db(tx).await?))
		.await;
	assert!(indexes.is_empty());
	let mut revision = 0;
	for (field, min, max) in bounds {
		for value in [min, max] {
			let mut boundary = index_spec.clone();
			boundary[field] = json!(value);
			let saved = assert_json(
				app.deployment_administrator
					.post(
						&path,
						&json!({"expected_revision":revision,"spec":boundary}),
						"json",
					)
					.await
					.unwrap(),
				200,
			);
			revision += 1;
			assert_eq!(saved["revision"], revision);
			assert_eq!(saved["spec"][field], value);
		}
	}
	for dimensions in [1, 8192] {
		let mut boundary = index_spec.clone();
		boundary["embedding"]["dimensions"] = json!(dimensions);
		let saved = assert_json(
			app.deployment_administrator
				.post(
					&path,
					&json!({"expected_revision":revision,"spec":boundary}),
					"json",
				)
				.await
				.unwrap(),
			200,
		);
		revision += 1;
		assert_eq!(saved["revision"], revision);
	}
}

#[rstest]
#[tokio::test]
async fn malformed_sources_and_client_authority_cannot_create_semantic_entries(
	#[future] endpoint: EndpointFixture,
	index_spec: Value,
) {
	let app = endpoint.await;
	let member = crate::endpoint::tenant_subject(&app, "member").await;
	let workspace = workspace(&member, "Source contracts").await;
	let base = format!(
		"/api/workspaces/{}/semantic",
		workspace["id"].as_str().unwrap()
	);
	assert_json(
		app.deployment_administrator
			.post(
				&format!("{base}/index"),
				&json!({"expected_revision":0,"spec":index_spec}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let entries = format!("{base}/entries");
	for (source, status) in [
		(json!({}), 422),
		(json!([]), 422),
		(Value::Null, 422),
		(json!({"kind":"unknown"}), 422),
		(json!({"kind":"memory"}), 422),
		(json!({"kind":"memory","text":7}), 422),
		(json!({"kind":"memory","text":" \t\u{2003}"}), 400),
		(
			json!({"kind":"memory","text":"text","unexpected":true}),
			422,
		),
		(json!({"kind":"artifact","id":"invalid"}), 422),
		(json!({"kind":"message","id":7}), 422),
		(json!({"kind":"artifact"}), 422),
		(
			json!({"kind":"message","id":uuid::Uuid::new_v4(),"text":"extra"}),
			422,
		),
	] {
		let response = member
			.post(
				&entries,
				&json!({"key":"memory","expected_revision":0,"source":source,"metadata":{}}),
				"json",
			)
			.await
			.unwrap();
		if status == 422 {
			assert_json_rejection(response, 422);
		} else {
			assert_json(response, 400);
		}
	}
	assert_json_rejection(member.post(&entries, &json!({"key":"memory","expected_revision":0,"source":{"kind":"memory","text":"safe"},"metadata":{},"authority":{"subject":"forged"}}), "json").await.unwrap(), 422);
	assert_eq!(
		assert_json(member.get(&entries).await.unwrap(), 200),
		json!([])
	);
	let mut db = app.database.lease.handle();
	assert!(
		SemanticEntry::objects()
			.all()
			.all_with_db(&mut db)
			.await
			.unwrap()
			.is_empty()
	);
	let saved = assert_json(member.post(&entries, &json!({"key":"memory","expected_revision":0,"source":{"kind":"memory","text":"Memory"},"metadata":{}}), "json").await.unwrap(), 200);
	let record = SemanticEntry::objects()
		.get(serde_json::from_value(saved["id"].clone()).unwrap())
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(record.authority.0["subject"], "member");
	assert!(record.authority.0["credential"].is_string());
}

#[rstest]
#[tokio::test]
async fn index_generation_replay_and_cleanup_retries_keep_durable_tombstones(
	#[future] endpoint: EndpointFixture,
	mut index_spec: Value,
) {
	use aidash_server::database::native;
	use reinhardt::query::{
		Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder,
		types::{ColumnDef, ColumnType},
	};
	// Arrange two generations, including a superseded point in the active collection.
	let app = endpoint.await;
	let member = crate::endpoint::tenant_subject(&app, "member").await;
	let workspace = workspace(&member, "Generation cleanup").await;
	let base = format!(
		"/api/workspaces/{}/semantic",
		workspace["id"].as_str().unwrap()
	);
	let index_path = format!("{base}/index");
	let entries_path = format!("{base}/entries");
	let cleanup_path = format!("{base}/cleanup");
	let first = assert_json(
		app.deployment_administrator
			.post(
				&index_path,
				&json!({"expected_revision":0,"spec":index_spec}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let original = assert_json(member.post(&entries_path,
		&json!({"key":"memory","expected_revision":0,"source":{"kind":"memory","text":"first"},"metadata":{}}), "json").await.unwrap(), 200);
	index_spec["embedding"]["model_version"] = json!("2");
	let second = assert_json(
		app.deployment_administrator
			.post(
				&index_path,
				&json!({"expected_revision":1,"spec":index_spec}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let replay = assert_json(
		app.deployment_administrator
			.post(
				&index_path,
				&json!({"expected_revision":1,"spec":index_spec}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_eq!(second, replay);
	assert_eq!(second["revision"], 2);
	assert_ne!(first["collection"], second["collection"]);
	let reconfigured = assert_json(member.get(&entries_path).await.unwrap(), 200);
	assert_eq!(reconfigured[0]["revision"], 1);
	assert_eq!(reconfigured[0]["index_revision"], 2);
	assert_ne!(original["point_id"], reconfigured[0]["point_id"]);
	let point_id: Uuid = serde_json::from_value(reconfigured[0]["point_id"].clone()).unwrap();
	let vector: aidash_server::semantic::VectorConfig =
		serde_json::from_value(index_spec["vector"].clone()).unwrap();
	let store = &app.runtime.store;
	let first_collection = first["collection"].as_str().unwrap();
	let second_collection = second["collection"].as_str().unwrap();
	aidash_server::semantic::backend::ensure_collection(store, &vector, first_collection, 8)
		.await
		.unwrap();
	aidash_server::semantic::backend::ensure_collection(store, &vector, second_collection, 8)
		.await
		.unwrap();
	let workspace_tenant: String =
		sqlx::query_scalar("SELECT tenant FROM workspaces WHERE id = $1")
			.bind(workspace["id"].as_str().unwrap().parse::<Uuid>().unwrap())
			.fetch_one(store.pool.fixture_driver())
			.await
			.unwrap();
	aidash_server::semantic::backend::upsert(
		store,
		&vector,
		second_collection,
		point_id,
		&[1., 0., 0., 0., 0., 0., 0., 0.],
		json!({"workspace_id":workspace["id"],"tenant":workspace_tenant}),
	)
	.await
	.unwrap();
	// Real FK holds inject deletion failures in the current PostgreSQL adapter.
	// Only the table owner can create the holds; the server role holds DML.
	let owner: native::Pool = crate::native_database::owner_pool(store.pool.fixture_driver())
		.await
		.into();
	let mut tx = owner.begin(&TEST_FIXTURE).await.unwrap();
	for (table, column, parent, parent_column, kind, value) in [
		(
			"fixture_point_hold",
			"point_id",
			"semantic_vectors",
			"id",
			ColumnType::Uuid,
			Expr::value(point_id),
		),
		(
			"fixture_collection_hold",
			"collection",
			"semantic_vector_collections",
			"collection",
			ColumnType::Text,
			Expr::value(first_collection),
		),
	] {
		native::query(
			&Query::create_table()
				.table(Alias::new(table))
				.col(ColumnDef::new(column).column_type(kind))
				.foreign_key([column], Alias::new(parent), [parent_column], None, None)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
		native::query(
			&Query::insert()
				.into_table(Alias::new(table))
				.columns([Alias::new(column)])
				.from_subquery(Query::select().expr(value).to_owned())
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
	}
	tx.commit().await.unwrap();
	let updated = assert_json(member.post(&entries_path,
		&json!({"key":"memory","expected_revision":1,"source":{"kind":"memory","text":"second"},"metadata":{}}), "json").await.unwrap(), 200);
	let pending = json!({"points":{"retired":1,"pending":1,"failed":0},"collections":{"retired":1,"pending":1,"failed":0}});
	assert_eq!(
		assert_json(
			app.deployment_administrator
				.get(&cleanup_path)
				.await
				.unwrap(),
			200
		),
		pending
	);

	// Act: failed physical deletion remains pending and cannot immediately retry.
	worker::sweep(&app.runtime.store).await.unwrap();
	let failed = json!({"points":{"retired":1,"pending":1,"failed":1},"collections":{"retired":1,"pending":1,"failed":1}});
	assert_eq!(
		assert_json(
			app.deployment_administrator
				.get(&cleanup_path)
				.await
				.unwrap(),
			200
		),
		failed
	);
	let mut db = app.database.lease.handle();
	let first_point_retry = SemanticPoint::objects()
		.get(point_id)
		.get_with_db(&mut db)
		.await
		.unwrap()
		.next_attempt;
	let first_collection_retry = SemanticCollection::objects()
		.get(first_collection.to_owned())
		.get_with_db(&mut db)
		.await
		.unwrap()
		.next_attempt;
	worker::sweep(&app.runtime.store).await.unwrap();
	assert_eq!(
		assert_json(
			app.deployment_administrator
				.get(&cleanup_path)
				.await
				.unwrap(),
			200
		),
		failed
	);
	assert_eq!(
		SemanticPoint::objects()
			.get(point_id)
			.get_with_db(&mut db)
			.await
			.unwrap()
			.next_attempt,
		first_point_retry
	);
	assert_eq!(
		SemanticCollection::objects()
			.get(first_collection.to_owned())
			.get_with_db(&mut db)
			.await
			.unwrap()
			.next_attempt,
		first_collection_retry
	);

	// Advance only the persisted retry deadline, then acknowledge absent identities.
	let mut released = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	for table in ["fixture_point_hold", "fixture_collection_hold"] {
		native::query(
			&Query::delete()
				.from_table(Alias::new(table))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *released)
		.await
		.unwrap();
	}
	released.commit().await.unwrap();
	let mut point = SemanticPoint::objects()
		.get(point_id)
		.get_with_db(&mut db)
		.await
		.unwrap();
	let mut collection = SemanticCollection::objects()
		.get(first["collection"].as_str().unwrap().to_owned())
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert!(point.next_attempt > Utc::now());
	assert!(collection.next_attempt > Utc::now());
	point.next_attempt = Utc::now() - Duration::seconds(1);
	collection.next_attempt = point.next_attempt;
	let mut tx = app.database.begin_fixture().await;
	SemanticPoint::objects()
		.save_with_executor(&mut *tx, &point)
		.await
		.unwrap();
	SemanticCollection::objects()
		.save_with_executor(&mut *tx, &collection)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	worker::sweep(&app.runtime.store).await.unwrap();

	// Assert: replay did not duplicate a generation, and acknowledgements keep tombstones.
	let cleaned = json!({"points":{"retired":1,"pending":0,"failed":0},"collections":{"retired":1,"pending":0,"failed":0}});
	assert_eq!(
		assert_json(
			app.deployment_administrator
				.get(&cleanup_path)
				.await
				.unwrap(),
			200
		),
		cleaned
	);
	let points = SemanticPoint::objects()
		.all()
		.all_with_db(&mut db)
		.await
		.unwrap();
	let collections = SemanticCollection::objects()
		.all()
		.all_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(points.len(), 3);
	assert_eq!(collections.len(), 2);
	let point = points.iter().find(|point| point.id == point_id).unwrap();
	assert!(point.retired && point.cleaned_at.is_some() && point.last_error.is_none());
	let collection = collections
		.iter()
		.find(|collection| collection.retired)
		.unwrap();
	assert!(collection.cleaned_at.is_some() && collection.last_error.is_none());
	let active: Vec<_> = points.iter().filter(|point| !point.retired).collect();
	assert_eq!(active.len(), 1);
	assert_eq!(json!(active[0].id), updated["point_id"]);
	let history = SemanticHistory::objects()
		.all()
		.all_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(
		history
			.iter()
			.filter(|event| event.entry_id.is_none())
			.count(),
		2
	);
}

/// The Ordered Retrieval Key pins this digest: an ordinary entry insert or
/// edit must change it although the index revision stays the same, while an
/// unchanged corpus or another Workspace's entries leave it as it was.
#[rstest]
#[tokio::test]
async fn corpus_digest_changes_with_entries_but_not_with_the_index_revision(
	#[future] endpoint: EndpointFixture,
	index_spec: Value,
) {
	// Arrange
	let app = endpoint.await;
	let member = crate::endpoint::tenant_subject(&app, "member").await;
	let store = &app.runtime.store;
	let mut ids = vec![];
	for title in ["Corpus", "Other corpus"] {
		let workspace = workspace(&member, title).await;
		let id: Uuid = serde_json::from_value(workspace["id"].clone()).unwrap();
		assert_json(
			app.deployment_administrator
				.post(
					&format!("/api/workspaces/{id}/semantic/index"),
					&json!({"expected_revision":0,"spec":index_spec}),
					"json",
				)
				.await
				.unwrap(),
			200,
		);
		ids.push(id);
	}
	let (workspace, other) = (ids[0], ids[1]);
	let entries = format!("/api/workspaces/{workspace}/semantic/entries");
	let index = format!("/api/workspaces/{workspace}/semantic/index");
	let participant = Some(Uuid::new_v4());
	let digest = || {
		aidash_server::semantic::services::corpus_digest(
			store,
			&TEST_FIXTURE,
			workspace,
			participant,
		)
	};
	let unbound =
		|| aidash_server::semantic::services::corpus_digest(store, &TEST_FIXTURE, workspace, None);
	let empty = digest().await.unwrap();
	// Act
	assert_json(member.post(&entries, &json!({"key":"memory","expected_revision":0,"source":{"kind":"memory","text":"first"},"metadata":{}}), "json").await.unwrap(), 200);
	let inserted = digest().await.unwrap();
	assert_json(member.post(&entries, &json!({"key":"memory","expected_revision":1,"source":{"kind":"memory","text":"second"},"metadata":{}}), "json").await.unwrap(), 200);
	let edited = digest().await.unwrap();
	assert_json(member.post(&format!("/api/workspaces/{other}/semantic/entries"), &json!({"key":"memory","expected_revision":0,"source":{"kind":"memory","text":"elsewhere"},"metadata":{}}), "json").await.unwrap(), 200);
	let unchanged = digest().await.unwrap();
	// A memory unit mutation bumps only its bank's revision.
	use aidash_server::database::native;
	use reinhardt::query::{
		Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	};
	let bank = Uuid::now_v7();
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_banks"))
			.columns(["id", "home", "tenant", "workspace_id", "revision"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(bank))
					.expr(Expr::value(store.deployment.as_str()))
					.expr(Expr::cust(format!(
						"(SELECT tenant FROM workspaces WHERE id = '{workspace}')"
					)))
					.expr(Expr::value(workspace))
					.expr(Expr::value(1_i64))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	let shared_bank = digest().await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_banks"))
			.value_expr(Alias::new("revision"), Expr::col("revision").add(1_i64))
			.and_where(Expr::col("id").eq(Expr::value(bank)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	let mutated_bank = digest().await.unwrap();
	// Assert
	assert_ne!(inserted, empty);
	assert_ne!(edited, inserted);
	assert_eq!(unchanged, edited);
	assert_ne!(shared_bank, unchanged);
	assert_ne!(mutated_bank, shared_bank);
	// Without a memory participant no bank is recalled, so none is keyed.
	assert_eq!(unbound().await.unwrap(), edited);
	assert_eq!(
		assert_json(member.get(&index).await.unwrap(), 200)["revision"],
		1
	);
}

/// Adds a deny of `action` for `subject` to the `endpoint` Tenant's policy.
async fn deny(app: &EndpointFixture, subject: &str, action: &str) {
	let policy = assert_json(
		app.deployment_administrator
			.get("/api/authorization/endpoint")
			.await
			.unwrap(),
		200,
	);
	let mut bundle = policy["bundle"].clone();
	bundle["policies"].as_array_mut().unwrap().push(json!({
		"id":format!("deny-{action}"),"effect":"deny","subjects":{"ids":[subject]},
		"actions":[action],"resources":{"kinds":["*"]}
	}));
	assert_json(
		app.deployment_administrator
			.post(
				"/api/authorization/endpoint",
				&json!({"expected_revision":policy["revision"],"bundle":bundle}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
}

#[rstest]
#[tokio::test]
async fn members_configure_their_index_under_semantic_index_manage(
	#[future] endpoint: EndpointFixture,
	index_spec: Value,
) {
	let app = endpoint.await;
	let member = crate::endpoint::tenant_subject(&app, "member").await;
	let workspace = workspace(&member, "Configured by a member").await;
	let id = workspace["id"].as_str().unwrap();
	let index = format!("/api/workspaces/{id}/semantic/index");
	assert_json(
		member
			.post(
				&index,
				&json!({"expected_revision":0,"spec":index_spec}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_json(
		member
			.get(&format!("/api/workspaces/{id}/semantic/cleanup"))
			.await
			.unwrap(),
		200,
	);
	deny(&app, "member", "semantic.index.manage").await;
	let denials = || async {
		sqlx::query_scalar::<_, i64>(
			"SELECT count(*) FROM authorization_decisions WHERE subject = 'member' \
			 AND action = 'semantic.index.manage' AND NOT (decision->>'allowed')::boolean",
		)
		.fetch_one(app.runtime.store.pool.fixture_driver())
		.await
		.unwrap()
	};
	let before = denials().await;
	assert_json(
		member
			.post(
				&index,
				&json!({"expected_revision":1,"spec":index_spec}),
				"json",
			)
			.await
			.unwrap(),
		403,
	);
	assert_eq!(denials().await, before + 1, "the refusal is audited");
}

#[rstest]
#[case::private("http://10.0.0.7:8080/v1")]
#[case::loopback("http://127.0.0.1:9/v1")]
#[case::metadata("http://169.254.169.254/v1")]
#[tokio::test]
async fn a_keyless_endpoint_at_an_unpermitted_private_address_is_refused_when_configured(
	#[future] endpoint: EndpointFixture,
	mut index_spec: Value,
	#[case] address: &str,
) {
	let app = endpoint.await;
	let member = crate::endpoint::tenant_subject(&app, "member").await;
	let workspace = workspace(&member, "Private embeddings").await;
	index_spec["embedding"]["endpoint"] = json!(address);
	let body = json!({"expected_revision":0,"spec":index_spec});
	let path = format!(
		"/api/workspaces/{}/semantic/index",
		workspace["id"].as_str().unwrap()
	);
	for client in [&member, &app.deployment_administrator] {
		assert_json(client.post(&path, &body, "json").await.unwrap(), 400);
	}
}
