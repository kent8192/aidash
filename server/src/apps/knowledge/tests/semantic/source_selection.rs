//! Native request selection uses admitted Registry declarations and durable Attempts.
use super::*;
use aidash_domain::{
	context::{recovery::Failure, source::SelectionFailure},
	decision::{
		AnswerType, DeciderConfig, DeciderRoute, Hook, Mode, SOURCE_BUILDER, SOURCE_OPTION,
		SOURCE_RULE, policy::StateRetention,
	},
	registry::bindings::QualifiedRef,
};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};

#[path = "../../../execution/tests/support/tenant_openrouter.rs"]
mod tenant_openrouter;

#[derive(Debug, Clone, Copy)]
pub(super) enum SelectionCase {
	Shadow,
	Enforce,
}
impl SelectionCase {
	fn mode(self) -> Mode {
		match self {
			Self::Shadow => Mode::Shadow,
			Self::Enforce => Mode::Enforce,
		}
	}
}

pub(super) async fn configure_selector(
	f: &mut aidash_server::federation::Federation,
	app: &common::TestApplication,
	token: &str,
	endpoint: &str,
	case: SelectionCase,
) {
	let provider = tenant_openrouter::install(f);
	provider.connect("acme").await;
	*provider.endpoint.lock().unwrap() = endpoint.into();
	app.context.set_singleton(f.clone());
	let config = DeciderConfig {
		hook: Hook::ContextSelection,
		answer_type: AnswerType::Noul,
		description: "Keep relevant Workspace items".into(),
		provider_credential: "openrouter".into(),
		routes: vec![DeciderRoute::jev("typesafe/jev-1.14")],
		builder: SOURCE_BUILDER.into(),
		option_source: SOURCE_OPTION.into(),
		rule: SOURCE_RULE.into(),
		mode: case.mode(),
		max_calls_per_run: 20,
		state_retention: StateRetention::default(),
	};
	f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			serde_json::from_value(json!({
				"id":"context-selector","version":"1.0.0","kind":"decider",
				"name":{"en":"Context selector"},"description":{"en":"Select individual Workspace items"},
				"config":config
			}))
			.unwrap(),
		)
		.await
		.unwrap();
	let (status, body) = request(
		app,
		token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({
			"node":aidash_domain::nodes::NodeName::derived("acme"),
			"entry":{"id":"context-selector","version":"1.0.0"},
			"expected_revision":0,"enabled":true
		}),
	)
	.await;
	assert_eq!(status, 200, "approve selector: {body}");
}

pub(super) fn declaration(f: &aidash_server::federation::Federation, case: SelectionCase) -> Value {
	json!({
		"provider":"aidash.native-context/1","operation":"workspace_retrieval",
		"tools":{"read":[QualifiedRef::builtin(f.config.deployment.as_str(),"workspace_read")],"write":[],"detail":QualifiedRef::builtin(f.config.deployment.as_str(),"workspace_read")},
		"pin":{"policy":"scored","mode":case.mode(),"decider":{"registry_deployment":f.config.deployment,"id":"context-selector","version":"1.0.0"}},
		"volatility":"step","query":"task-inputs@1","refresh":"revision-bound@1",
		"disclosure":{"previews":true,"query":true},"required_home":false
	})
}

/// The first proposal keeps the artifact and drops the message; a later call
/// keeps both. It never supplies admission, receipts or final fitting.
pub(super) fn answer(body: Value, calls: &Arc<Mutex<Vec<Value>>>) -> reinhardt::Response {
	let call = {
		let mut calls = calls.lock().unwrap();
		calls.push(body.clone());
		calls.len()
	};
	assert_eq!(body["state"]["contract"], SOURCE_BUILDER);
	let candidates = body["state"]["candidates"].as_array().unwrap();
	let answers: serde_json::Map<_, _> = body["questions"]
		.as_object()
		.unwrap()
		.keys()
		.map(|question| {
			let identity = question.strip_prefix("keep_").unwrap();
			let candidate = candidates
				.iter()
				.find(|candidate| {
					candidate["id"].as_str().unwrap().strip_prefix("sha256:") == Some(identity)
				})
				.expect("question identifies one source candidate");
			let preview = candidate["preview"].as_str().unwrap();
			let preview =
				serde_json::from_str::<String>(preview).unwrap_or_else(|_| preview.into());
			let keep = match preview.as_str() {
				"A car has a steering wheel." => true,
				"Vehicle safety" => call > 1,
				other => panic!("unexpected disclosed source preview: {other}"),
			};
			(
				question.clone(),
				json!({"type":"noul","noul":if keep {0.9} else {0.1}}),
			)
		})
		.collect();
	reinhardt::Response::ok()
		.with_json(&json!({
			"model":"typesafe/jev-1.14-20260917","provider":"TypeSafe","answers":answers,
			"usage":{"input_tokens":40,"output_tokens":2,"cost":0.000002}
		}))
		.unwrap()
}

async fn cleanup(fixture: ControlledContext) {
	fixture.worker.federation.store.pool.close().await;
	fixture.worker.federation.store.control_pool.close().await;
	drop(fixture.model_server);
	drop(fixture.embedding_server);
	dispose(fixture.f, &fixture.url, &fixture.schema).await;
}

fn workspace_envelopes(context: &Value) -> Vec<Value> {
	context["current"]["context_sources"]
		.as_array()
		.expect("explicit Step sources are attributed data")
		.iter()
		.filter(|item| item["provenance"]["identity"]["source"]["id"] == "context-workspace")
		.cloned()
		.collect()
}

#[rstest::rstest]
#[case::shadow(SelectionCase::Shadow)]
#[case::enforce(SelectionCase::Enforce)]
#[tokio::test]
async fn scored_sources_reach_the_actual_request_as_individual_items(
	#[case] case: SelectionCase,
	#[future(awt)]
	#[with(false, true, false, false, false, Some(case))]
	controlled_context: ControlledContext,
) {
	let fixture = controlled_context;
	let before = fixture.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	// Read the same authorized source through its public search boundary before
	// classification. Shadow must preserve these exact source envelopes.
	let (status, original) = request(
		&fixture._fixture.application,
		&fixture.token,
		"POST",
		&format!("/api/workspaces/{}/semantic/search", before.workspace_id),
		json!({"query":"Research\nUse approved tools","limit":10,"max_tokens":4096}),
	)
	.await;
	assert_eq!(status, 200, "{original}");
	let expected = original["matches"]
		.as_array()
		.unwrap()
		.iter()
		.filter(|item| match case {
			SelectionCase::Shadow => item["source"]["kind"] != "memory",
			SelectionCase::Enforce => item["source"]["kind"] == "artifact",
		})
		.cloned()
		.collect::<Vec<_>>();
	if matches!(case, SelectionCase::Shadow) {
		fixture
			.tool_rounds
			.store(1, std::sync::atomic::Ordering::Release);
	}
	fixture.worker.worker_once().await.unwrap();
	let contexts = fixture.captured.lock().unwrap().clone();
	let run = fixture.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	assert_eq!(contexts.len(), 1, "{run:?}");
	let envelopes = workspace_envelopes(&contexts[0]);
	for envelope in &envelopes {
		assert_eq!(envelope["source"], "context_source");
		assert_eq!(envelope["trust"], "data");
		assert_eq!(envelope["provenance"]["placement"]["volatility"], "step");
	}
	let matches = envelopes
		.iter()
		.map(|item| item["content"].clone())
		.collect::<Vec<_>>();
	assert_eq!(matches, expected);
	let mut kinds = matches
		.iter()
		.map(|item| item["source"]["kind"].as_str().unwrap())
		.collect::<Vec<_>>();
	kinds.sort();
	assert_eq!(
		kinds,
		match case {
			SelectionCase::Shadow => vec!["artifact", "message"],
			SelectionCase::Enforce => vec!["artifact"],
		}
	);
	assert_eq!(
		matches
			.iter()
			.find(|item| item["source"]["kind"] == "artifact")
			.unwrap()["text"],
		serde_json::to_string("A car has a steering wheel.").unwrap()
	);
	let selections = &run.context.source_selections;
	assert_eq!(selections.len(), 1);
	let selection = &selections[0];
	assert_eq!(selection.mode, case.mode());
	assert_eq!(selection.failure, None);
	assert_eq!(selection.candidates.len(), 2);
	assert_ne!(
		selection.candidates[0].identity,
		selection.candidates[1].identity
	);
	assert_eq!(selection.proposed.len(), 1);
	assert_eq!(selection.selected.len(), matches.len());
	assert_eq!(selection.attempts.len(), 1);
	assert_eq!(fixture.selector_calls.lock().unwrap().len(), 1);
	let delivered = run
		.context
		.source_deliveries
		.iter()
		.filter(|item| item.identity.source.id == "context-workspace")
		.collect::<Vec<_>>();
	assert_eq!(delivered.len(), matches.len());
	assert!(
		delivered
			.iter()
			.all(|item| selection.selected.contains(&item.identity.key()))
	);
	// The proposal may exclude an item in Shadow; the accepted request and its
	// receipts still contain both original source envelopes.
	if matches!(case, SelectionCase::Shadow) {
		assert_eq!(
			matches
				.iter()
				.find(|item| item["source"]["kind"] == "message")
				.unwrap()["text"],
			"Vehicle safety"
		);
		assert_eq!(selection.selected.len(), 2);
		for _ in 0..3 {
			if fixture.captured.lock().unwrap().len() == 2 {
				break;
			}
			fixture.worker.worker_once().await.unwrap();
		}
		let contexts = fixture.captured.lock().unwrap().clone();
		assert_eq!(contexts.len(), 2);
		// Both calls use this exact immutable Source, Step placement and source
		// candidates. Changing the Shadow keep proposal cannot change its bytes.
		assert_eq!(
			serde_json::to_vec(&workspace_envelopes(&contexts[0])).unwrap(),
			serde_json::to_vec(&workspace_envelopes(&contexts[1])).unwrap()
		);
		let updated = fixture.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
		assert_eq!(updated.context.source_selections.len(), 2);
		let next = updated.context.source_selections.last().unwrap();
		assert_eq!(next.candidate_digest, selection.candidate_digest);
		assert_eq!(next.query_digest, selection.query_digest);
		assert_eq!(next.proposed.len(), 2);
		assert_eq!(next.selected.len(), 2);
		assert_eq!(next.failure, None);
		assert_eq!(fixture.selector_calls.lock().unwrap().len(), 2);
	}
	cleanup(fixture).await;
}

#[derive(Debug, Clone, Copy)]
enum Denial {
	Unapproved,
	Disclosure,
}

#[rstest::rstest]
#[case::unapproved(Denial::Unapproved)]
#[case::disclosure(Denial::Disclosure)]
#[tokio::test]
async fn source_selection_denial_pauses_before_any_paid_request(
	#[case] denial: Denial,
	#[future(awt)]
	#[with(false, true, false, false, false, Some(SelectionCase::Enforce))]
	controlled_context: ControlledContext,
) {
	let fixture = controlled_context;
	let app = &fixture._fixture.application;
	let expected = match denial {
		Denial::Unapproved => {
			let (status, body) = request(app, &fixture.token, "POST", "/api/authorization/acme/catalog", json!({
				"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"context-selector","version":"1.0.0"},"expected_revision":1,"enabled":false
			})).await;
			assert_eq!(status, 200, "{body}");
			(
				Failure::SourceSelectionUnapproved,
				SelectionFailure::Unapproved,
			)
		}
		Denial::Disclosure => {
			let (status, snapshot) = request(
				app,
				&fixture.f.config.api_token,
				"GET",
				"/api/authorization/acme",
				Value::Null,
			)
			.await;
			assert_eq!(status, 200, "{snapshot}");
			let mut policy = snapshot["bundle"].clone();
			policy["policies"].as_array_mut().unwrap().push(json!({
				"id":"deny-selector-disclosure","effect":"deny","subjects":{"any":true},"actions":["semantic.disclose"],"resources":{"kinds":["workspace"]},
				"condition":{"op":"eq","left":{"source":"resource","path":"/hook"},"right":{"source":"literal","value":"context_selection"}}
			}));
			let (status, body) = request(
				app,
				&fixture.f.config.api_token,
				"POST",
				"/api/authorization/acme",
				json!({"expected_revision":snapshot["revision"],"bundle":policy}),
			)
			.await;
			assert_eq!(status, 200, "{body}");
			(
				Failure::SourceSelectionForbidden,
				SelectionFailure::ForbiddenDisclosure,
			)
		}
	};
	fixture.worker.worker_once().await.unwrap();
	let run = fixture.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	assert_eq!(run.control, aidash_domain::RunControl::Paused);
	assert_eq!(run.recovery.context_reason, Some(expected.0), "{run:?}");
	assert_eq!(
		run.context.source_selections.last().unwrap().failure,
		Some(expected.1)
	);
	assert!(fixture.selector_calls.lock().unwrap().is_empty());
	assert!(fixture.captured.lock().unwrap().is_empty());
	let query = Query::select()
		.expr(Expr::cust("COUNT(*)"))
		.from(Alias::new("decision_attempts"))
		.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
		.to_string(PostgresQueryBuilder);
	let attempts: i64 = sqlx::query_scalar(&query)
		.fetch_one(fixture.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(attempts, 0);
	cleanup(fixture).await;
}
