//! Provider timeouts survive registry HTTP requests and persisted local overrides.
#[path = "../../execution/tests/support/endpoint.rs"]
mod endpoint_fixtures;
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;

use aidash_server::apps::registry::models::states::DefinitionKind;
use aidash_server::apps::registry::models::{Definition, Installation};
use aidash_server::registry::{Entry, ModelConfig};
use endpoint_fixtures::{EndpointFixture, assert_json, endpoint};
use reinhardt::core::exception::DatabaseErrorKind;
use reinhardt::db::orm::Model;
use rstest::{fixture, rstest};
use serde_json::{Value, json};

#[fixture]
fn timeout_model() -> Value {
	json!({"id":"timeout-model","version":"1.0.0","kind":"model",
        "name":{"en":"Timeout fixture"},"description":{"en":"Persistence test"},
        "config":{"provider":"openrouter","model_id":"vendor/model",
            "endpoint":"http://127.0.0.1:1/v1","credential_env":null,
            "context_window":32768,"max_output_tokens":4096,
            "modalities":["text"],"cost":{}}})
}

#[fixture]
fn supported_timeouts() -> Vec<Option<Value>> {
	vec![
		None,
		Some(Value::Null),
		Some(json!(1)),
		Some(json!(900)),
		Some(json!(1200)),
		Some(json!(u32::MAX)),
	]
}

#[fixture]
fn invalid_configs() -> Vec<Value> {
	let mut configurations: Vec<_> = [
		json!(0),
		json!(-1),
		json!(1.5),
		json!("900"),
		json!(true),
		json!([]),
		json!({}),
		json!(4294967296_u64),
	]
	.into_iter()
	.map(|value| json!({"request_timeout_secs":value}))
	.collect();
	configurations.push(json!({"request_timeout_sec":900}));
	configurations
}

#[rstest]
#[tokio::test]
async fn configured_timeouts_round_trip_through_the_registry_api(
	#[future] endpoint: EndpointFixture,
	timeout_model: Value,
	supported_timeouts: Vec<Option<Value>>,
) {
	// Arrange
	let app = endpoint.await;
	let mut connection = app.database.lease.handle();
	for (index, timeout) in supported_timeouts.into_iter().enumerate() {
		let id = format!("registered-{index}");
		let mut entry = timeout_model.clone();
		entry["id"] = json!(id);
		if let Some(value) = &timeout {
			entry["config"]["request_timeout_secs"] = value.clone();
		}
		// Act
		let created = assert_json(
			app.operator
				.post("/api/registry", &entry, "json")
				.await
				.unwrap(),
			200,
		);
		let found = assert_json(
			app.operator
				.get(&format!("/api/registry/{id}/1.0.0"))
				.await
				.unwrap(),
			200,
		);
		let saved = Definition::objects()
			.filter(Definition::field_id().eq(id))
			.get_with_db(&mut connection)
			.await
			.unwrap();
		// Assert
		assert_eq!(created["config"], entry["config"]);
		assert_eq!(found["config"], entry["config"]);
		assert_eq!(saved.metadata.0["config"], entry["config"]);
		let expected_seconds = timeout.as_ref().and_then(Value::as_u64).unwrap_or(900);
		let config: ModelConfig =
			serde_json::from_value(saved.metadata.0["config"].clone()).unwrap();
		assert_eq!(
			config.request_timeout().unwrap().as_secs(),
			expected_seconds
		);
	}
}

#[rstest]
#[tokio::test]
async fn persisted_timeout_overrides_are_applied_by_registry_reads(
	#[future] endpoint: EndpointFixture,
	timeout_model: Value,
	supported_timeouts: Vec<Option<Value>>,
) {
	// Arrange
	let app = endpoint.await;
	let mut connection = app.database.lease.handle();
	for (index, timeout) in supported_timeouts.into_iter().enumerate() {
		let id = format!("overridden-{index}");
		let mut entry = timeout_model.clone();
		entry["id"] = json!(id);
		// Use a different base value to make an applied override observable.
		entry["config"]["request_timeout_secs"] = json!(45);
		assert_json(
			app.operator
				.post("/api/registry", &entry, "json")
				.await
				.unwrap(),
			200,
		);
		let _definition = Definition::objects()
			.filter(Definition::field_id().eq(id.clone()))
			.get_with_db(&mut connection)
			.await
			.unwrap();
		let mut overrides = json!({});
		if let Some(value) = &timeout {
			overrides["request_timeout_secs"] = value.clone();
		}
		let installation = Installation::build()
			.id(&id)
			.version("1.0.0")
			.digest("timeout-fixture")
			.config(overrides.clone().into())
			.finish();
		// Act
		Installation::objects()
			.create_with_conn(&mut connection, &installation)
			.await
			.unwrap();
		let found = assert_json(
			app.operator
				.get(&format!("/api/registry/{id}/1.0.0"))
				.await
				.unwrap(),
			200,
		);
		let saved = Installation::objects()
			.filter(Installation::field_id().eq(id))
			.get_with_db(&mut connection)
			.await
			.unwrap();
		// Assert
		assert_eq!(saved.config.0, overrides);
		let expected = match &timeout {
			None => 45,
			Some(value) => value.as_u64().unwrap_or(900),
		};
		let config: ModelConfig = serde_json::from_value(found["config"].clone()).unwrap();
		assert_eq!(config.request_timeout().unwrap().as_secs(), expected);
		assert_eq!(
			found["config"]["request_timeout_secs"],
			timeout.unwrap_or(json!(45))
		);
	}
}

#[rstest]
#[tokio::test]
async fn invalid_timeouts_and_unknown_fields_leave_no_registration(
	#[future] endpoint: EndpointFixture,
	timeout_model: Value,
	invalid_configs: Vec<Value>,
) {
	// Arrange
	let app = endpoint.await;
	let seeded_definitions = app
		.runtime
		.registry
		.list(&Default::default())
		.await
		.unwrap();
	let mut connection = app.database.lease.handle();
	for (index, invalid) in invalid_configs.into_iter().enumerate() {
		let mut value = timeout_model.clone();
		value["id"] = json!(format!("invalid-{index}"));
		value["config"]
			.as_object_mut()
			.unwrap()
			.extend(invalid.as_object().unwrap().clone());
		let entry: Entry = serde_json::from_value(value).unwrap();
		// Act
		let response = app
			.operator
			.post("/api/registry", &entry, "json")
			.await
			.unwrap();
		let draft = Definition::build()
			.id(&entry.id)
			.version(&entry.version)
			.kind(DefinitionKind::Model)
			.metadata(json!(entry).into())
			.finish();
		let error = Definition::objects()
			.create_with_conn(&mut connection, &draft)
			.await
			.unwrap_err();
		// Assert
		assert_json(response, 400);
		let database = error.database_error().expect("database constraint error");
		assert_eq!(
			database.kind(),
			DatabaseErrorKind::CheckViolation,
			"{error}"
		);
		assert_eq!(
			database.constraint(),
			Some("registry_model_config"),
			"{error}"
		);
	}
	assert_eq!(
		app.runtime
			.registry
			.list(&Default::default())
			.await
			.unwrap(),
		seeded_definitions
	);
	assert!(
		Installation::objects()
			.all()
			.all_with_db(&mut connection)
			.await
			.unwrap()
			.is_empty()
	);
}

fn assert_constraint(error: reinhardt::core::exception::Error, constraint: &str) {
	let database = error.database_error().expect("database constraint error");
	assert_eq!(
		database.kind(),
		DatabaseErrorKind::CheckViolation,
		"{error}"
	);
	assert_eq!(database.code(), Some("23514"), "{error}");
	assert_eq!(database.constraint(), Some(constraint), "{error}");
}

#[rstest]
#[tokio::test]
async fn database_validates_registered_and_overridden_timeouts(
	#[future] endpoint: EndpointFixture,
	timeout_model: Value,
	supported_timeouts: Vec<Option<Value>>,
	invalid_configs: Vec<Value>,
) {
	// Arrange: keep an existing installation so invalid updates must retain it.
	let app = endpoint.await;
	let mut connection = app.database.lease.handle();
	assert_json(
		app.operator
			.post("/api/registry", &timeout_model, "json")
			.await
			.unwrap(),
		200,
	);
	let id = timeout_model["id"].as_str().unwrap();
	Installation::objects()
		.create_with_conn(
			&mut connection,
			&Installation::build()
				.id(id)
				.version("1.0.0")
				.digest("timeout-fixture")
				.config(json!({}).into())
				.finish(),
		)
		.await
		.unwrap();
	for timeout in supported_timeouts.into_iter().flatten() {
		let config = json!({"request_timeout_secs": timeout});
		// Act / Assert: accepted updates remain effective at the registry boundary.
		let updated = Installation::objects()
			.filter(Installation::field_id().eq(id.to_owned()))
			.update_fields_with_conn(
				&mut connection,
				[(
					Installation::field_config(),
					reinhardt::db::orm::Json(config),
				)],
			)
			.await
			.unwrap();
		assert_eq!(updated, 1);
		let found = assert_json(
			app.operator
				.get(&format!("/api/registry/{id}/1.0.0"))
				.await
				.unwrap(),
			200,
		);
		assert_eq!(found["config"]["request_timeout_secs"], timeout);
	}
	let retained = Installation::objects()
		.filter(Installation::field_id().eq(id.to_owned()))
		.get_with_db(&mut connection)
		.await
		.unwrap()
		.config;
	for (index, config) in invalid_configs.into_iter().enumerate() {
		let mut entry = timeout_model.clone();
		entry["id"] = json!(format!("raw-invalid-{index}"));
		entry["config"]
			.as_object_mut()
			.unwrap()
			.extend(config.as_object().unwrap().clone());
		let definition = Definition::build()
			.id(entry["id"].as_str().unwrap())
			.version("1.0.0")
			.kind(DefinitionKind::Model)
			.metadata(entry.into())
			.finish();
		assert_constraint(
			Definition::objects()
				.create_with_conn(&mut connection, &definition)
				.await
				.unwrap_err(),
			"registry_model_config",
		);
		let error = Installation::objects()
			.filter(Installation::field_id().eq(id.to_owned()))
			.update_fields_with_conn(
				&mut connection,
				[(
					Installation::field_config(),
					reinhardt::db::orm::Json(config),
				)],
			)
			.await
			.unwrap_err();
		assert_constraint(error, "installations_config");
		assert_eq!(
			Installation::objects()
				.filter(Installation::field_id().eq(id.to_owned()))
				.get_with_db(&mut connection)
				.await
				.unwrap()
				.config,
			retained
		);
	}
}

#[rstest]
#[tokio::test]
async fn database_validates_media_route_members_for_models_and_installations(
	#[future] endpoint: EndpointFixture,
	timeout_model: Value,
) {
	// Arrange
	let app = endpoint.await;
	let mut connection = app.database.lease.handle();
	assert_json(
		app.operator
			.post("/api/registry", &timeout_model, "json")
			.await
			.unwrap(),
		200,
	);
	let id = timeout_model["id"].as_str().unwrap();
	let route = json!({"tag":"fixture/verified", "formats":["image/png"],
		"source":"fixture verification", "verified_at":"2026-09-28T12:00:00Z",
		"expires_at":"2026-10-28T12:00:00Z"});
	let mut valid = timeout_model.clone();
	valid["id"] = json!("valid-route");
	valid["config"]["media_routes"] = json!([route]);
	Definition::objects()
		.create_with_conn(
			&mut connection,
			&Definition::build()
				.id("valid-route")
				.version("1.0.0")
				.kind(DefinitionKind::Model)
				.metadata(valid.into())
				.finish(),
		)
		.await
		.unwrap();
	let config = json!({"media_routes":[route]});
	Installation::objects()
		.create_with_conn(
			&mut connection,
			&Installation::build()
				.id(id)
				.version("1.0.0")
				.digest("timeout-fixture")
				.config(config.clone().into())
				.finish(),
		)
		.await
		.unwrap();
	for (index, routes) in [
		json!([1]),
		json!([{"tag":"fixture/verified"}]),
		json!([{"tag":"fixture/verified", "formats":[1], "source":"fixture", "verified_at":"2026-09-28T12:00:00Z", "expires_at":"2026-10-28T12:00:00Z"}]),
		json!([{"tag":"fixture/verified", "formats":["image/png"], "source":"fixture", "verified_at":"2026-09-28T12:00:00Z", "expires_at":"2026-10-28T12:00:00Z", "unexpected":true}]),
		json!([{"tag":"fixture/verified", "formats":["image/png"], "source":"fixture", "verified_at":"not a timestamp", "expires_at":"2026-10-28T12:00:00Z"}]),
	].into_iter().enumerate() {
		// Act / Assert
		let mut entry = timeout_model.clone();
		entry["id"] = json!(format!("invalid-route-{index}"));
		entry["config"]["media_routes"] = routes.clone();
		let definition = Definition::build().id(entry["id"].as_str().unwrap()).version("1.0.0")
			.kind(DefinitionKind::Model).metadata(entry.into()).finish();
		assert_constraint(Definition::objects().create_with_conn(&mut connection, &definition)
			.await.unwrap_err(), "registry_model_config");
		assert_constraint(Installation::objects().filter(Installation::field_id().eq(id.to_owned()))
			.update_fields_with_conn(&mut connection, [(Installation::field_config(), reinhardt::db::orm::Json(json!({"media_routes":routes})))])
			.await.unwrap_err(), "installations_config");
		assert_eq!(Installation::objects().filter(Installation::field_id().eq(id.to_owned()))
			.get_with_db(&mut connection).await.unwrap().config.0, config);
	}
}
