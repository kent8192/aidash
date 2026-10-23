//! Ordinary Run admission pins the default without granting disclosure authority.
use super::*;
use aidash_application::registry::system::{COMPACTION_DECIDER_ID, COMPACTION_DECIDER_VERSION};
use aidash_domain::{context::recovery::Failure, decision::Hook};

#[rstest::rstest]
#[case::unapproved("unapproved", Some(Failure::DeciderUnapproved))]
#[case::missing_credential("missing_credential", Some(Failure::DeciderUnavailable))]
#[case::approved("approved", None)]
#[case::removed("removed", Some(Failure::ContextUnreducible))]
#[case::override_decider("override", None)]
#[tokio::test]
async fn ordinary_runs_pin_the_compaction_default_but_only_dispatch_with_live_approval_and_credentials(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
	#[case] state: &str,
	#[case] expected: Option<Failure>,
) {
	let (mut f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let calls = Arc::new(AtomicUsize::new(0));
	let seen = calls.clone();
	let router = reinhardt::test::stub::StubRouter::new()
        .route("/systemone", http::Method::POST, reply(move |request: reinhardt::Request| {
            let body = request.json::<Value>().unwrap();
            seen.fetch_add(1, Ordering::SeqCst);
            assert_eq!(body["model"], "typesafe/jev-1.14");
            let answers: serde_json::Map<_, _> = body["questions"].as_object().unwrap().keys()
                .map(|key| (key.clone(), json!({"noul":0.0}))).collect();
            async move { reinhardt::Response::ok().with_json(&json!({"model":"typesafe/jev-1.14","answers":answers})).unwrap() }
        }))
        .route("/v1/chat/completions", http::Method::POST, reply(|_| async {
            reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Done"}}],"usage":{"prompt_tokens":100,"completion_tokens":10}})).unwrap()
        })).into_server_router();
	let upstream = reinhardt::test::fixtures::server::test_server_guard(router).await;
	let openrouter = tenant_openrouter::install(&mut f);
	app.context.set_singleton(f.clone());
	let (_, token, task) = bootstrap(&f, &app, &upstream.url).await;
	if matches!(state, "approved" | "override") {
		openrouter.connect("acme").await;
		*openrouter.endpoint.lock().unwrap() = upstream.url.clone();
	}
	if matches!(state, "approved" | "missing_credential") {
		let (status, body) = request(
			&app,
			&token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({
				"node":aidash_domain::nodes::NodeName::derived("acme"),
				"entry":{"id":COMPACTION_DECIDER_ID,"version":COMPACTION_DECIDER_VERSION},
				"expected_revision":0,"enabled":true
			}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
	let version = if matches!(state, "removed" | "override") {
		if state == "override" {
			let mut replacement =
				aidash_application::registry::system::compaction_decider().unwrap();
			replacement.id = "custom.compaction".into();
			let (status, body) =
				request(&app, &token, "POST", "/api/registry", json!(replacement)).await;
			assert_eq!(status, 200, "{body}");
			let (status, body) = request(
				&app,
				&token,
				"POST",
				"/api/authorization/acme/catalog",
				json!({
					"node":aidash_domain::nodes::NodeName::derived("acme"),
					"entry":{"id":"custom.compaction","version":"1.0.0"},"expected_revision":0,"enabled":true
				}),
			)
			.await;
			assert_eq!(status, 200, "{body}");
		}
		let (_, mut agent) = request(
			&app,
			&token,
			"GET",
			"/api/registry/research/1.0.0",
			Value::Null,
		)
		.await;
		agent["version"] = json!("1.1.0");
		agent
			.as_object_mut()
			.unwrap()
			.remove("binding_normalization");
		if state == "removed" {
			agent["config"]["remove_default"]
				.as_array_mut()
				.unwrap()
				.push(json!("compaction"));
		} else {
			agent["config"]["bindings"].as_array_mut().unwrap().push(json!({
                "kind":"decider","target":{"registry_deployment":f.config.deployment,"id":"custom.compaction","version":"1.0.0"},
                "narrow":{"decision":{"max_calls_per_run":3}}
            }));
		}
		let (status, body) = request(&app, &token, "POST", "/api/registry", agent).await;
		assert_eq!(status, 200, "{body}");
		// Register the new immutable Agent subject through the Catalog API.
		let (status, body) = request(
			&app,
			&token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({
				"node":aidash_domain::nodes::NodeName::derived("acme"),
				"entry":{"id":"research","version":"1.1.0"},"expected_revision":0,"enabled":true
			}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		"1.1.0"
	} else {
		"1.0.0"
	};
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task}/delegate"),
		json!({
			"node_id":f.config.deployment,"agent":{"id":"research","version":version}
		}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let worker = Harness {
		federation: f.clone(),
	};
	worker.worker_once().await.unwrap();
	let run = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.task_id == task)
		.unwrap();
	assert_eq!(
		run.control,
		aidash_domain::RunControl::Active,
		"ordinary work must start before compaction is needed: {:?}",
		run.error
	);
	assert_eq!(run.phase().as_str(), "THINKING", "{:?}", run.error);
	let snapshot = run
		.context
		.binding_snapshot
		.as_ref()
		.expect("admitted graph");
	let bound = snapshot.bound_decider(Hook::Compaction).unwrap();
	assert_eq!(bound.is_some(), state != "removed");
	if let Some((bound, _)) = bound {
		assert_eq!(
			bound.pin.identity.id,
			if state == "override" {
				"custom.compaction"
			} else {
				COMPACTION_DECIDER_ID
			}
		);
		assert_eq!(bound.pin.identity.version, COMPACTION_DECIDER_VERSION);
	}
	seed_history(&f, &run).await;
	for _ in 0..6 {
		worker.worker_once().await.unwrap();
	}
	let current = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(
		current.recovery.context_reason,
		expected,
		"phase={:?}, control={:?}, error={:?}",
		current.phase(),
		current.control,
		current.error
	);
	assert_eq!(
		calls.load(Ordering::SeqCst),
		usize::from(matches!(state, "approved" | "override"))
	);
	if expected.is_none() {
		assert_eq!(
			current.phase().as_str(),
			"COMPLETED",
			"phase={:?}, control={:?}, error={:?}",
			current.phase(),
			current.control,
			current.error
		);
		assert_eq!(current.context.compactions, 1);
	} else {
		assert_eq!(
			current.control,
			aidash_domain::RunControl::Paused,
			"phase={:?}, control={:?}, error={:?}",
			current.phase(),
			current.control,
			current.error
		);
	}
	cleanup(f, &url, &schema).await;
}
