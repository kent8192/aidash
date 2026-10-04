//! Validate typed request contracts before definitions, packages or events persist.
use crate::endpoint::{EndpointFixture, assert_json, assert_json_rejection, endpoint};
use crate::registry::skill;
use aidash_server::apps::execution::models::Event;
use aidash_server::apps::registry::models::{Definition, Installation, Package};
use reinhardt::db::orm::Model;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[fixture]
fn model_entry() -> Value {
	json!({"id":"validation-model","version":"1.0.0","kind":"model","name":{"en":"Model"},"description":{"en":"Validation fixture"},"config":{"provider":"openrouter","model_id":"fixture","endpoint":"http://localhost:9/v1","context_window":32768,"max_output_tokens":4096,"modalities":["text"],"cost":{}}})
}

#[rstest]
#[tokio::test]
async fn malformed_registry_contracts_leave_no_definitions_or_events(
	#[future] endpoint: EndpointFixture,
	skill: Value,
) {
	let app = endpoint.await;
	let mut invalid = vec![];
	for schema in [
		json!({"type":7}),
		json!({"items":7}),
		json!({"properties":{"count":{"required":"name"}}}),
	] {
		let mut entry = skill.clone();
		entry["schema"] = schema;
		invalid.push((entry, 400));
	}
	for field in ["capabilities", "tags", "languages", "skills"] {
		for value in [
			json!(7),
			Value::Null,
			json!(true),
			json!({}),
			json!([]),
			json!(["nested"]),
		] {
			let mut entry = skill.clone();
			entry[field] = json!(["valid", value]);
			invalid.push((entry, 422));
		}
	}
	for field in ["name", "description"] {
		for value in [json!(7), Value::Null, json!(true), json!({}), json!([])] {
			let mut entry = skill.clone();
			entry[field] = json!({"en":"Valid", "ja":value});
			invalid.push((entry, 422));
		}
	}
	let mut unknown = skill.clone();
	unknown["unexpected"] = json!(true);
	invalid.push((unknown, 422));
	for (entry, status) in invalid {
		let response = app
			.operator
			.post("/api/registry", &entry, "json")
			.await
			.unwrap();
		if status == 422 {
			assert_json_rejection(response, 422);
		} else {
			assert_json(response, 400);
		}
	}
	assert!(
		Definition::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
	assert!(
		Event::objects()
			.filter(Event::field_kind().eq("registry.registered".to_owned()))
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
	let mut valid = skill;
	for field in ["capabilities", "tags", "languages", "skills"] {
		valid[field] = json!(["one", "two"]);
	}
	let stored = assert_json(
		app.operator
			.post("/api/registry", &valid, "json")
			.await
			.unwrap(),
		200,
	);
	let _: aidash_server::registry::Entry = serde_json::from_value(stored.clone()).unwrap();
	assert_eq!(
		assert_json(
			app.operator
				.get("/api/registry/endpoint-skill/1.0.0")
				.await
				.unwrap(),
			200
		),
		stored
	);
}

#[rstest]
#[tokio::test]
async fn registry_nonblank_fields_follow_rust_unicode_whitespace(
	#[future] endpoint: EndpointFixture,
	model_entry: Value,
	skill: Value,
) {
	let app = endpoint.await;
	let whitespace: Vec<_> = (0..=0x10ffff)
		.filter_map(char::from_u32)
		.filter(|c| c.is_whitespace())
		.collect();
	let mut blanks: Vec<String> = whitespace.iter().map(char::to_string).collect();
	blanks.extend([whitespace.iter().collect(), String::new()]);
	for blank in blanks {
		for (base, field) in [
			(&model_entry, "model_id"),
			(&model_entry, "endpoint"),
			(&skill, "instructions"),
		] {
			let mut entry = base.clone();
			entry["config"][field] = json!(blank);
			let response = app
				.operator
				.post("/api/registry", &entry, "json")
				.await
				.unwrap();
			assert_eq!(
				response.status_code(),
				400,
				"{field}={blank:?}: {}",
				response.text()
			);
		}
	}
	assert!(
		Definition::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
	for (index, content) in ["\t\u{a0}content\u{3000}\n", "\u{200b}"]
		.into_iter()
		.enumerate()
	{
		for (base, field) in [(&model_entry, "model_id"), (&skill, "instructions")] {
			let mut entry = base.clone();
			entry["id"] = json!(format!("{field}-{index}"));
			entry["config"][field] = json!(content);
			assert_eq!(
				assert_json(
					app.operator
						.post("/api/registry", &entry, "json")
						.await
						.unwrap(),
					200
				)["config"][field],
				content
			);
		}
	}
}

#[rstest]
#[tokio::test]
async fn published_package_identity_and_digest_round_trip_without_legacy_backfills(
	#[future] endpoint: EndpointFixture,
	mut skill: Value,
	model_entry: Value,
) {
	let app = endpoint.await;
	skill["schema"] = json!({"type":"object","properties":{"fraction":{"type":"number","minimum":1.25e-20,"maximum":std::f64::consts::PI}}});
	skill["config"]["future_extension"] =
		json!({"small":1.25e-20,"large":1.0e20,"precise":f64::from_bits(0x3fb999999999999b)});
	let package = json!({"entity":skill,"author":"Fixture","permissions":[],"dependencies":[]});
	let mut invalid = vec![];
	for field in ["id", "version"] {
		for value in [json!(1), json!(true), Value::Null, json!([]), json!({})] {
			let mut malformed = package.clone();
			malformed["entity"][field] = value;
			invalid.push((malformed, 422));
		}
		let mut missing = package.clone();
		missing["entity"].as_object_mut().unwrap().remove(field);
		invalid.push((missing, 422));
	}
	for (field, value) in [
		("permissions", json!([7])),
		("dependencies", json!([{"id":7,"version":"1.0.0"}])),
	] {
		let mut malformed = package.clone();
		malformed[field] = value;
		invalid.push((malformed, 422));
	}
	let mut unknown = package.clone();
	unknown["entity"]["unexpected"] = json!(true);
	invalid.push((unknown, 422));
	// Models are registered directly; they are not installable marketplace packages.
	let mut model_package = package.clone();
	model_package["entity"] = model_entry;
	invalid.push((model_package, 400));
	for (malformed, status) in invalid {
		let response = app
			.operator
			.post("/api/marketplace", &malformed, "json")
			.await
			.unwrap();
		if status == 422 {
			assert_json_rejection(response, 422);
		} else {
			assert_json(response, 400);
		}
	}
	assert!(
		Package::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
	let published = assert_json(
		app.operator
			.post("/api/marketplace", &package, "json")
			.await
			.unwrap(),
		200,
	);
	let stored = Package::objects()
		.all()
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(stored.id, skill["id"]);
	assert_eq!(stored.version, skill["version"]);
	let expected: aidash_server::registry::Entry = serde_json::from_value(skill.clone()).unwrap();
	assert_eq!(
		stored.manifest["entity"],
		serde_json::to_value(expected).unwrap()
	);
	assert_eq!(
		serde_json::from_str::<Value>(&stored.manifest_source).unwrap(),
		*stored.manifest
	);
	assert_eq!(
		stored.digest,
		format!(
			"sha256:{:x}",
			Sha256::digest(stored.manifest_source.as_bytes())
		)
	);
	assert_eq!(published["digest"], stored.digest);
	let path = "/api/marketplace/endpoint-skill/1.0.0/install";
	let installed = assert_json(
		app.operator
			.post(path, &json!({"digest":stored.digest,"config":{}}), "json")
			.await
			.unwrap(),
		200,
	);
	assert_eq!(installed["config"], skill["config"]);
}

#[rstest]
#[tokio::test]
async fn installed_agent_overrides_require_live_model_references(
	#[future] endpoint: EndpointFixture,
	model_entry: Value,
	mut skill: Value,
) {
	let app = endpoint.await;
	for id in ["validation-model", "override-model"] {
		let mut model = model_entry.clone();
		model["id"] = json!(id);
		assert_json(
			app.operator
				.post("/api/registry", &model, "json")
				.await
				.unwrap(),
			200,
		);
	}
	assert_json(
		app.operator
			.post("/api/registry", &skill, "json")
			.await
			.unwrap(),
		200,
	);
	skill["id"] = json!("installed-agent");
	skill["kind"] = json!("agent");
	skill["config"] =
		json!({"model":{"id":"validation-model","version":"1.0.0"},"instructions":"Work"});
	let package = json!({"entity":skill,"author":"Fixture","permissions":[],"dependencies":[]});
	let published = assert_json(
		app.operator
			.post("/api/marketplace", &package, "json")
			.await
			.unwrap(),
		200,
	);
	let path = "/api/marketplace/installed-agent/1.0.0/install";
	for (reference, status) in [
		(json!({"id":"missing","version":"1.0.0"}), 404),
		(json!({"id":"endpoint-skill","version":"1.0.0"}), 400),
	] {
		assert_json(
			app.operator
				.post(
					path,
					&json!({"digest":published["digest"],"config":{"model":reference}}),
					"json",
				)
				.await
				.unwrap(),
			status,
		);
	}
	assert!(
		Installation::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
	assert!(
		Definition::objects()
			.filter(Definition::field_id().eq("installed-agent".to_owned()))
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
	let reference = json!({"id":"override-model","version":"1.0.0"});
	let installed = assert_json(
		app.operator
			.post(
				path,
				&json!({"digest":published["digest"],"config":{"model":reference}}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_eq!(installed["config"]["model"], reference);
	assert_eq!(
		assert_json(
			app.operator
				.get("/api/registry/installed-agent/1.0.0")
				.await
				.unwrap(),
			200
		)["config"]["model"],
		reference
	);
}

#[rstest]
#[case::tool("tool", json!({"transport":"http","endpoint":"http://localhost:9/base","credential_env":null,"replay":"read_only"}), vec![json!({"transport":"bogus"}),json!({"endpoint":7}),json!({"unexpected":true})], json!({"endpoint":"http://localhost:8/override"}))]
#[case::skill("skill", json!({"instructions":"Base instructions"}), vec![json!({"instructions":7}),json!({"instructions":" \t\u{2003}"}),json!({"unexpected":true})], json!({"instructions":"Override instructions"}))]
#[tokio::test]
async fn invalid_installation_overrides_cannot_replace_saved_configuration(
	#[future] endpoint: EndpointFixture,
	mut skill: Value,
	#[case] kind: &str,
	#[case] config: Value,
	#[case] invalid: Vec<Value>,
	#[case] valid: Value,
) {
	let app = endpoint.await;
	skill["kind"] = json!(kind);
	skill["config"] = config;
	let published = assert_json(
		app.operator
			.post(
				"/api/marketplace",
				&json!({"entity":skill,"author":"Fixture","permissions":[],"dependencies":[]}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let path = "/api/marketplace/endpoint-skill/1.0.0/install";
	let saved = assert_json(
		app.operator
			.post(
				path,
				&json!({"digest":published["digest"],"config":valid}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	for config in invalid {
		assert_json(
			app.operator
				.post(
					path,
					&json!({"digest":published["digest"],"config":config}),
					"json",
				)
				.await
				.unwrap(),
			400,
		);
		assert_eq!(
			assert_json(
				app.operator
					.get("/api/registry/endpoint-skill/1.0.0")
					.await
					.unwrap(),
				200
			),
			saved
		);
	}
	let rows = Installation::objects()
		.all()
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(rows.len(), 1);
	assert_eq!(*rows[0].config, valid);
}

#[rstest]
#[tokio::test]
async fn concurrent_installation_and_registry_republication_preserve_immutable_versions(
	#[future] endpoint: EndpointFixture,
	mut skill: Value,
) {
	let app = endpoint.await;
	skill["kind"] = json!("tool");
	skill["config"] = json!({"transport":"http","endpoint":"http://localhost:9/base","credential_env":null,"replay":"read_only"});
	let published = assert_json(
		app.operator
			.post(
				"/api/marketplace",
				&json!({"entity":skill,"author":"Fixture","permissions":[],"dependencies":[]}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_json(
		app.operator
			.post("/api/registry", &skill, "json")
			.await
			.unwrap(),
		200,
	);
	let mut changed = skill.clone();
	changed["config"] = json!({"transport":"native","operation":"echo"});
	let install =
		json!({"digest":published["digest"],"config":{"endpoint":"http://localhost:8/override"}});
	let (registration, installation) =
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			tokio::join!(
				app.operator.post("/api/registry", &changed, "json"),
				app.operator.post(
					"/api/marketplace/endpoint-skill/1.0.0/install",
					&install,
					"json"
				)
			)
		})
		.await
		.expect("publication and installation must complete without deadlock");
	assert_json(registration.unwrap(), 409);
	assert_eq!(
		assert_json(installation.unwrap(), 200)["config"]["transport"],
		"http"
	);
	let stored = Definition::objects()
		.all()
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(stored.metadata["config"], skill["config"]);
	let effective = assert_json(
		app.operator
			.get("/api/registry/endpoint-skill/1.0.0")
			.await
			.unwrap(),
		200,
	);
	assert_eq!(
		effective["config"]["endpoint"],
		install["config"]["endpoint"]
	);
}
