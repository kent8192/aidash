#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
use aidash_application::tenancy::TEST_FIXTURE;
use common::upstream_fixtures as upstream;
use reinhardt::ServerRouter as Router;
use rstest::fixture;
use upstream::reply;
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_server::semantic::{self, ConfigureIndex, EmbeddingConfig, IndexSpec, VectorConfig};

use common::request;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use uuid::Uuid;
#[path = "semantic/source_selection.rs"]
mod source_selection;

struct ControlledContext {
	_fixture: common::ApplicationFixture,
	f: aidash_server::federation::Federation,
	worker: aidash_server::harness::Harness,
	url: String,
	schema: String,
	/// The Task owner's token.
	token: String,
	captured: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
	selector_calls: Arc<Mutex<Vec<Value>>>,
	/// Tool-call responses before the model stops, for Runs that need several inferences.
	tool_rounds: std::sync::Arc<std::sync::atomic::AtomicUsize>,
	model_server: reinhardt::test::fixtures::server::TestServerGuard,
	embedding_server: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	fault: std::sync::Arc<EmbeddingFault>,
}

mod controlled_context_composition {
	// rstest 0.26.1 does not copy function-level allows onto generated partial_N functions.
	// Each flag selects an independent context shape that tests combine through `#[with]`.
	#![allow(clippy::too_many_arguments)]
	use super::*;
	#[rstest::fixture]
	pub(super) async fn controlled_context(
		#[default(true)] memory: bool,
		#[default(true)] workspace_retrieval: bool,
		#[default(false)] degrade: bool,
		#[default(false)] on_demand_memory: bool,
		#[default(None)] selection: Option<source_selection::SelectionCase>,
		#[from(common::runtime)] _runtime: common::RuntimeFuture,
		#[future(awt)]
		#[from(common::native_application)]
		#[with(Default::default(), aidash_server::sse::Service::new(Default::default()), Arc::new(|r| r), _runtime.clone())]
		application_fixture: common::ApplicationFixture,

		#[future(awt)]
		#[from(controlled_context_provider)]
		fixture: ControlledContextProvider,

		#[from(embedding_fault)] fault: std::sync::Arc<EmbeddingFault>,
		#[from(embeddings_router)]
		#[with(fault.clone())]
		_embeddings_router: std::sync::Arc<Router>,
		#[future(awt)]
		#[with(_embeddings_router.clone())]
		embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	) -> ControlledContext {
		let captured = fixture.state.captured;
		let selector_calls = fixture.state.selector_calls;
		let tool_rounds = fixture.state.tool_rounds;
		let model_server = fixture.server;

		use aidash_server::domain::{ArtifactInput, qualified_agent};
		let (mut f, url, schema) = application_fixture.runtime.parts();
		let app = application_fixture.application.clone();

		let endpoint = model_server.url.clone();
		let (mut policy, token, task) =
			common::bootstrap_with_context(&f, &app, &endpoint, true).await;
		if let Some(selection) = selection {
			source_selection::configure_selector(&mut f, &app, &token, &endpoint, selection).await;
		}
		let acme = aidash_domain::tenancy::TenantName::new("acme").unwrap();
		let mut agent = f.registry.get(&acme, "research", "1.0.0").await.unwrap();
		agent.version = "1.0.1".into();
		agent.binding_normalization = None;
		let mut bindings = vec![];
		for (enabled, id, kind, adapter) in [
			(memory, "context-memory", "memory", "semantic_memory"),
			(
				workspace_retrieval,
				"context-thread",
				"source",
				"workspace_retrieval",
			),
		] {
			if !enabled {
				continue;
			}
			let mut source = json!({"adapter":adapter});
			// `degrade` is the default since ADR 0129; `false` asks for the
			// Outage Wait explicitly.
			if adapter == "workspace_retrieval" {
				source["on_unavailable"] = json!(if degrade { "degrade" } else { "pause" });
			}
			let mut entry = json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Explicit native context"},"config":{"schema_version":1,"source":source}});
			if on_demand_memory && adapter == "semantic_memory" {
				entry["config"]["context_source"] = json!({"provider":"aidash.native-context/1","operation":"semantic_memory","tools":{"read":[],"write":[],"detail":null},"pin":{"policy":"on_demand"},"volatility":"step","query":"task-inputs@1","refresh":"revision-bound@1","disclosure":{"previews":false,"query":false},"required_home":false});
			}
			if adapter == "workspace_retrieval"
				&& let Some(selection) = selection
			{
				entry["config"]["context_source"] = source_selection::declaration(&f, selection);
			}
			f.registry
				.register(&acme, serde_json::from_value(entry).unwrap())
				.await
				.unwrap();
			assert_eq!(
				common::post(&app, &token, "/api/authorization/acme/catalog", json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":id,"version":"1.0.0"},"expected_revision":0,"enabled":true}))
				.await
				.0,
				200
			);
			bindings.push(json!({"kind":kind,"target":{"registry_deployment":f.config.deployment,"id":id,"version":"1.0.0"},"narrow":{}}));
		}
		if selection.is_some() {
			bindings.push(json!({"kind":"decider","target":{"registry_deployment":f.config.deployment,"id":"context-selector","version":"1.0.0"},"narrow":{}}));
		}
		agent.config["bindings"] = json!(bindings);
		f.registry.register(&acme, agent).await.unwrap();
		let owner = qualified_agent(
			&aidash_domain::nodes::NodeAddress::derived(f.config.deployment.as_str(), "acme"),
			"research",
			"1.0.1",
		);
		policy["subjects"][&owner] =
			json!({"kind":"agent", "node": aidash_domain::nodes::NodeName::derived("acme")});
		assert_eq!(
			common::post(
				&app,
				&f.config.api_token,
				"/api/authorization/acme",
				json!({"expected_revision":1,"bundle":policy})
			)
			.await
			.0,
			200
		);
		assert_eq!(
			common::post(&app, &token, "/api/authorization/acme/catalog", json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"research","version":"1.0.1"},"expected_revision":0,"enabled":true}))
			.await
			.0,
			200
		);
		let thread = f.store.task(&TEST_FIXTURE, task).await.unwrap().thread_id;
		let embedding_endpoint = embeddings.url.clone();
		// The embedding fixture is a Keyless endpoint on loopback (ADR 0036).
		f.store
			.keyless
			.permit_origin_of(&embedding_endpoint)
			.unwrap();
		let embedding_server = embeddings;
		configure(&f, &token, thread, &spec(&embedding_endpoint), 0).await;
		assert_eq!(
			common::post(
				&app,
				&token,
				&format!("/api/tasks/{task}/claim"),
				json!({"revision":0,"agent":{"id":"research","version":"1.0.1"}})
			)
			.await
			.0,
			200
		);
		let worker = aidash_server::harness::Harness {
			federation: f.for_workers().await.unwrap(),
		};
		worker.worker_once().await.unwrap();
		let artifact = f
			.store
			.publish_artifact(
				&TEST_FIXTURE,
				task,
				&owner,
				"controlled-artifact",
				&ArtifactInput {
					kind: "text".into(),
					name: "Car report".into(),
					content: json!("A car has a steering wheel."),
				},
			)
			.await
			.unwrap();
		let message = f
			.store
			.message_record(
				&TEST_FIXTURE,
				thread,
				"human",
				"Vehicle safety",
				Some("controlled-message"),
			)
			.await
			.unwrap();
		for (key, source) in [
			(
				"memory",
				json!({"kind":"memory","text":"A car carries passengers."}),
			),
			("artifact", json!({"kind":"artifact","id":artifact.id})),
			("message", json!({"kind":"message","id":message.id})),
		] {
			let (status, entry) = common::post(
				&app,
				&token,
				&format!("/api/threads/{thread}/semantic/entries"),
				json!({"key":key,"expected_revision":0,"source":source,"metadata":{}}),
			)
			.await;
			assert_eq!(status, 200, "source: {entry}");
		}
		semantic::worker::sweep(&f.store).await.unwrap();
		ControlledContext {
			_fixture: application_fixture,
			f,
			worker,
			url,
			schema,
			token,
			captured,
			selector_calls,
			tool_rounds,
			model_server,
			embedding_server,
			fault,
		}
	}
}
use controlled_context_composition::controlled_context;

#[rstest::rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
#[tokio::test]
async fn automatic_semantic_context_respects_each_agent_behavior_control(
	#[case] memory: bool,
	#[case] workspace_retrieval: bool,
	#[future(awt)]
	#[with(memory, workspace_retrieval)]
	controlled_context: ControlledContext,
) {
	let fixture = controlled_context;
	fixture.worker.worker_once().await.unwrap();
	let contexts = fixture.captured.lock().unwrap().clone();
	assert_eq!(
		contexts.len(),
		1,
		"run: {:?}",
		fixture.f.store.runs(&TEST_FIXTURE).await.unwrap()[0]
	);
	let semantic = &contexts[0]["snapshot"]["semantic_memory"]["thread"];
	if !memory && !workspace_retrieval {
		assert!(semantic.is_null(), "disabled retrieval: {semantic}");
	} else {
		let mut kinds: Vec<_> = semantic["matches"]
			.as_array()
			.unwrap()
			.iter()
			.map(|m| m["source"]["kind"].as_str().unwrap())
			.collect();
		kinds.sort();
		let mut expected = Vec::new();
		if memory {
			expected.push("memory");
		}
		if workspace_retrieval {
			expected.extend(["artifact", "message"]);
		}
		expected.sort();
		assert_eq!(kinds, expected, "semantic: {semantic}");
	}
	fixture.worker.federation.store.pool.close().await;
	fixture.worker.federation.store.control_pool.close().await;
	drop(fixture.model_server);
	drop(fixture.embedding_server);
	dispose(fixture.f, &fixture.url, &fixture.schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn on_demand_memory_does_not_leak_through_an_always_thread_source(
	#[future(awt)]
	#[with(true, true, false, true)]
	controlled_context: ControlledContext,
) {
	let fixture = controlled_context;
	fixture.worker.worker_once().await.unwrap();
	let contexts = fixture.captured.lock().unwrap().clone();
	assert_eq!(contexts.len(), 1);
	let matches = contexts[0]["snapshot"]["semantic_memory"]["thread"]["matches"]
		.as_array()
		.expect("thread result remains available");
	let mut kinds = matches
		.iter()
		.map(|item| item["source"]["kind"].as_str().unwrap())
		.collect::<Vec<_>>();
	kinds.sort();
	assert_eq!(kinds, ["artifact", "message"]);
	fixture.worker.federation.store.pool.close().await;
	fixture.worker.federation.store.control_pool.close().await;
	drop(fixture.model_server);
	drop(fixture.embedding_server);
	dispose(fixture.f, &fixture.url, &fixture.schema).await;
}

/// Why automatic semantic retrieval cannot answer during a Run's step.
#[derive(Debug, Clone, Copy)]
enum Unavailable {
	/// The embedding provider answers HTTP 503.
	ProviderStatus,
	/// The embedding provider no longer accepts connections.
	ProviderTransport,
	/// The embedding provider accepts the request and never answers in time.
	ProviderTimeout,
	/// The vector backend holds none of the indexed points.
	VectorBackend,
}

/// Issue #233: a local Run in a Thread whose index has `auto_context`
/// enabled treats unavailable semantic retrieval as a typed, transient
/// `unavailable` semantic failure, like a required-Home Run. The step stops
/// before inference and retries with a persisted delay; the Run and its Task
/// are not failed.
#[rstest::rstest]
#[case::provider_status(Unavailable::ProviderStatus)]
#[case::provider_transport(Unavailable::ProviderTransport)]
#[case::provider_timeout(Unavailable::ProviderTimeout)]
#[case::vector_backend(Unavailable::VectorBackend)]
#[tokio::test]
async fn local_auto_context_semantic_outage_is_a_typed_transient_failure_for_every_cause(
	#[case] unavailable: Unavailable,
	#[future(awt)]
	#[with(false, true)]
	controlled_context: ControlledContext,
) {
	use aidash_server::domain::{RunState, TaskStatus, WaitingState};
	let mut fixture = controlled_context;
	let mut embedding_server = Some(fixture.embedding_server);
	// Indexing already called the provider; only calls after this point belong to the step.
	let indexed = fixture.fault.requests();
	let query_calls = match unavailable {
		Unavailable::ProviderStatus => {
			fixture.fault.set(Fault::Unavailable);
			1
		}
		Unavailable::ProviderTimeout => {
			fixture.fault.set(Fault::Hang);
			1
		}
		Unavailable::ProviderTransport => {
			let server = std::sync::Arc::into_inner(embedding_server.take().unwrap())
				.expect("the test owns the embedding provider");
			let address = server.url.trim_start_matches("http://").to_owned();
			drop(server);
			// The guard closes its listener asynchronously; wait until connections are refused.
			while tokio::net::TcpStream::connect(&address).await.is_ok() {
				tokio::task::yield_now().await;
			}
			// Indexing left keep-alive sockets in the shared client pools; fresh
			// clients can only open new connections, so the step observes the
			// refusal. Embeddings use the Keyless-aware provider client pair.
			let store = &mut fixture.worker.federation.store;
			store.semantic_client = semantic::backend::client().unwrap();
			store.provider_clients = semantic::backend::clients().unwrap();
			0
		}
		Unavailable::VectorBackend => {
			let mut connection = fixture
				.worker
				.federation
				.store
				.pool
				.begin(&TEST_FIXTURE)
				.await
				.unwrap();
			aidash_server::database::native::query(
				&reinhardt::query::Query::delete()
					.from_table(reinhardt::query::Alias::new("semantic_vectors"))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut *connection)
			.await
			.unwrap();
			connection.commit().await.unwrap();
			// Point presence is checked before the query is embedded.
			0
		}
	};

	// Act: the step that requests semantic context before inference.
	assert!(fixture.worker.worker_once().await.unwrap());
	let run = fixture.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);

	// Assert: a scheduled retry with the typed transient reason, no inference.
	assert_eq!(
		fixture.fault.requests() - indexed,
		query_calls,
		"{unavailable:?}"
	);
	assert!(
		fixture.captured.lock().unwrap().is_empty(),
		"{unavailable:?}"
	);
	assert_eq!(run.control.as_str(), "ACTIVE", "{unavailable:?}");
	let retry = run.recovery.retry.as_ref().expect("a scheduled retry");
	assert_eq!(retry.count, 1, "{unavailable:?}");
	assert!(retry.at > chrono::Utc::now(), "{unavailable:?}");
	// Missing vector points are retried the same way but typed apart: the
	// index needs repair, so they pause after the retries instead of waiting
	// (ADR 0059, ADR 0129).
	let (reason, message) = if matches!(unavailable, Unavailable::VectorBackend) {
		(
			"index_repair",
			"The semantic index is missing vector points and needs repair",
		)
	} else {
		(
			"unavailable",
			"The semantic backend is temporarily unavailable",
		)
	};
	assert_eq!(
		serde_json::to_value(run.recovery.semantic_reason).unwrap(),
		reason,
		"{unavailable:?}"
	);
	assert_eq!(run.error.as_deref(), Some(message), "{unavailable:?}");
	assert!(
		!matches!(
			&run.state,
			RunState::Waiting(wait) if matches!(**wait, WaitingState::FailureDelivery { .. })
		),
		"{unavailable:?}: {:?}",
		run.state
	);
	assert_ne!(
		fixture
			.f
			.store
			.task(&TEST_FIXTURE, run.task_id)
			.await
			.unwrap()
			.status,
		TaskStatus::Failed,
		"{unavailable:?}"
	);

	fixture.worker.federation.store.pool.close().await;
	fixture.worker.federation.store.control_pool.close().await;
	drop(fixture.model_server);
	drop(embedding_server);
	dispose(fixture.f, &fixture.url, &fixture.schema).await;
}

/// Issue #233, ADR 0129: after the bounded retry a local Run whose Thread
/// source waits (`on_unavailable: pause`) enters an Outage Wait instead of a
/// pause, keeps its Task, and goes on to inference by itself once the provider
/// answers (or the index completes) and the wait wakes. No person resumes it.
#[rstest::rstest]
#[case::provider_outage(false)]
#[case::pending_index(true)]
#[tokio::test]
async fn local_semantic_outage_waits_after_bounded_retry_and_goes_on_when_the_provider_recovers(
	#[case] pending_entry: bool,
	#[future(awt)]
	#[with(false, true)]
	controlled_context: ControlledContext,
) {
	use aidash_server::domain::{RunState, TaskStatus, WaitingState};
	let fixture = controlled_context;
	if pending_entry {
		add_pending_thread_entry(&fixture).await;
	} else {
		fixture.fault.set(Fault::Unavailable);
	}
	let mut run_id = None;
	for failure in 1..=6 {
		assert!(
			fixture.worker.worker_once().await.unwrap(),
			"attempt {failure}"
		);
		let run = fixture.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
		run_id = Some(run.id);
		assert!(
			fixture.captured.lock().unwrap().is_empty(),
			"attempt {failure}"
		);
		let reason = serde_json::to_value(run.recovery.semantic_reason).unwrap();
		if failure < 6 {
			assert_eq!(run.control.as_str(), "ACTIVE", "{run:?}");
			assert_eq!(reason, "unavailable", "{run:?}");
			let retry = run.recovery.retry.as_ref().unwrap();
			assert_eq!(retry.count, failure, "{run:?}");
			assert!(retry.at > chrono::Utc::now(), "{run:?}");
			// Advance only the persisted deadline after checking that the
			// production path saved a delay and kept its failure counter.
			advance_retry(&fixture, run.id).await;
		} else {
			assert_eq!(run.control.as_str(), "ACTIVE", "{run:?}");
			// The Run retries by itself while it waits, so management offers
			// only `wait`; `retries_exhausted` comes at the bound (ADR 0129).
			assert_eq!(reason, "unavailable", "{run:?}");
			assert!(run.recovery.semantic_outage_retries_exhausted(), "{run:?}");
			assert!(run.recovery.retry.is_none(), "{run:?}");
			assert!(
				matches!(&run.state, RunState::Waiting(wait)
					if matches!(wait.as_ref(), WaitingState::Outage { service, .. } if service == "Semantic context")),
				"{run:?}"
			);
			assert_ne!(
				fixture
					.f
					.store
					.task(&TEST_FIXTURE, run.task_id)
					.await
					.unwrap()
					.status,
				TaskStatus::Failed
			);
		}
	}

	let run_id = run_id.unwrap();
	let (status, management) = common::get(
		&fixture._fixture.application,
		&fixture.token,
		&format!("/api/executions/{run_id}/management"),
	)
	.await;
	assert_eq!(status, 200, "{management}");
	assert_eq!(management["semantic_recovery"]["actions"], json!(["wait"]));

	fixture.fault.clear();
	if pending_entry {
		aidash_server::semantic::worker::sweep(&fixture.f.store)
			.await
			.unwrap();
	}
	// The wait wakes at its next check; nobody resumes the Run.
	advance_outage_wake(&fixture, run_id).await;
	run_to_inference(&fixture, 1).await;
	let run = fixture.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	assert_eq!(run.control.as_str(), "ACTIVE", "{run:?}");
	assert!(run.recovery.outage.is_none(), "{run:?}");
	let contexts = fixture.captured.lock().unwrap().clone();
	assert_eq!(contexts.len(), 1, "the woken step reaches inference");
	assert!(
		contexts[0]["snapshot"]["semantic_memory"]["thread"]["matches"]
			.as_array()
			.is_some_and(|matches| !matches.is_empty()),
		"the resumed step carries semantic context: {}",
		contexts[0]
	);

	fixture.worker.federation.store.pool.close().await;
	fixture.worker.federation.store.control_pool.close().await;
	drop(fixture.model_server);
	drop(fixture.embedding_server);
	dispose(fixture.f, &fixture.url, &fixture.schema).await;
}

/// Add a visible Thread source without running the indexer. A healthy
/// provider alone cannot make this entry READY; indexing must finish first.
async fn add_pending_thread_entry(fixture: &ControlledContext) {
	let run = fixture.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	let message = fixture
		.f
		.store
		.message_record(
			&TEST_FIXTURE,
			run.thread_id,
			"alice",
			"Newly written vehicle maintenance knowledge.",
			Some("pending-semantic-entry"),
		)
		.await
		.unwrap();
	let (status, entry) = common::post(&fixture._fixture.application, &fixture.token, &format!("/api/threads/{}/semantic/entries", run.thread_id), json!({"key":"pending-entry","expected_revision":0,"source":{"kind":"message","id":message.id},"metadata":{}})).await;
	assert_eq!(status, 200, "{entry}");
	assert_eq!(entry["state"], "PENDING", "{entry}");
}

/// Make a scheduled retry due now, leaving its counter and reason as the
/// production path persisted them.
async fn advance_retry(fixture: &ControlledContext, run: Uuid) {
	sqlx::query(
		&reinhardt::query::Query::update()
			.table(reinhardt::query::Alias::new("task_executions"))
			.value_expr(
				reinhardt::query::Alias::new("pending"),
				Expr::cust(
					"JSONB_SET(pending, '{recovery,retry,at}', TO_JSONB(CLOCK_TIMESTAMP()))",
				),
			)
			.and_where(SimpleExpr::CustomWithExpr(
				"(id=? AND SET_CONFIG('aidash.input_ledger_worker','true',true)='true')".to_owned(),
				vec![Expr::value(run).into()],
			))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.execute(fixture.f.store.pool.fixture_driver())
	.await
	.unwrap();
}

/// Make an Outage Wait's next check due now, leaving what it tracks as the
/// production path persisted it.
async fn advance_outage_wake(fixture: &ControlledContext, run: Uuid) {
	sqlx::query(
		&reinhardt::query::Query::update()
			.table(reinhardt::query::Alias::new("task_executions"))
			.value_expr(
				reinhardt::query::Alias::new("pending"),
				Expr::cust("JSONB_SET(pending, '{data,wake_at}', TO_JSONB(CLOCK_TIMESTAMP()))"),
			)
			.and_where(SimpleExpr::CustomWithExpr(
				"(id=? AND SET_CONFIG('aidash.input_ledger_worker','true',true)='true')".to_owned(),
				vec![Expr::value(run).into()],
			))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.execute(fixture.f.store.pool.fixture_driver())
	.await
	.unwrap();
}

/// Run the worker until the model has received `count` requests.
async fn run_to_inference(fixture: &ControlledContext, count: usize) {
	for _ in 0..40 {
		if fixture.captured.lock().unwrap().len() >= count {
			return;
		}
		fixture.worker.worker_once().await.unwrap();
	}
	panic!(
		"model request {count} never arrived: {:?}",
		fixture.f.store.runs(&TEST_FIXTURE).await.unwrap()[0]
	);
}

/// The `semantic_memory` a Native model request carried in its current step.
fn request_semantic_memory(fixture: &ControlledContext, request: usize) -> Value {
	let contexts = fixture.captured.lock().unwrap();
	contexts[request - 1]["snapshot"]["semantic_memory"].clone()
}

/// Issue #236: a local Run reuses its semantic retrieval while its
/// Retrieval Key holds (ADR 0020), so an embedding outage after the first
/// retrieval does not touch the steps that reuse it. A Run message sent during
/// the outage changes the key's query inputs; only the step that then needs a
/// new retrieval follows the outage policy (#233). Once the provider recovers,
/// the retry retrieves under the new key and the next step reuses it.
#[rstest::rstest]
#[case::run_message(false)]
#[case::pending_entry(true)]
#[tokio::test]
async fn local_run_keeps_reusing_its_retrieval_through_an_embedding_outage(
	#[case] pending_entry: bool,
	#[future(awt)]
	#[with(false, true)]
	controlled_context: ControlledContext,
) {
	use aidash_server::domain::TaskStatus;
	let fixture = controlled_context;
	fixture
		.tool_rounds
		.store(4, std::sync::atomic::Ordering::Release);
	let run = || async { fixture.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0) };

	// Arrange: the first step retrieves while the provider is healthy.
	let healthy = fixture.fault.requests();
	run_to_inference(&fixture, 1).await;
	assert_eq!(
		fixture.fault.requests() - healthy,
		1,
		"step 1 embeds its query"
	);
	let retrieved = request_semantic_memory(&fixture, 1);
	assert!(
		retrieved["thread"]["matches"]
			.as_array()
			.is_some_and(|matches| !matches.is_empty()),
		"step 1 carries semantic context: {retrieved}"
	);

	// Act: the provider goes down; the next steps keep the same Retrieval Key.
	fixture.fault.set(Fault::Unavailable);
	let outage = fixture.fault.requests();
	for step in 2..=3 {
		run_to_inference(&fixture, step).await;
		// Assert: the reuse reaches inference with the same bytes, untouched.
		assert_eq!(
			request_semantic_memory(&fixture, step),
			retrieved,
			"step {step}"
		);
		let current = run().await;
		assert_eq!(
			current.control.as_str(),
			"ACTIVE",
			"step {step}: {current:?}"
		);
		assert!(current.recovery.retry.is_none(), "step {step}: {current:?}");
		assert!(
			serde_json::to_value(current.recovery.semantic_reason)
				.unwrap()
				.is_null(),
			"step {step}: {current:?}"
		);
	}
	assert_eq!(fixture.fault.requests(), outage, "a reuse embeds nothing");

	// A pending entry changes the corpus, just as a Run message changes the query.
	if pending_entry {
		add_pending_thread_entry(&fixture).await;
	} else {
		let (status, body) = common::post(
			&fixture._fixture.application,
			&fixture.token,
			&format!("/api/executions/{}/message", run().await.id),
			json!({"content":"Also cover vehicle maintenance."}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
	for _ in 0..40 {
		if run().await.recovery.retry.is_some() {
			break;
		}
		fixture.worker.worker_once().await.unwrap();
	}

	// Assert: the step that needs a new retrieval follows the outage policy.
	let paused = run().await;
	let retry = paused.recovery.retry.as_ref().unwrap_or_else(|| {
		panic!(
			"a scheduled retry; inferences={}, embeddings since outage={}, run={paused:?}",
			fixture.captured.lock().unwrap().len(),
			fixture.fault.requests() - outage,
		)
	});
	assert_eq!(retry.count, 1, "{paused:?}");
	assert_eq!(
		serde_json::to_value(paused.recovery.semantic_reason).unwrap(),
		"unavailable"
	);
	assert_eq!(paused.control.as_str(), "ACTIVE");
	assert_eq!(fixture.captured.lock().unwrap().len(), 3, "no inference");
	assert_eq!(
		fixture.fault.requests() - outage,
		usize::from(!pending_entry),
		"incomplete indexing makes no provider call"
	);
	assert_ne!(
		fixture
			.f
			.store
			.task(&TEST_FIXTURE, paused.task_id)
			.await
			.unwrap()
			.status,
		TaskStatus::Failed
	);

	// Act: the provider recovers and the retry is due.
	fixture.fault.clear();
	if pending_entry {
		aidash_server::semantic::worker::sweep(&fixture.f.store)
			.await
			.unwrap();
	}
	advance_retry(&fixture, paused.id).await;
	let recovered = fixture.fault.requests();
	run_to_inference(&fixture, 4).await;
	// Assert: the retry retrieves under the new key, and the next step reuses it.
	assert_eq!(
		fixture.fault.requests() - recovered,
		1,
		"the retry embeds its query"
	);
	assert!(run().await.recovery.retry.is_none());
	let refreshed = request_semantic_memory(&fixture, 4);
	run_to_inference(&fixture, 5).await;
	assert_eq!(
		fixture.fault.requests() - recovered,
		1,
		"step 5 reuses step 4"
	);
	assert_eq!(request_semantic_memory(&fixture, 5), refreshed);

	fixture.worker.federation.store.pool.close().await;
	fixture.worker.federation.store.control_pool.close().await;
	drop(fixture.model_server);
	drop(fixture.embedding_server);
	dispose(fixture.f, &fixture.url, &fixture.schema).await;
}

/// Issue #234 (ADR 0059): an opted-in thread source degrades after the
/// bounded retry. The step reaches inference with the typed marker, records the
/// degraded step and emits `execution.semantic_degraded`. Missing vector points need
/// repair, so they still pause even with the opt-in.
#[rstest::rstest]
#[case::embedding_outage(true)]
#[case::missing_points(false)]
#[tokio::test]
async fn an_opted_in_local_thread_outage_degrades_after_the_bounded_retry(
	#[case] embedding_outage: bool,
	#[future(awt)]
	#[with(false, true, true)]
	controlled_context: ControlledContext,
) {
	let fixture = controlled_context;
	if embedding_outage {
		fixture.fault.set(Fault::Unavailable);
	} else {
		let mut connection = fixture
			.worker
			.federation
			.store
			.pool
			.begin(&TEST_FIXTURE)
			.await
			.unwrap();
		aidash_server::database::native::query(
			&reinhardt::query::Query::delete()
				.from_table(reinhardt::query::Alias::new("semantic_vectors"))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut *connection)
		.await
		.unwrap();
		connection.commit().await.unwrap();
	}
	for attempt in 1..=6 {
		assert!(
			fixture.worker.worker_once().await.unwrap(),
			"attempt {attempt}"
		);
		let run = fixture.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
		if attempt < 6 {
			assert!(
				fixture.captured.lock().unwrap().is_empty(),
				"attempt {attempt}"
			);
			assert_eq!(
				run.recovery.retry.as_ref().unwrap().count,
				attempt,
				"{run:?}"
			);
			advance_retry(&fixture, run.id).await;
			continue;
		}
		if !embedding_outage {
			assert!(fixture.captured.lock().unwrap().is_empty());
			assert_eq!(run.control.as_str(), "PAUSED", "{run:?}");
			assert_eq!(
				serde_json::to_value(run.recovery.semantic_reason).unwrap(),
				"retries_exhausted"
			);
			assert!(run.recovery.degraded.is_empty(), "{run:?}");
			continue;
		}
		let contexts = fixture.captured.lock().unwrap().clone();
		assert_eq!(
			contexts.len(),
			1,
			"the degraded step reaches inference: {run:?}"
		);
		let semantic = &contexts[0]["snapshot"]["semantic_memory"];
		assert_eq!(
			semantic.get("thread").unwrap_or(semantic),
			&json!({"status":"unavailable","cause":"embedding_provider"}),
			"{}",
			contexts[0]
		);
		assert_ne!(run.control.as_str(), "PAUSED", "{run:?}");
		assert_eq!(run.recovery.degraded.count, 1, "{run:?}");
		assert_eq!(
			serde_json::to_value(&run.recovery.degraded.recent).unwrap(),
			json!([{"step":0,"cause":"embedding_provider"}])
		);
		let events = fixture
			.f
			.store
			.events(&TEST_FIXTURE, 0, Some(run.thread_id), 500)
			.await
			.unwrap();
		let degraded = events
			.iter()
			.find(|event| event.kind == "execution.semantic_degraded")
			.expect("execution.semantic_degraded");
		assert_eq!(degraded.data["cause"], "embedding_provider");
		assert_eq!(degraded.data["run_id"], json!(run.id));
	}

	fixture.worker.federation.store.pool.close().await;
	fixture.worker.federation.store.control_pool.close().await;
	drop(fixture.model_server);
	drop(fixture.embedding_server);
	dispose(fixture.f, &fixture.url, &fixture.schema).await;
}

/// A budget skip is not evidence that a previously unavailable index recovered.
#[rstest::rstest]
#[tokio::test]
async fn a_skipped_thread_read_keeps_the_durable_outage(
	#[future(awt)]
	#[with(false, true, true)]
	controlled_context: ControlledContext,
) {
	let fixture = controlled_context;
	let worker = Uuid::new_v4();
	let mut run = fixture
		.f
		.store
		.lease_run(worker, 30)
		.await
		.unwrap()
		.unwrap();
	run.recovery.degraded.record(
		run.step,
		aidash_domain::semantic::degraded::OutageCause::EmbeddingProvider,
	);
	fixture
		.f
		.store
		.save_run(&TEST_FIXTURE, &run, worker, "execution.semantic_degraded")
		.await
		.unwrap();
	fixture
		.f
		.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	let mut limited = spec(&fixture.embedding_server.url);
	limited.max_result_tokens = 128;
	limited.embedding.model = "m".repeat(256);
	limited.embedding.model_version = "v".repeat(128);
	configure(
		&fixture._fixture.application,
		&fixture.token,
		run.thread_id,
		&limited,
		1,
	)
	.await;
	let calls = fixture.fault.requests();
	assert!(fixture.worker.worker_once().await.unwrap());
	let saved = fixture.f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(
		fixture.fault.requests(),
		calls,
		"the budget skip must not query the provider"
	);
	assert!(
		saved.recovery.degraded.outage,
		"a skipped retrieval cannot recover the outage"
	);
	assert_eq!(
		fixture.captured.lock().unwrap().len(),
		1,
		"the optional thread slot still allows inference"
	);
	fixture.worker.federation.store.pool.close().await;
	fixture.worker.federation.store.control_pool.close().await;
	drop(fixture.model_server);
	drop(fixture.embedding_server);
	dispose(fixture.f, &fixture.url, &fixture.schema).await;
}

/// Issue #233: a rejected key (HTTP 401–403) or a response that breaks the
/// approved contract is not an outage. The local Run pauses at once with its
/// own typed reason and schedules no retry.
#[rstest::rstest]
#[case::key_rejected(Fault::Rejected, "configuration")]
#[case::provider_contract(Fault::Malformed, "provider_contract")]
#[tokio::test]
async fn local_semantic_key_rejection_and_contract_violation_pause_without_retry(
	#[case] fault: Fault,
	#[case] reason: &str,
	#[future(awt)]
	#[with(false, true)]
	controlled_context: ControlledContext,
) {
	use aidash_server::domain::TaskStatus;
	let fixture = controlled_context;
	let indexed = fixture.fault.requests();
	fixture.fault.set(fault);

	assert!(fixture.worker.worker_once().await.unwrap());
	let run = fixture.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);

	assert_eq!(fixture.fault.requests() - indexed, 1, "{reason}");
	assert!(fixture.captured.lock().unwrap().is_empty(), "{reason}");
	assert_eq!(run.control.as_str(), "PAUSED", "{run:?}");
	assert!(run.recovery.retry.is_none(), "{run:?}");
	assert_eq!(
		serde_json::to_value(run.recovery.semantic_reason).unwrap(),
		reason
	);
	assert_ne!(
		fixture
			.f
			.store
			.task(&TEST_FIXTURE, run.task_id)
			.await
			.unwrap()
			.status,
		TaskStatus::Failed
	);

	fixture.worker.federation.store.pool.close().await;
	fixture.worker.federation.store.control_pool.close().await;
	drop(fixture.model_server);
	drop(fixture.embedding_server);
	dispose(fixture.f, &fixture.url, &fixture.schema).await;
}

/// How the fixture embedding provider fails; it answers normally until one is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
	/// HTTP 503 from the provider.
	Unavailable = 1,
	/// The provider accepts the request and never answers within the client timeout.
	Hang = 2,
	/// HTTP 401: the provider rejects the configured key.
	Rejected = 3,
	/// HTTP 200 with a body that is not an embedding response.
	Malformed = 4,
}
#[derive(Default)]
struct EmbeddingFault {
	mode: std::sync::atomic::AtomicU8,
	requests: std::sync::atomic::AtomicUsize,
}
impl EmbeddingFault {
	fn set(&self, fault: Fault) {
		self.mode
			.store(fault as u8, std::sync::atomic::Ordering::Release);
	}
	fn clear(&self) {
		self.mode.store(0, std::sync::atomic::Ordering::Release);
	}
	fn requests(&self) -> usize {
		self.requests.load(std::sync::atomic::Ordering::Acquire)
	}
}
fn spec(endpoint: &str) -> IndexSpec {
	IndexSpec {
		embedding: EmbeddingConfig {
			provider: "openai".into(),
			endpoint: format!("{endpoint}/v1"),
			provider_credential: None,
			model: "fixture".into(),
			model_version: "1".into(),
			dimensions: 3,
			inputs: [aidash_domain::semantic::EmbeddingInputKind::Text]
				.into_iter()
				.collect(),
		},
		vector: VectorConfig {
			provider: "postgres".into(),
			endpoint: "local".into(),
			credential_env: None,
		},
		enabled: true,
		auto_context: true,
		max_sources: 64,
		max_results: 10,
		max_result_tokens: 4096,
		max_input_bytes: 8192,
		max_image_bytes: 8 * 1024 * 1024,
		max_images: 256,
	}
}
fn configure_request(spec: &IndexSpec, expected_revision: i64) -> Value {
	let mut options = serde_json::to_value(spec).unwrap();
	options.as_object_mut().unwrap().remove("embedding");
	json!({"expected_revision": expected_revision, "spec": options})
}
async fn configure(
	f: &aidash_server::federation::Federation,
	token: &str,
	thread: Uuid,
	spec: &IndexSpec,
	revision: i64,
) -> Value {
	// The provider-under-test uses a deliberately isolated configuration.
	// HTTP index settings always derive their model from the Tenant.
	let actor = aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	}
	.authenticate(token)
	.await
	.unwrap();
	serde_json::to_value(
		semantic::service::configure(
			&f.store,
			&actor,
			thread,
			ConfigureIndex {
				expected_revision: revision,
				acknowledge_memory_embeddings: false,
				spec: spec.clone(),
			},
		)
		.await
		.unwrap(),
	)
	.unwrap()
}

async fn put(
	app: &common::TestApplication,
	token: &str,
	thread: Uuid,
	key: &str,
	text: &str,
	revision: i64,
) -> Value {
	let (status,entry)=common::post(app, token, &format!("/api/threads/{thread}/semantic/entries"), json!({"key":key,"source":{"kind":"memory","text":text},"expected_revision":revision,"metadata":{"topic":"public"}})).await;
	assert_eq!(status, 200, "{entry}");
	entry
}
async fn search(app: &common::TestApplication, token: &str, thread: Uuid) -> (u16, Value) {
	common::post(
		app,
		token,
		&format!("/api/threads/{thread}/semantic/search"),
		json!({"query":"vehicle","limit":1,"max_tokens":2048}),
	)
	.await
}
async fn dispose(f: aidash_server::federation::Federation, url: &str, schema: &str) {
	let rows: Vec<(String, Value)> = sqlx::query_as(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::SimpleExpr::from(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("collection")),
			))
			.expr(reinhardt::query::SimpleExpr::from(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("vector")),
			))
			.from(reinhardt::query::Alias::new("semantic_collections"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_all(f.store.pool.fixture_driver())
	.await
	.unwrap();
	for (collection, config) in rows {
		semantic::backend::delete_collection(
			&f.store,
			&serde_json::from_value(config).unwrap(),
			&collection,
		)
		.await
		.unwrap();
	}
	common::cleanup(f, url, schema).await;
}

#[rstest::rstest]
#[case::openai("openai")]
#[case::openrouter("openrouter")]
#[tokio::test]
async fn semantic_lifecycle_is_durable_revisioned_and_not_keyword_search(
	#[case] provider: &str,
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let endpoint = embeddings.url.clone();
	let server = embeddings;
	let (_, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let thread = f.store.task(&TEST_FIXTURE, task).await.unwrap().thread_id;
	let mut config = spec(&endpoint);
	config.embedding.provider = provider.into();
	let first = configure(&f, &token, thread, &config, 0).await;
	assert_eq!(
		configure(&f, &token, thread, &config, 0).await["revision"],
		1
	);
	let cars = put(&app, &token, thread, "cars", "A car carries passengers.", 0).await;
	assert_eq!(
		put(&app, &token, thread, "cars", "A car carries passengers.", 0).await["id"],
		cars["id"]
	);
	put(
		&app,
		&token,
		thread,
		"bread",
		"Bread is baked in an oven.",
		0,
	)
	.await;
	assert_eq!(
		search(&app, &token, thread).await.0,
		409,
		"pending data is never a successful empty search"
	);
	semantic::worker::sweep(&f.store).await.unwrap();
	let (status, result) = search(&app, &token, thread).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(result["matches"][0]["entry_id"], cars["id"]);
	assert_eq!(result["matches"][0]["source"], json!({"kind":"memory"}));
	assert!(
		!result["matches"][0]["text"]
			.as_str()
			.unwrap()
			.contains("vehicle")
	);
	assert_eq!(result["model_version"], "1");
	// Replace the source using an optimistic revision. The old point must not
	// remain eligible even before the vector deletion has run.
	let updated = put(&app, &token, thread, "cars", "A car uses roads safely.", 1).await;
	assert_eq!(updated["revision"], 2);
	assert_eq!(search(&app, &token, thread).await.0, 409);
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(
		search(&app, &token, thread).await.1["matches"][0]["revision"],
		2
	);
	config.embedding.model_version = "2".into();
	let second = configure(&f, &token, thread, &config, 1).await;
	assert_ne!(first["collection"], second["collection"]);
	assert_eq!(search(&app, &token, thread).await.0, 409);
	// Fresh pools reconstruct authority and jobs entirely from durable state.
	let restarted = f.for_workers().await.unwrap();
	semantic::worker::sweep(&restarted.store).await.unwrap();
	assert_eq!(search(&app, &token, thread).await.1["model_version"], "2");
	restarted.store.pool.close().await;
	restarted.store.control_pool.close().await;
	let current_points = request(
		&app,
		&token,
		"GET",
		&format!("/api/threads/{thread}/semantic/entries"),
		json!(null),
	)
	.await
	.1;
	let current_point = current_points["entries"]
		.as_array()
		.unwrap()
		.iter()
		.find(|e| e["id"] == cars["id"])
		.unwrap()["point_id"]
		.clone();
	let path = format!(
		"/api/threads/{thread}/semantic/entries/{}",
		cars["id"].as_str().unwrap()
	);
	assert_eq!(
		request(
			&app,
			&token,
			"DELETE",
			&path,
			json!({"expected_revision":2})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&app,
			&token,
			"DELETE",
			&path,
			json!({"expected_revision":2})
		)
		.await
		.0,
		200
	);
	let after = search(&app, &token, thread).await;
	assert_eq!(after.0, 200, "{}", after.1);
	assert_ne!(after.1["matches"][0]["entry_id"], cars["id"]);
	semantic::worker::sweep(&f.store).await.unwrap();
	use aidash_application::ports::VectorIndex;
	assert!(
		!aidash_server::bootstrap::semantic_transport(&f.store)
			.present(
				&config.vector,
				second["collection"].as_str().unwrap(),
				second["tenant"].as_str().unwrap(),
				&[serde_json::from_value(current_point.clone()).unwrap()]
			)
			.await
			.unwrap()
	);
	// The private-network permission is not a hosted-model declaration. A
	// Tenant that forbids Keyless endpoints stops query embeddings from their next send.
	{
		use aidash_server::apps::identity::models::TenantDataProtection;
		use reinhardt::db::orm::Model;
		let policy = TenantDataProtection::build()
			.tenant("acme")
			.level("no_training")
			.forbid_plan_connections(false)
			.forbid_keyless_endpoints(true)
			.updated_at(chrono::Utc::now())
			.updated_by("fixture")
			.finish();
		f.store
			.pool
			.atomic(&TEST_FIXTURE, async |tx| {
				TenantDataProtection::objects()
					.insert_with_executor(tx, &policy)
					.await
					.map_err(reinhardt::core::exception::Error::from)?;
				Ok(())
			})
			.await
			.unwrap();
	}
	let refused = search(&app, &token, thread).await;
	assert_eq!(refused.0, 400, "{}", refused.1);
	assert!(
		refused.1.to_string().contains("forbids Keyless endpoints"),
		"{}",
		refused.1
	);
	drop(server);
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_access_is_checked_before_search_and_jobs_retain_revocation(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let endpoint = embeddings.url.clone();
	let server = embeddings;
	let (mut policy, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let thread = f.store.task(&TEST_FIXTURE, task).await.unwrap().thread_id;
	configure(&f, &token, thread, &spec(&endpoint), 0).await;
	let cars = put(&app, &token, thread, "cars", "A car carries passengers.", 0).await;
	semantic::worker::sweep(&f.store).await.unwrap();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"revoke-semantic","effect":"deny","subjects":{"ids":["alice"]},"actions":["semantic.read"],"resources":{"kinds":["semantic"]}}));
	let (status, response) = common::post(
		&app,
		&f.config.api_token,
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	let reindex_path = format!(
		"/api/threads/{thread}/semantic/entries/{}/reindex",
		cars["id"].as_str().unwrap()
	);
	assert_eq!(
		common::post(&app, &token, &reindex_path, json!({"expected_revision":1}))
			.await
			.0,
		403,
		"write-only permission cannot disclose a stored source in mutation responses"
	);
	// No allowed candidates means no embedding request is necessary, even when
	// the embedding backend is down. The caller cannot infer denied sources.
	drop(server);
	let response = search(&app, &token, thread).await;
	assert_eq!(response.0, 200, "{}", response.1);
	assert_eq!(response.1["matches"], json!([]));
	// Credentials are listed and revoked by the host here: the HTTP routes need
	// a Tenant Administrator Mapping in a browser (#484).
	let auth = aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	};
	let credential = auth
		.credentials("acme")
		.await
		.unwrap()
		.into_iter()
		.find(|c| c.subject == "alice")
		.unwrap();
	auth.revoke_credential("acme", credential.id).await.unwrap();
	{
		let query_bind_1 = Uuid::parse_str(cars["id"].as_str().unwrap()).unwrap();
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("semantic_entries"))
				.value_expr(
					reinhardt::query::Alias::new("next_attempt"),
					reinhardt::query::Expr::cust("CLOCK_TIMESTAMP()"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	semantic::worker::sweep(&f.store).await.unwrap();
	let state: String = {
		let query_bind_1 = Uuid::parse_str(cars["id"].as_str().unwrap()).unwrap();
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("state")),
				))
				.from(reinhardt::query::Alias::new("semantic_entries"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(state, "REVOKED");
	assert_eq!(search(&app, &token, thread).await.0, 401);
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_outage_and_input_bounds_are_visible_and_retriable(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let endpoint = embeddings.url.clone();
	let server = embeddings;
	let (_, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let thread = f.store.task(&TEST_FIXTURE, task).await.unwrap().thread_id;
	let mut config = spec(&endpoint);
	config.embedding.dimensions = 4; // Provider's actual width is three.
	configure(&f, &token, thread, &config, 0).await;
	let cars = put(&app, &token, thread, "cars", "A car carries passengers.", 0).await;
	semantic::worker::sweep(&f.store).await.unwrap();
	let entries = request(
		&app,
		&token,
		"GET",
		&format!("/api/threads/{thread}/semantic/entries"),
		json!(null),
	)
	.await;
	assert_eq!(entries.0, 200);
	assert_eq!(entries.1["entries"][0]["state"], "ERROR");
	assert_eq!(entries.1["entries"][0]["attempts"], 1);
	assert_eq!(search(&app, &token, thread).await.0, 409);
	config.embedding.dimensions = 3;
	configure(&f, &token, thread, &config, 1).await;
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(search(&app, &token, thread).await.0, 200);
	let reindex_path = format!(
		"/api/threads/{thread}/semantic/entries/{}/reindex",
		cars["id"].as_str().unwrap()
	);
	let first_reindex =
		common::post(&app, &token, &reindex_path, json!({"expected_revision":1})).await;
	let replay_reindex =
		common::post(&app, &token, &reindex_path, json!({"expected_revision":1})).await;
	assert_eq!(first_reindex.0, 200);
	assert_eq!(replay_reindex.0, 200);
	assert_eq!(first_reindex.1["point_id"], replay_reindex.1["point_id"]);
	semantic::worker::sweep(&f.store).await.unwrap();
	let (status, _) = common::post(
		&app,
		&token,
		&format!("/api/threads/{thread}/semantic/search"),
		json!({"query":"vehicle","limit":999,"max_tokens":2048}),
	)
	.await;
	assert_eq!(status, 400);
	let (status,_)=common::post(&app, &token, &format!("/api/threads/{thread}/semantic/entries"), json!({"key":"cars","expected_revision":99,"source":{"kind":"memory","text":"changed"},"metadata":{}})).await;
	assert_eq!(status, 409);
	drop(server);
	let response = search(&app, &token, thread).await;
	assert_eq!(response.0, 503, "embedding outage must fail visibly");
	assert!(response.1["matches"].is_null());
	assert!(!response.1.to_string().contains(&endpoint));
	assert!(!cars["id"].is_null());
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_context_is_provenanced_and_revocation_hides_run_journals(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)]
	#[from(semantic_context_is_provenanced_and_revocation_hides_run_journals_provider)]
	fixture: SemanticContextIsProvenancedAndRevocationHidesRunJournalsProvider,

	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	let captured = fixture.state.captured;
	let server = fixture.server;

	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let embedding_endpoint = embeddings.url.clone();
	// The embedding fixture is a Keyless endpoint on loopback (ADR 0036).
	f.store
		.keyless
		.permit_origin_of(&embedding_endpoint)
		.unwrap();
	let embedding_server = embeddings;

	let endpoint = server.url.clone();
	let (mut policy, token, task) = common::bootstrap_with_context(&f, &app, &endpoint, true).await;
	let thread = f.store.task(&TEST_FIXTURE, task).await.unwrap().thread_id;
	configure(&f, &token, thread, &spec(&embedding_endpoint), 0).await;
	let cars = put(&app, &token, thread, "cars", "A car carries passengers.", 0).await;
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(
		common::post(
			&app,
			&token,
			&format!("/api/tasks/{task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		200
	);
	let worker = aidash_server::harness::Harness {
		federation: f.for_workers().await.unwrap(),
	};
	worker.worker_once().await.unwrap();
	worker.worker_once().await.unwrap();
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	assert_eq!(run.phase().as_str(), "TOOL_CALL", "{:?}", run.error);
	let context = captured.lock().unwrap()[0].clone();
	assert_eq!(
		context["snapshot"]["semantic_memory"]["thread"]["matches"][0]["entry_id"],
		cars["id"]
	);
	assert_eq!(
		context["snapshot"]["semantic_memory"]["thread"]["model_version"],
		"1"
	);
	let count: i64 = {
		let query_bind_1 = run.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new("semantic_run_reads"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(run_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(count, 1);
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-inline-semantic-memory","effect":"deny","subjects":{"ids":["alice"]},"actions":["semantic.read"],"resources":{"kinds":["semantic"]}}));
	assert_eq!(
		common::post(
			&app,
			&f.config.api_token,
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let visible = request(
		&app,
		&token,
		"GET",
		&format!("/api/threads/{thread}/semantic/entries"),
		json!(null),
	)
	.await
	.1;
	assert_eq!(
		visible["entries"].as_array().unwrap().len(),
		0,
		"inline source reads require the semantic entry permission"
	);
	policy["policies"].as_array_mut().unwrap().pop();
	assert_eq!(
		common::post(
			&app,
			&f.config.api_token,
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let path = format!(
		"/api/threads/{thread}/semantic/entries/{}",
		cars["id"].as_str().unwrap()
	);
	assert_eq!(
		request(
			&app,
			&token,
			"DELETE",
			&path,
			json!({"expected_revision":1})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&app,
			&token,
			"GET",
			&format!("/api/executions/{}", run.id),
			json!(null)
		)
		.await
		.0,
		403
	);
	worker.worker_once().await.unwrap();
	assert_eq!(
		f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.control
			.as_str(),
		"PAUSED"
	);
	assert_eq!(captured.lock().unwrap().len(), 1);
	worker.federation.store.pool.close().await;
	worker.federation.store.control_pool.close().await;
	drop(server);
	drop(embedding_server);
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn linked_sources_and_agent_metadata_filters_respect_original_authority(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	use aidash_server::domain::{ArtifactInput, qualified_agent};
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let endpoint = embeddings.url.clone();
	let server = embeddings;
	let (mut policy, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let thread = f.store.task(&TEST_FIXTURE, task).await.unwrap().thread_id;
	configure(&f, &token, thread, &spec(&endpoint), 0).await;
	assert_eq!(
		common::post(
			&app,
			&token,
			&format!("/api/tasks/{task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		200
	);
	aidash_server::harness::Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	let artifact = f
		.store
		.publish_artifact(
			&TEST_FIXTURE,
			task,
			&qualified_agent(
				&aidash_domain::nodes::NodeAddress::derived(f.config.deployment.as_str(), "acme"),
				"research",
				"1.0.0",
			),
			"semantic-artifact",
			&ArtifactInput {
				kind: "text".into(),
				name: "Car report".into(),
				content: json!("A car has a steering wheel."),
			},
		)
		.await
		.unwrap();
	f.store
		.message(
			&TEST_FIXTURE,
			thread,
			"human",
			"Vehicle safety",
			Some("semantic-vehicle-safety"),
		)
		.await
		.unwrap();
	let message = f
		.store
		.snapshot(&TEST_FIXTURE, thread)
		.await
		.unwrap()
		.messages[0]
		.id;
	let mut ids = vec![];
	for (kind, id, agent) in [
		("artifact", artifact.id, None),
		("message", message, Some("aidash://private/agent@1.0.0")),
	] {
		let (status,entry)=common::post(&app, &token, &format!("/api/threads/{thread}/semantic/entries"), json!({"key":kind,"expected_revision":0,"source":{"kind":kind,"id":id},"agent":agent,"metadata":{"topic":"transport"}})).await;
		assert_eq!(status, 200, "{entry}");
		ids.push(entry);
	}
	semantic::worker::sweep(&f.store).await.unwrap();
	let found = search(&app, &token, thread).await;
	assert_eq!(found.0, 200, "{}", found.1);
	assert_eq!(
		found.1["matches"][0]["entry_id"], ids[0]["id"],
		"agent-scoped data is not implicitly included"
	);
	let path = format!("/api/threads/{thread}/semantic/search");
	let (_,found)=common::post(&app, &token, &path, json!({"query":"vehicle","agent":"aidash://private/agent@1.0.0","metadata":{"topic":"transport"},"limit":10,"max_tokens":4096})).await;
	assert_eq!(found["matches"].as_array().unwrap().len(), 2);
	let (_, filtered) = common::post(
		&app,
		&token,
		&path,
		json!({"query":"vehicle","metadata":{"topic":"cooking"},"limit":10,"max_tokens":4096}),
	)
	.await;
	assert_eq!(filtered["matches"], json!([]));
	// An out-of-band source update invalidates a READY vector immediately. A
	// background sweep allocates a new revision and then indexes the new text.
	{
		let query_bind_1 = artifact.id;
		let query_bind_2 = json!("A car needs brakes.");
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("artifacts"))
				.value_expr(
					reinhardt::query::Alias::new("content"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(search(&app, &token, thread).await.0, 409);
	sqlx::query(
		&reinhardt::query::Query::update()
			.table(reinhardt::query::Alias::new("semantic_entries"))
			.value_expr(
				reinhardt::query::Alias::new("next_attempt"),
				reinhardt::query::Expr::cust("CLOCK_TIMESTAMP()"),
			)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	semantic::worker::sweep(&f.store).await.unwrap();
	semantic::worker::sweep(&f.store).await.unwrap();
	assert!(
		search(&app, &token, thread).await.1["matches"][0]["text"]
			.as_str()
			.unwrap()
			.contains("brakes")
	);
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-original-artifact","effect":"deny","subjects":{"ids":["alice"]},"actions":["artifact.read"],"resources":{"kinds":["artifact"],"ids":[artifact.id.to_string()]}}));
	assert_eq!(
		common::post(
			&app,
			&f.config.api_token,
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(search(&app, &token, thread).await.1["matches"], json!([]));
	let (_, entries) = request(
		&app,
		&token,
		"GET",
		&format!("/api/threads/{thread}/semantic/entries"),
		json!(null),
	)
	.await;
	assert!(!entries.to_string().contains(&artifact.id.to_string()));
	// A Subject cannot configure or attach a resource across Tenants either.
	let other_tenant = common::tenant_scope(&f.store, "other").await;
	let other = f
		.store
		.create_thread(&other_tenant, "bob", "Other")
		.await
		.unwrap();
	assert_eq!(
		common::post(
			&app,
			&token,
			&format!("/api/threads/{}/semantic/index", other.id),
			configure_request(&spec(&endpoint), 0)
		)
		.await
		.0,
		403
	);
	assert_eq!(common::post(&app, &token, &format!("/api/threads/{}/semantic/entries",other.id), json!({"key":"forged","expected_revision":0,"source":{"kind":"artifact","id":artifact.id},"metadata":{}})).await.0,403);
	assert_eq!(search(&app, &token, other.id).await.0, 403);
	drop(server);
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_postgres_restart_and_lost_points_recover(
	#[from(common::isolated_test_environment)] environment: futures_util::future::Shared<
		futures_util::future::BoxFuture<'static, Arc<common::TestEnvironment>>,
	>,
	#[future(awt)]
	#[from(isolated_application)]
	#[with(environment.clone())]
	application_fixture: common::ApplicationFixture,
	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	use aidash_application::ports::VectorIndex;
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let embedding = embeddings.url.clone();
	let server = embeddings;
	let (_, token, task) = common::bootstrap(&f, &app, &embedding).await;
	let thread = f.store.task(&TEST_FIXTURE, task).await.unwrap().thread_id;
	let config = spec(&embedding);
	let index = configure(&f, &token, thread, &config, 0).await;
	let entry = put(
		&app,
		&token,
		thread,
		"restart",
		"A car carries passengers.",
		0,
	)
	.await;
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(search(&app, &token, thread).await.0, 200);
	// Act: restart the exact PostgreSQL instance owned by the application fixture.
	environment.await.restart_database().await;
	assert_eq!(
		search(&app, &token, thread).await.1["matches"][0]["entry_id"],
		entry["id"],
		"source and acknowledged vector survive the same PostgreSQL restart"
	);
	let transport = aidash_server::bootstrap::semantic_transport(&f.store);
	transport
		.delete_point(
			&config.vector,
			index["collection"].as_str().unwrap(),
			Uuid::parse_str(entry["point_id"].as_str().unwrap()).unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(
		search(&app, &token, thread).await.0,
		503,
		"missing vectors cannot masquerade as empty retrieval"
	);
	sqlx::query(
		&reinhardt::query::Query::update()
			.table(reinhardt::query::Alias::new("semantic_entries"))
			.value_expr(
				reinhardt::query::Alias::new("next_attempt"),
				reinhardt::query::Expr::cust("CLOCK_TIMESTAMP()"),
			)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(search(&app, &token, thread).await.0, 200);
	drop(server);
	dispose(f, &url, &schema).await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;

use reinhardt::query::Expr;

#[rstest::fixture]
fn embedding_fault() -> std::sync::Arc<EmbeddingFault> {
	std::sync::Arc::new(EmbeddingFault::default())
}
#[rstest::fixture]
fn embeddings_router(
	#[from(embedding_fault)] fault: std::sync::Arc<EmbeddingFault>,
) -> std::sync::Arc<Router> {
	use std::sync::atomic::Ordering;
	std::sync::Arc::new(reinhardt::test::stub::StubRouter::new()
.route("/v1/embeddings", http::Method::POST, reply(move |request: reinhardt::Request| {let input = request.json::<Value>().unwrap();let fault = fault.clone();async move {
            fault.requests.fetch_add(1, Ordering::AcqRel);
            match fault.mode.load(Ordering::Acquire) {
                mode if mode == Fault::Unavailable as u8 => return reinhardt::Response::new(http::StatusCode::SERVICE_UNAVAILABLE).with_json(&json!({"error":"fixture outage"})).unwrap(),
                mode if mode == Fault::Hang as u8 => tokio::time::sleep(std::time::Duration::from_secs(120)).await,
                mode if mode == Fault::Rejected as u8 => return reinhardt::Response::new(http::StatusCode::UNAUTHORIZED).with_json(&json!({"error":"fixture key rejected"})).unwrap(),
                mode if mode == Fault::Malformed as u8 => return reinhardt::Response::ok().with_json(&json!({"unexpected":true})).unwrap(),
                _ => {}
            }
            let text=input["input"].as_str().unwrap_or_default().to_lowercase();
            // Explicit fixture semantics: related concepts share a vector even
            // when the query and stored text have no common keyword.
            let vector=if text.contains("car") || text.contains("vehicle") {vec![1.0,0.0,0.0]} else if text.contains("bread") || text.contains("baking") {vec![0.0,1.0,0.0]} else {vec![0.0,0.0,1.0]};
            reinhardt::Response::ok().with_json(&json!({"model":input["model"],"data":[{"index":0,"embedding":vector}],"usage":{"prompt_tokens":1,"total_tokens":1}})).unwrap()
        }})).into_server_router())
}
#[rstest::fixture]
async fn embeddings(
	#[from(embeddings_router)] _embeddings_router: std::sync::Arc<Router>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_embeddings_router.clone())]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) -> std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard> {
	std::sync::Arc::new(server)
}

#[rstest::fixture]
fn isolated_application(
	#[from(common::isolated_test_environment)] environment: futures_util::future::Shared<
		futures_util::future::BoxFuture<'static, std::sync::Arc<common::TestEnvironment>>,
	>,
	#[from(common::execution_database)]
	#[with(environment.clone())]
	database: common::DatabaseFuture,
	#[from(common::runtime)]
	#[with(database.clone())]
	runtime: common::RuntimeFuture,
	#[from(common::native_application)]
	#[with(Default::default(),aidash_server::sse::Service::new(Default::default()),Arc::new(|r|r),runtime.clone())]
	application: common::ApplicationFuture,
) -> common::ApplicationFuture {
	let _ = (environment, database, runtime);
	application
}

#[derive(Clone)]
struct ControlledContextState {
	captured: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
	selector_calls: Arc<Mutex<Vec<Value>>>,
	tool_rounds: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
#[fixture]
fn controlled_context_state() -> ControlledContextState {
	ControlledContextState {
		captured: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
		selector_calls: Default::default(),
		tool_rounds: Default::default(),
	}
}
#[fixture]
fn controlled_context_router(
	#[from(controlled_context_state)] state: ControlledContextState,
) -> std::sync::Arc<Router> {
	let captured = state.captured.clone();
	let received = captured.clone();
	let tool_rounds = state.tool_rounds.clone();
	let selector_calls = state.selector_calls.clone();
	std::sync::Arc::new(reinhardt::test::stub::StubRouter::new()
.route("/systemone", http::Method::POST, reply(move |request: reinhardt::Request| {
	let calls = selector_calls.clone();
	async move { source_selection::answer(request.json::<Value>().unwrap(), &calls) }
}))
.route("/v1/chat/completions", http::Method::POST, reply(move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();
			let received = received.clone();
			let tool_rounds = tool_rounds.clone();
			async move {
				let context = common::captured_context(&body);
				let count = {
					let mut received = received.lock().unwrap();
					received.push(context);
					received.len()
				};
				let message = if count <= tool_rounds.load(std::sync::atomic::Ordering::Acquire) {
					json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("observe-{count}"),"type":"function","function":{"name":"workspace_observe","arguments":"{}"}}]})
				} else {
					json!({"content":"Complete"})
				};
				let finish = if message.get("tool_calls").is_some() { "tool_calls" } else { "stop" };
				reinhardt::Response::ok().with_json(&json!({"choices":[{"finish_reason":finish,"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
			}
		})).into_server_router())
}
struct ControlledContextProvider {
	state: ControlledContextState,
	server: reinhardt::test::fixtures::server::TestServerGuard,
}
#[fixture]
async fn controlled_context_provider(
	#[from(controlled_context_state)] state: ControlledContextState,
	#[from(controlled_context_router)]
	#[with(state.clone())]
	_router: std::sync::Arc<Router>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_router.clone())]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) -> ControlledContextProvider {
	ControlledContextProvider { state, server }
}

#[derive(Clone)]
struct SemanticContextIsProvenancedAndRevocationHidesRunJournalsState {
	captured: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
}
#[fixture]
fn semantic_context_is_provenanced_and_revocation_hides_run_journals_state()
-> SemanticContextIsProvenancedAndRevocationHidesRunJournalsState {
	SemanticContextIsProvenancedAndRevocationHidesRunJournalsState {
		captured: Arc::new(Mutex::new(Vec::<Value>::new())),
	}
}
#[fixture]
fn semantic_context_is_provenanced_and_revocation_hides_run_journals_router(
	#[from(semantic_context_is_provenanced_and_revocation_hides_run_journals_state)]
	state: SemanticContextIsProvenancedAndRevocationHidesRunJournalsState,
) -> std::sync::Arc<Router> {
	let captured = state.captured.clone();
	let requests = captured.clone();
	std::sync::Arc::new(reinhardt::test::stub::StubRouter::new()
.route("/v1/chat/completions", http::Method::POST, reply(move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();
            let requests=requests.clone();
            async move {
                let context:Value=crate::common::captured_context(&body);
                requests.lock().unwrap().push(context);
                reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"remember-car","type":"function","function":{"name":"workspace_observe","arguments":"{}"}}]}}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
            }
        })).into_server_router())
}
struct SemanticContextIsProvenancedAndRevocationHidesRunJournalsProvider {
	state: SemanticContextIsProvenancedAndRevocationHidesRunJournalsState,
	server: reinhardt::test::fixtures::server::TestServerGuard,
}
#[fixture]
async fn semantic_context_is_provenanced_and_revocation_hides_run_journals_provider(
	#[from(semantic_context_is_provenanced_and_revocation_hides_run_journals_state)]
	state: SemanticContextIsProvenancedAndRevocationHidesRunJournalsState,
	#[from(semantic_context_is_provenanced_and_revocation_hides_run_journals_router)]
	#[with(state.clone())]
	_router: std::sync::Arc<Router>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_router.clone())]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) -> SemanticContextIsProvenancedAndRevocationHidesRunJournalsProvider {
	SemanticContextIsProvenancedAndRevocationHidesRunJournalsProvider { state, server }
}

/// Neither Deployment Administrator form (the shared bearer or a browser
/// context) nor another Tenant's Subject configures, reads or mutates a
/// Tenant's Semantic Index or its knowledge (#484).
#[rstest::rstest]
#[tokio::test]
async fn deployment_and_other_tenant_authority_never_reaches_semantic_index_or_knowledge(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	// Arrange: a configured index with one stored entry, a browser administrator
	// and a Subject of a second Tenant.
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let endpoint = embeddings.url.clone();
	let server = embeddings;
	let (_, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let thread = f.store.task(&TEST_FIXTURE, task).await.unwrap().thread_id;
	let config = spec(&endpoint);
	configure(&f, &token, thread, &config, 0).await;
	let entry = put(&app, &token, thread, "cars", "A car carries passengers.", 0).await;
	let browser = common::BrowserAdministrator::new(&f).await;
	common::tenant_root(&f, "other").await;
	assert_eq!(
		common::post(&app, &f.config.api_token, "/api/authorization/other", json!({"expected_revision":0,"bundle":{"tenant":"other","subjects":{"bob":{"kind":"user"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]}})).await.0,
		200
	);
	let bob = common::subject_credential(&f.store, "other", "bob").await;
	let index = format!("/api/threads/{thread}/semantic/index");
	let entries = format!("/api/threads/{thread}/semantic/entries");
	let mut reconfigured = config.clone();
	reconfigured.max_results = 1;
	let reconfigure = configure_request(&reconfigured, 1);
	let overwrite = json!({"key":"cars","expected_revision":1,"source":{"kind":"memory","text":"overwritten"},"metadata":{}});
	let routes = [
		("GET", index.clone(), Value::Null),
		("POST", index.clone(), reconfigure),
		("GET", entries.clone(), Value::Null),
		("POST", entries.clone(), overwrite),
		(
			"GET",
			format!("/api/threads/{thread}/semantic/cleanup"),
			Value::Null,
		),
		(
			"POST",
			format!("/api/threads/{thread}/memory/operate"),
			json!({}),
		),
		(
			"POST",
			format!("/api/threads/{thread}/memory/units/mutate"),
			json!({}),
		),
	];

	// Act and Assert
	for (method, path, body) in &routes {
		let (status, response) =
			request(&app, &f.config.api_token, method, path, body.clone()).await;
		assert_eq!(
			status, 403,
			"bearer administrator {method} {path}: {response}"
		);
		let (status, response) = browser.send(method, path, body.clone()).await;
		assert_eq!(
			status, 403,
			"browser administrator {method} {path}: {response}"
		);
		// A rejection of the empty probe body is plain text, so read the status only.
		let mut probe = app
			.api_http
			.request(method.parse().unwrap(), path)
			.header("Authorization", format!("Bearer {bob}"));
		if *method == "POST" {
			probe = probe.json(body);
		}
		let status = probe.send().await.unwrap().status_code();
		assert!(
			matches!(status, 400 | 403 | 404 | 422),
			"other Tenant {method} {path}: {status}"
		);
	}
	// The Tenant's own Subject still reads, and nothing changed.
	let (status, current) = common::get(&app, &token, &index).await;
	assert_eq!(status, 200, "{current}");
	assert_eq!(current["revision"], 1);
	assert_eq!(current["spec"]["max_results"], 10);
	let (status, stored) = common::get(&app, &token, &entries).await;
	assert_eq!(status, 200, "{stored}");
	assert_eq!(stored["entries"].as_array().unwrap().len(), 1);
	assert_eq!(stored["entries"][0]["id"], entry["id"]);
	drop(server);
	dispose(f, &url, &schema).await;
}
