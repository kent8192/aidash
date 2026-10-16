//! Validate typed request contracts before definitions, packages or events persist.
use crate::endpoint::{EndpointFixture, assert_json, assert_json_rejection, endpoint};
use crate::registry::skill;
use aidash_server::apps::execution::models::Event;
use reinhardt::db::orm::Model;
use rstest::{fixture, rstest};
use serde_json::{Value, json};

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
	let seeded_definitions = app
		.runtime
		.registry
		.list(&Default::default())
		.await
		.unwrap();
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
			.deployment_administrator
			.post("/api/registry", &entry, "json")
			.await
			.unwrap();
		if status == 422 {
			assert_json_rejection(response, 422);
		} else {
			assert_json(response, 400);
		}
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
		app.deployment_administrator
			.post("/api/registry", &valid, "json")
			.await
			.unwrap(),
		200,
	);
	let _: aidash_server::registry::Entry = serde_json::from_value(stored.clone()).unwrap();
	assert_eq!(
		assert_json(
			app.deployment_administrator
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
	let seeded_definitions = app
		.runtime
		.registry
		.list(&Default::default())
		.await
		.unwrap();
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
				.deployment_administrator
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
	assert_eq!(
		app.runtime
			.registry
			.list(&Default::default())
			.await
			.unwrap(),
		seeded_definitions
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
					app.deployment_administrator
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
