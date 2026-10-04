use crate::endpoint::{EndpointFixture, assert_json, assert_json_rejection, endpoint, workspace};
use aidash_server::apps::knowledge::models::{
	SemanticCollection, SemanticEntry, SemanticHistory, SemanticIndexe, SemanticPoint,
};
use aidash_server::semantic::worker;
use chrono::{Duration, Utc};
use reinhardt::db::orm::Model;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use uuid::Uuid;

#[path = "support/cleanup_backend.rs"]
mod cleanup_fixture;
use cleanup_fixture::{CleanupBackend, cleanup_backend};

#[fixture]
fn index_spec() -> Value {
	json!({"embedding":{"provider":"openai","endpoint":"http://localhost:9","credential_env":null,"model":"fixture","model_version":"1","dimensions":8},"vector":{"provider":"qdrant","endpoint":"http://localhost:9","credential_env":null},"enabled":true,"auto_context":false,"max_sources":10,"max_results":5,"max_result_tokens":128,"max_input_bytes":256})
}

#[rstest]
#[tokio::test]
async fn memory_byte_limit_and_index_revisions_are_enforced_over_http(
	#[future] endpoint: EndpointFixture,
	mut index_spec: Value,
) {
	// Arrange
	let app = endpoint.await;
	let workspace = workspace(&app.operator, "Semantic limits").await;
	let base = format!(
		"/api/workspaces/{}/semantic",
		workspace["id"].as_str().unwrap()
	);
	let index_path = format!("{base}/index");
	let entries_path = format!("{base}/entries");
	assert_json(
		app.operator
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
	let oversized = app.operator.post(&entries_path, &json!({"key":"memory","expected_revision":0,"source":{"kind":"memory","text":"あ".repeat(86)},"metadata":{}}), "json").await.unwrap();
	// Assert
	assert_json(oversized, 400);
	assert_eq!(
		assert_json(app.operator.get(&entries_path).await.unwrap(), 200),
		json!([])
	);
	// Act
	let saved = assert_json(app.operator.post(&entries_path, &json!({"key":"memory","expected_revision":0,"source":{"kind":"memory","text":"あ".repeat(80)},"metadata":{}}), "json").await.unwrap(), 200);
	index_spec["max_input_bytes"] = json!(128);
	let reduced = app
		.operator
		.post(
			&index_path,
			&json!({"expected_revision":1,"spec":index_spec}),
			"json",
		)
		.await
		.unwrap();
	index_spec["max_sources"] = json!(0);
	let invalid_spec = app
		.operator
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
	let unchanged = assert_json(app.operator.get(&index_path).await.unwrap(), 200);
	assert_eq!(unchanged["revision"], 1);
	assert_eq!(unchanged["spec"]["max_input_bytes"], 256);
	let entries = assert_json(app.operator.get(&entries_path).await.unwrap(), 200);
	assert_eq!(entries[0]["id"], saved["id"]);
}

#[rstest]
#[tokio::test]
async fn index_contract_validation_precedes_persistence_and_accepts_boundary_values(
	#[future] endpoint: EndpointFixture,
	index_spec: Value,
) {
	let app = endpoint.await;
	let workspace = workspace(&app.operator, "Index contracts").await;
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
		("/embedding/credential_env", json!("OPENAI_API_KEY"), 400),
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
			.operator
			.post(&path, &json!({"expected_revision":0,"spec":spec}), "json")
			.await
			.unwrap();
		if status == 422 {
			assert_json_rejection(response, 422);
		} else {
			assert_json(response, 400);
		}
	}
	let mut db = app.database.lease.handle();
	assert!(
		SemanticIndexe::objects()
			.all()
			.all_with_db(&mut db)
			.await
			.unwrap()
			.is_empty()
	);
	let mut revision = 0;
	for (field, min, max) in bounds {
		for value in [min, max] {
			let mut boundary = index_spec.clone();
			boundary[field] = json!(value);
			let saved = assert_json(
				app.operator
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
			app.operator
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
	let workspace = workspace(&app.operator, "Source contracts").await;
	let base = format!(
		"/api/workspaces/{}/semantic",
		workspace["id"].as_str().unwrap()
	);
	assert_json(
		app.operator
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
		let response = app
			.operator
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
	assert_json_rejection(app.operator.post(&entries, &json!({"key":"memory","expected_revision":0,"source":{"kind":"memory","text":"safe"},"metadata":{},"authority":{"subject":"forged"}}), "json").await.unwrap(), 422);
	assert_eq!(
		assert_json(app.operator.get(&entries).await.unwrap(), 200),
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
	let saved = assert_json(app.operator.post(&entries, &json!({"key":"memory","expected_revision":0,"source":{"kind":"memory","text":"Memory"},"metadata":{}}), "json").await.unwrap(), 200);
	let record = SemanticEntry::objects()
		.get(serde_json::from_value(saved["id"].clone()).unwrap())
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(record.authority.0["subject"], "operator");
	assert_eq!(record.authority.0["credential"], Value::Null);
}

#[rstest]
#[tokio::test]
async fn index_generation_replay_and_cleanup_retries_keep_durable_tombstones(
	#[future] endpoint: EndpointFixture,
	#[future] cleanup_backend: CleanupBackend,
	mut index_spec: Value,
) {
	// Arrange two generations, including a superseded point in the active collection.
	let app = endpoint.await;
	let backend = cleanup_backend.await;
	index_spec["vector"]["endpoint"] = json!(backend.server.url);
	index_spec["embedding"]["endpoint"] = json!(backend.server.url);
	let workspace = workspace(&app.operator, "Generation cleanup").await;
	let base = format!(
		"/api/workspaces/{}/semantic",
		workspace["id"].as_str().unwrap()
	);
	let index_path = format!("{base}/index");
	let entries_path = format!("{base}/entries");
	let cleanup_path = format!("{base}/cleanup");
	let first = assert_json(
		app.operator
			.post(
				&index_path,
				&json!({"expected_revision":0,"spec":index_spec}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let original = assert_json(app.operator.post(&entries_path,
		&json!({"key":"memory","expected_revision":0,"source":{"kind":"memory","text":"first"},"metadata":{}}), "json").await.unwrap(), 200);
	index_spec["embedding"]["model_version"] = json!("2");
	let second = assert_json(
		app.operator
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
		app.operator
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
	let reconfigured = assert_json(app.operator.get(&entries_path).await.unwrap(), 200);
	assert_eq!(reconfigured[0]["revision"], 1);
	assert_eq!(reconfigured[0]["index_revision"], 2);
	assert_ne!(original["point_id"], reconfigured[0]["point_id"]);
	let updated = assert_json(app.operator.post(&entries_path,
		&json!({"key":"memory","expected_revision":1,"source":{"kind":"memory","text":"second"},"metadata":{}}), "json").await.unwrap(), 200);
	let pending = json!({"points":{"retired":1,"pending":1,"failed":0},"collections":{"retired":1,"pending":1,"failed":0}});
	assert_eq!(
		assert_json(app.operator.get(&cleanup_path).await.unwrap(), 200),
		pending
	);

	// Act: failed physical deletion remains pending and cannot immediately retry.
	worker::sweep(&app.runtime.store).await.unwrap();
	let failed = json!({"points":{"retired":1,"pending":1,"failed":1},"collections":{"retired":1,"pending":1,"failed":1}});
	assert_eq!(
		assert_json(app.operator.get(&cleanup_path).await.unwrap(), 200),
		failed
	);
	let expected_requests = vec![
		format!(
			"point:{}:{}",
			second["collection"].as_str().unwrap(),
			reconfigured[0]["point_id"].as_str().unwrap()
		),
		format!("collection:{}", first["collection"].as_str().unwrap()),
	];
	assert_eq!(*backend.state.requests.lock().unwrap(), expected_requests);
	worker::sweep(&app.runtime.store).await.unwrap();
	assert_eq!(*backend.state.requests.lock().unwrap(), expected_requests);

	// Advance only the persisted retry deadline, then acknowledge absent identities.
	backend.state.available.store(true, Ordering::SeqCst);
	let mut db = app.database.lease.handle();
	let point_id: Uuid = serde_json::from_value(reconfigured[0]["point_id"].clone()).unwrap();
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
	let mut tx = app.database.connection.begin().await.unwrap();
	SemanticPoint::objects()
		.save_with_executor(tx.as_mut(), &point)
		.await
		.unwrap();
	SemanticCollection::objects()
		.save_with_executor(tx.as_mut(), &collection)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	worker::sweep(&app.runtime.store).await.unwrap();

	// Assert: replay did not duplicate a generation, and acknowledgements keep tombstones.
	let cleaned = json!({"points":{"retired":1,"pending":0,"failed":0},"collections":{"retired":1,"pending":0,"failed":0}});
	assert_eq!(
		assert_json(app.operator.get(&cleanup_path).await.unwrap(), 200),
		cleaned
	);
	assert_eq!(
		*backend.state.requests.lock().unwrap(),
		[expected_requests.clone(), expected_requests].concat()
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
