//! Duty templates in the Marketplace (#1011 D4), over HTTP and PostgreSQL:
//! a person publishes one of their Duties as a template, another person
//! installs it from their Thread as their own Duty, and a later version
//! is offered, never applied by itself.
use super::*;

/// A Tenant where alice (its creator) and bob (a member) can start the
/// Tenant's `reporter` Agent, and a Thread of each of them.
struct Fixture {
	d: Deployment,
	alice: Person,
	bob: Person,
	alice_context: String,
	bob_context: String,
	alice_thread: String,
	bob_thread: String,
}

impl Fixture {
	async fn new(runtime: common::RuntimeFuture) -> Self {
		let d = Deployment::new(runtime, open_rules()).await;
		let alice = d.person("alice").await;
		let bob = d.person("bob").await;
		let (tenant, alice_context) = d.tenant_of(&alice, "Team").await;
		let bob_context = d.admit(&tenant, &bob, "bob", json!(["members"])).await;
		let name = aidash_domain::tenancy::TenantName::new(&tenant).unwrap();
		d.federation.registry.seed(&name).await.unwrap();
		let mut agent = None;
		for (kind, key, config) in [
			(
				"model",
				"reporter-model",
				json!({"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:1/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
			),
			(
				"agent",
				"reporter",
				json!({"schema_version":1,"model":{"id":"reporter-model","version":"1.0.0"},"instructions":"Report","bindings":[],"remove_default":[]}),
			),
		] {
			let entry: aidash_server::registry::Entry = serde_json::from_value(json!({"id":key,"version":"1.0.0","kind":kind,"name":{"en":key},"description":{"en":"Duty template fixture"},"schema":{},"config":config})).unwrap();
			d.federation
				.registry
				.register(&name, entry.clone())
				.await
				.unwrap();
			agent = Some(entry);
		}
		common::approve_agent(&d.federation, &tenant, &agent.unwrap()).await;
		let mut threads = vec![];
		for (person, context) in [(&alice, &alice_context), (&bob, &bob_context)] {
			let (status, thread) = d
				.as_person(
					person,
					"POST",
					"/api/threads",
					Some(context),
					json!({"title":"Operations"}),
				)
				.await;
			assert_eq!(status, 200, "{thread}");
			threads.push(
				thread["id"].as_str().unwrap().to_owned(),
			);
		}
		let bob_thread = threads.pop().unwrap();
		let alice_thread = threads.pop().unwrap();
		Self {
			d,
			alice,
			bob,
			alice_context,
			bob_context,
			alice_thread,
			bob_thread,
		}
	}

	async fn alice(&self, method: &str, path: &str, body: Value) -> (u16, Value) {
		self.d
			.as_person(&self.alice, method, path, Some(&self.alice_context), body)
			.await
	}

	async fn bob(&self, method: &str, path: &str, body: Value) -> (u16, Value) {
		self.d
			.as_person(&self.bob, method, path, Some(&self.bob_context), body)
			.await
	}

	/// Alice's Duty with a time trigger, a Router and a webhook trigger.
	async fn alice_duty(&self, procedure: &str) -> String {
		let (status, duty) = self
			.alice(
				"POST",
				&format!("/api/threads/{}/duties", self.alice_thread),
				json!({
					"title": "GitHub CI duty",
					"mission": "Fix failing CI runs on main.",
					"procedure": procedure,
					"agent": {"id":"reporter","version":"1.0.0"},
					"trigger": {"kind":"interval","every_seconds":3600},
					"router": {"language":"python","source":"print('{\"action\":\"task\",\"title\":\"CI\",\"description\":\"run\"}')"}
				}),
			)
			.await;
		assert_eq!(status, 200, "{duty}");
		let id = duty["id"].as_str().unwrap().to_owned();
		let (status, trigger) = self
			.alice(
				"POST",
				&format!("/api/duties/{id}/triggers"),
				json!({"kind":"webhook"}),
			)
			.await;
		assert_eq!(status, 200, "{trigger}");
		id
	}

	async fn publish(&self, duty: &str, version: &str, mission: Option<&str>) -> Value {
		if let Some(mission) = mission {
			let (status, body) = self
				.alice(
					"PATCH",
					&format!("/api/duties/{duty}"),
					json!({"mission": mission}),
				)
				.await;
			assert_eq!(status, 200, "{body}");
		}
		let (status, published) = self
			.alice(
				"POST",
				&format!("/api/duties/{duty}/template"),
				json!({
					"package_id": "github-ci-duty",
					"version": version,
					"author": "Acme Platform",
					"triggers": [
						{"kind":"time","instruction":"Hourly is enough for most repositories."},
						{"kind":"webhook","instruction":"Add the URL as a workflow_run webhook in GitHub."}
					],
					"idempotency_key": Uuid::new_v4(),
				}),
			)
			.await;
		assert_eq!(status, 200, "{published}");
		published
	}
}

#[rstest::rstest]
#[tokio::test]
async fn a_published_duty_installs_as_the_installers_own_duty(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	// Arrange: alice's Duty, and the template it would publish as.
	let f = Fixture::new(runtime).await;
	let duty = f.alice_duty("1. Read the failed job.").await;
	let (status, draft) = f
		.alice("GET", &format!("/api/duties/{duty}/template"), Value::Null)
		.await;
	assert_eq!(status, 200, "{draft}");
	assert_eq!(draft["triggers"][0]["kind"], "interval");
	assert_eq!(draft["triggers"][1]["kind"], "webhook");
	assert_eq!(draft["agent"]["id"], "reporter");
	// Act: alice publishes it.
	let published = f.publish(&duty, "1.0.0", None).await;
	let key = published["key"].as_str().unwrap().to_owned();
	// Assert: bob, a member, finds it among the Tenant's packages and reads
	// a template that names no person, secret or Duty id.
	let (status, packages) = f
		.bob("GET", "/api/marketplace/packages?q=github", Value::Null)
		.await;
	assert_eq!(status, 200, "{packages}");
	let summary = packages
		.as_array()
		.unwrap()
		.iter()
		.find(|package| package["key"] == key)
		.unwrap_or_else(|| panic!("{packages}"));
	assert_eq!(summary["kind"], "duty_template");
	assert!(
		summary["actions"]
			.as_array()
			.unwrap()
			.iter()
			.any(|action| action == "install"),
		"{summary}"
	);
	let (status, detail) = f
		.bob(
			"GET",
			&format!("/api/marketplace/packages/{key}"),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{detail}");
	let manifest = detail["manifest"].to_string();
	// Alice is the Tenant Creator, the subject `owner`.
	for absent in [
		duty.as_str(),
		"\"owner\"",
		"secret",
		"requester",
		"credential",
	] {
		assert!(!manifest.contains(absent), "{absent} in {manifest}");
	}
	let digest = detail["summary"]["digest"].as_str().unwrap().to_owned();
	// Act: bob installs it from his Thread with the suggested Agent.
	let request = json!({
		"digest": digest,
		"thread_id": f.bob_thread,
		"idempotency_key": Uuid::new_v4(),
	});
	let (status, installed) = f
		.bob(
			"POST",
			&format!("/api/marketplace/packages/{key}/duty"),
			request.clone(),
		)
		.await;
	// Assert: bob's own Duty with the template's work, the time trigger as
	// given and the webhook left for him to create.
	assert_eq!(status, 200, "{installed}");
	let mine = &installed["duty"];
	assert_eq!(mine["requester"], "bob");
	assert_eq!(mine["origin_thread_id"], f.bob_thread.as_str());
	assert_eq!(mine["mission"], "Fix failing CI runs on main.");
	assert_eq!(mine["procedure"], "1. Read the failed job.");
	assert_eq!(mine["trigger"]["every_seconds"], 3600);
	assert_eq!(mine["router"]["language"], "python");
	assert_eq!(mine["agent"]["id"], "reporter");
	assert_eq!(mine["template"]["key"], key.as_str());
	assert_eq!(mine["template"]["version"], "1.0.0");
	assert!(mine["template"].get("install_key").is_none(), "{mine}");
	assert_eq!(
		installed["triggers_to_create"],
		json!([{"kind":"webhook","instruction":"Add the URL as a workflow_run webhook in GitHub."}])
	);
	let bob_duty = mine["id"].as_str().unwrap().to_owned();
	let (status, triggers) = f
		.bob(
			"GET",
			&format!("/api/duties/{bob_duty}/triggers"),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{triggers}");
	assert_eq!(triggers, json!([]));
	// Act + Assert: a retried install returns the same Duty.
	let (status, again) = f
		.bob(
			"POST",
			&format!("/api/marketplace/packages/{key}/duty"),
			request,
		)
		.await;
	assert_eq!(status, 200, "{again}");
	assert_eq!(again["duty"]["id"], bob_duty.as_str());
	// Act + Assert: a Duty template is never installed as a Registry copy.
	let (status, refused) = f
		.bob(
			"POST",
			&format!("/api/marketplace/packages/{key}/install"),
			json!({"digest": digest, "idempotency_key": Uuid::new_v4()}),
		)
		.await;
	assert_eq!(status, 400, "{refused}");
	// Act: alice publishes a later version.
	f.publish(&duty, "1.1.0", Some("Fix failing CI runs and report them."))
		.await;
	// Assert: bob's Duty is offered the update and still has the old work.
	let (status, page) = f
		.bob("GET", &format!("/api/duties/{bob_duty}"), Value::Null)
		.await;
	assert_eq!(status, 200, "{page}");
	assert_eq!(page["template_update"]["version"], "1.1.0");
	assert_eq!(page["duty"]["mission"], "Fix failing CI runs on main.");
	// Act: bob applies it.
	let next = page["template_update"]["key"].as_str().unwrap().to_owned();
	let (status, next_detail) = f
		.bob(
			"GET",
			&format!("/api/marketplace/packages/{next}"),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{next_detail}");
	let (status, updated) = f
		.bob(
			"POST",
			&format!("/api/duties/{bob_duty}/template-update"),
			json!({"key": next, "digest": next_detail["summary"]["digest"]}),
		)
		.await;
	// Assert
	assert_eq!(status, 200, "{updated}");
	assert_eq!(updated["mission"], "Fix failing CI runs and report them.");
	assert_eq!(updated["template"]["version"], "1.1.0");
	assert_eq!(updated["requester"], "bob");
	f.d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_template_with_a_secret_like_field_or_credential_is_refused(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	// Arrange
	let f = Fixture::new(runtime).await;
	let duty = f.alice_duty("1. Read the failed job.").await;
	let publish = |extra: Value| {
		let mut body = json!({
			"package_id": "github-ci-duty",
			"version": "1.0.0",
			"author": "Acme Platform",
			"triggers": [
				{"kind":"time","instruction":"Hourly."},
				{"kind":"webhook","instruction":"Add the URL as a webhook."}
			],
			"idempotency_key": Uuid::new_v4(),
		});
		for (field, value) in extra.as_object().unwrap() {
			body[field] = value.clone();
		}
		body
	};
	// Act + Assert: a template has no place for a secret or a Requester; the
	// body is refused as it is parsed.
	for extra in [
		json!({"webhook_secret": "s3cr3t"}),
		json!({"requester": "alice"}),
		json!({"triggers": [{"kind":"webhook","instruction":"x","secret":"s3cr3t"}]}),
	] {
		let (status, refused) = f
			.alice(
				"POST",
				&format!("/api/duties/{duty}/template"),
				publish(extra.clone()),
			)
			.await;
		assert_eq!(status, 422, "{extra}: {refused}");
	}
	// Act + Assert: a credential written into the Duty's procedure stays home.
	let leaky = f
		.alice_duty("1. export GITHUB_TOKEN=ghp_0123456789abcdefABCDEF0123456789abcd")
		.await;
	let (status, refused) = f
		.alice(
			"POST",
			&format!("/api/duties/{leaky}/template"),
			publish(json!({})),
		)
		.await;
	assert_eq!(status, 400, "{refused}");
	assert!(
		refused.to_string().contains("carries no secrets"),
		"{refused}"
	);
	// Act + Assert: only a Duty's Requester publishes it.
	let (status, refused) = f
		.bob(
			"POST",
			&format!("/api/duties/{duty}/template"),
			publish(json!({})),
		)
		.await;
	assert!(matches!(status, 400 | 403 | 404), "{status}: {refused}");
	// Assert: nothing was published.
	let published =
		f.d.count("SELECT count(*) FROM marketplace_versions WHERE kind = 'duty_template'")
			.await;
	assert_eq!(published, 0);
	f.d.finish().await;
}
