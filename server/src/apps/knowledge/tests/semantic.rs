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

struct ControlledContext {
	_fixture: common::ApplicationFixture,
	f: aidash_server::federation::Federation,
	worker: aidash_server::harness::Harness,
	url: String,
	schema: String,
	/// The Task owner's token.
	token: String,
	captured: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
	/// Tool-call responses before the model stops, for Runs that need several inferences.
	tool_rounds: std::sync::Arc<std::sync::atomic::AtomicUsize>,
	model_server: reinhardt::test::fixtures::server::TestServerGuard,
	embedding_server: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	fault: std::sync::Arc<EmbeddingFault>,
}

/// A runtime whose node holds a Cache Salt Key when `ordered`, so it can create
/// Runs on the Ordered Projection Version.
#[rstest::fixture]
fn context_runtime(
	#[default(false)] ordered: bool,
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> common::RuntimeFuture {
	use futures_util::FutureExt as _;
	async move {
		let mut owner = runtime.await;
		if ordered {
			// A node without Cache Salt Keys creates only Legacy Runs.
			let store = &mut owner.federation.store;
			*store = store.clone().with_cache_salt(Some(
				aidash_integrations::inference::CacheSaltKeys::new(
					&[aidash_integrations::inference::CacheSaltKey {
						version: 1,
						secret: "fixture-cache-salt-secret".into(),
					}],
					1,
				)
				.unwrap(),
			));
		}
		owner
	}
	.boxed()
	.shared()
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
		#[default(false)] ordered: bool,
		#[default(false)] degrade: bool,
		#[from(context_runtime)]
		#[with(ordered)]
		_runtime: common::RuntimeFuture,
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
		let tool_rounds = fixture.state.tool_rounds;
		let model_server = fixture.server;

		use aidash_server::domain::{ArtifactInput, qualified_agent};
		let (f, url, schema) = application_fixture.runtime.parts();
		let app = application_fixture.application.clone();

		let endpoint = model_server.url.clone();
		let (mut policy, token, task) =
			common::bootstrap_with_context(&f, &app, &endpoint, true).await;
		let acme = aidash_domain::tenancy::TenantName::new("acme").unwrap();
		let mut agent = f.registry.get(&acme, "research", "1.0.0").await.unwrap();
		agent.version = "1.0.1".into();
		agent.binding_normalization = None;
		if ordered {
			// The bootstrap model declares only Legacy; an Ordered Agent needs a
			// model that also declares Ordered.
			let version = agent.config["model"]["version"]
				.as_str()
				.unwrap()
				.to_owned();
			let mut model = f
				.registry
				.get(
					&acme,
					agent.config["model"]["id"].as_str().unwrap(),
					&version,
				)
				.await
				.unwrap();
			model.id = "ordered-model".into();
			model.binding_normalization = None;
			model.config["projection_versions"] = json!(["legacy", "ordered"]);
			f.registry.register(&acme, model).await.unwrap();
			assert_eq!(
				request(
					&app,
					&token,
					"POST",
					"/api/authorization/acme/catalog",
					json!({"entry":{"id":"ordered-model","version":version},"expected_revision":0,"enabled":true})
				)
				.await
				.0,
				200
			);
			agent.config["model"] = json!({"id":"ordered-model","version":version});
			agent.config["projection_version"] = json!("ordered");
		}
		let mut bindings = vec![];
		for (enabled, id, kind, adapter) in [
			(memory, "context-memory", "memory", "semantic_memory"),
			(
				workspace_retrieval,
				"context-workspace",
				"source",
				"workspace_retrieval",
			),
		] {
			if !enabled {
				continue;
			}
			let mut source = json!({"adapter":adapter});
			if degrade && adapter == "workspace_retrieval" {
				source["on_unavailable"] = json!("degrade");
			}
			let entry = json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Explicit native context"},"config":{"schema_version":1,"source":source}});
			f.registry
				.register(&acme, serde_json::from_value(entry).unwrap())
				.await
				.unwrap();
			assert_eq!(
				request(
					&app,
					&token,
					"POST",
					"/api/authorization/acme/catalog",
					json!({"entry":{"id":id,"version":"1.0.0"},"expected_revision":0,"enabled":true})
				)
				.await
				.0,
				200
			);
			bindings.push(json!({"kind":kind,"target":{"registry_node":f.config.deployment,"id":id,"version":"1.0.0"},"narrow":{}}));
		}
		agent.config["bindings"] = json!(bindings);
		f.registry.register(&acme, agent).await.unwrap();
		let owner = qualified_agent(f.config.deployment.as_str(), "research", "1.0.1");
		policy["subjects"][&owner] = json!({"kind":"agent"});
		assert_eq!(
			request(
				&app,
				&f.config.api_token,
				"POST",
				"/api/authorization/acme",
				json!({"expected_revision":1,"bundle":policy})
			)
			.await
			.0,
			200
		);
		assert_eq!(
			request(
				&app,
				&token,
				"POST",
				"/api/authorization/acme/catalog",
				json!({"entry":{"id":"research","version":"1.0.1"},"expected_revision":0,"enabled":true})
			)
			.await
			.0,
			200
		);
		let workspace = f
			.store
			.task(&TEST_FIXTURE, task)
			.await
			.unwrap()
			.workspace_id;
		let embedding_endpoint = embeddings.url.clone();
		// The embedding fixture is a Keyless endpoint on loopback (ADR 0036).
		f.store
			.keyless
			.permit_origin_of(&embedding_endpoint)
			.unwrap();
		let embedding_server = embeddings;
		configure(
			&app,
			&f.config.api_token,
			workspace,
			&spec(&embedding_endpoint),
			0,
		)
		.await;
		assert_eq!(
			request(
				&app,
				&token,
				"POST",
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
				workspace,
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
			let (status, entry) = request(
				&app,
				&token,
				"POST",
				&format!("/api/workspaces/{workspace}/semantic/entries"),
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
	let semantic = &contexts[0]["current"]["semantic_memory"]["workspace"];
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

/// Issue #233: a local Run in a Workspace whose index has `auto_context`
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
	assert_eq!(
		serde_json::to_value(run.recovery.semantic_reason).unwrap(),
		"unavailable",
		"{unavailable:?}"
	);
	assert_eq!(
		run.error.as_deref(),
		Some("The semantic backend is temporarily unavailable"),
		"{unavailable:?}"
	);
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

/// Issue #233: after the bounded retry a local Run under an embedding outage
/// pauses with the typed `retries_exhausted`, keeps its Task, and continues to
/// inference once the provider recovers and the Run is manually resumed.
#[rstest::rstest]
#[tokio::test]
async fn local_semantic_outage_pauses_after_bounded_retry_and_resumes_when_the_provider_recovers(
	#[future(awt)]
	#[with(false, true)]
	controlled_context: ControlledContext,
) {
	use aidash_server::domain::{RunControlAction, TaskStatus};
	let fixture = controlled_context;
	fixture.fault.set(Fault::Unavailable);
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
			assert_eq!(run.control.as_str(), "PAUSED", "{run:?}");
			assert_eq!(reason, "retries_exhausted", "{run:?}");
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

	fixture.fault.clear();
	let resumed = fixture
		.f
		.store
		.control(&TEST_FIXTURE, run_id.unwrap(), RunControlAction::Resume)
		.await
		.unwrap();
	assert_eq!(resumed.control.as_str(), "ACTIVE");
	assert!(fixture.worker.worker_once().await.unwrap());
	let contexts = fixture.captured.lock().unwrap().clone();
	assert_eq!(contexts.len(), 1, "the resumed step reaches inference");
	assert!(
		contexts[0]["current"]["semantic_memory"]["workspace"]["matches"]
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

/// Make a scheduled retry due now, leaving its counter and reason as the
/// production path persisted them.
async fn advance_retry(fixture: &ControlledContext, run: Uuid) {
	sqlx::query(
		&reinhardt::query::Query::update()
			.table(reinhardt::query::Alias::new("runs"))
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

/// The `semantic_memory` an Ordered model request carried in its volatile part.
fn ordered_semantic_memory(fixture: &ControlledContext, request: usize) -> Value {
	let contexts = fixture.captured.lock().unwrap();
	let parts = contexts[request - 1]["ordered"]
		.as_array()
		.expect("an Ordered request");
	parts[1]["semantic_memory"].clone()
}

/// Issue #236: a local Ordered Run reuses its semantic retrieval while its
/// Retrieval Key holds (ADR 0020), so an embedding outage after the first
/// retrieval does not touch the steps that reuse it. A Run message sent during
/// the outage changes the key's query inputs; only the step that then needs a
/// new retrieval follows the outage policy (#233). Once the provider recovers,
/// the retry retrieves under the new key and the next step reuses it.
#[rstest::rstest]
#[tokio::test]
async fn local_ordered_run_keeps_reusing_its_retrieval_through_an_embedding_outage(
	#[future(awt)]
	#[with(false, true, true)]
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
	let retrieved = ordered_semantic_memory(&fixture, 1);
	assert!(
		retrieved["workspace"]["matches"]
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
			ordered_semantic_memory(&fixture, step),
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

	// Act: a Run message changes the query inputs of the Retrieval Key.
	let (status, body) = request(
		&fixture._fixture.application,
		&fixture.token,
		"POST",
		&format!("/api/runs/{}/message", run().await.id),
		json!({"content":"Also cover vehicle maintenance."}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
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
	assert_eq!(fixture.fault.requests() - outage, 1, "one query attempt");
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
	let refreshed = ordered_semantic_memory(&fixture, 4);
	run_to_inference(&fixture, 5).await;
	assert_eq!(
		fixture.fault.requests() - recovered,
		1,
		"step 5 reuses step 4"
	);
	assert_eq!(ordered_semantic_memory(&fixture, 5), refreshed);

	fixture.worker.federation.store.pool.close().await;
	fixture.worker.federation.store.control_pool.close().await;
	drop(fixture.model_server);
	drop(fixture.embedding_server);
	dispose(fixture.f, &fixture.url, &fixture.schema).await;
}

/// Issue #234 (ADR 0059): an opted-in workspace source degrades after the
/// bounded retry. The step reaches inference with the typed marker, records the
/// degraded step and emits `run.semantic_degraded`. Missing vector points need
/// repair, so they still pause even with the opt-in.
#[rstest::rstest]
#[case::embedding_outage(true)]
#[case::missing_points(false)]
#[tokio::test]
async fn an_opted_in_local_workspace_outage_degrades_after_the_bounded_retry(
	#[case] embedding_outage: bool,
	#[future(awt)]
	#[with(false, true, false, true)]
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
		let semantic = &contexts[0]["current"]["semantic_memory"];
		assert_eq!(
			semantic.get("workspace").unwrap_or(semantic),
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
			.events(&TEST_FIXTURE, 0, Some(run.workspace_id), 500)
			.await
			.unwrap();
		let degraded = events
			.iter()
			.find(|event| event.kind == "run.semantic_degraded")
			.expect("run.semantic_degraded");
		assert_eq!(degraded.data["cause"], "embedding_provider");
		assert_eq!(degraded.data["run_id"], json!(run.id));
	}

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
	}
}
async fn configure(
	app: &common::TestApplication,
	deployment_administrator: &str,
	workspace: Uuid,
	spec: &IndexSpec,
	revision: i64,
) -> Value {
	let (status, index) = request(
		app,
		deployment_administrator,
		"POST",
		&format!("/api/workspaces/{workspace}/semantic/index"),
		serde_json::to_value(ConfigureIndex {
			expected_revision: revision,
			spec: spec.clone(),
		})
		.unwrap(),
	)
	.await;
	assert_eq!(status, 200, "{index}");
	index
}
async fn put(
	app: &common::TestApplication,
	token: &str,
	workspace: Uuid,
	key: &str,
	text: &str,
	revision: i64,
) -> Value {
	let (status,entry)=request(app,token,"POST",&format!("/api/workspaces/{workspace}/semantic/entries"),json!({"key":key,"source":{"kind":"memory","text":text},"expected_revision":revision,"metadata":{"topic":"public"}})).await;
	assert_eq!(status, 200, "{entry}");
	entry
}
async fn search(app: &common::TestApplication, token: &str, workspace: Uuid) -> (u16, Value) {
	request(
		app,
		token,
		"POST",
		&format!("/api/workspaces/{workspace}/semantic/search"),
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
	let workspace = f
		.store
		.task(&TEST_FIXTURE, task)
		.await
		.unwrap()
		.workspace_id;
	let mut config = spec(&endpoint);
	config.embedding.provider = provider.into();
	let first = configure(&app, &f.config.api_token, workspace, &config, 0).await;
	assert_eq!(
		configure(&app, &f.config.api_token, workspace, &config, 0).await["revision"],
		1
	);
	let cars = put(
		&app,
		&token,
		workspace,
		"cars",
		"A car carries passengers.",
		0,
	)
	.await;
	assert_eq!(
		put(
			&app,
			&token,
			workspace,
			"cars",
			"A car carries passengers.",
			0
		)
		.await["id"],
		cars["id"]
	);
	put(
		&app,
		&token,
		workspace,
		"bread",
		"Bread is baked in an oven.",
		0,
	)
	.await;
	assert_eq!(
		search(&app, &token, workspace).await.0,
		409,
		"pending data is never a successful empty search"
	);
	semantic::worker::sweep(&f.store).await.unwrap();
	let (status, result) = search(&app, &token, workspace).await;
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
	let updated = put(
		&app,
		&token,
		workspace,
		"cars",
		"A car uses roads safely.",
		1,
	)
	.await;
	assert_eq!(updated["revision"], 2);
	assert_eq!(search(&app, &token, workspace).await.0, 409);
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(
		search(&app, &token, workspace).await.1["matches"][0]["revision"],
		2
	);
	config.embedding.model_version = "2".into();
	let second = configure(&app, &f.config.api_token, workspace, &config, 1).await;
	assert_ne!(first["collection"], second["collection"]);
	assert_eq!(search(&app, &token, workspace).await.0, 409);
	// Fresh pools reconstruct authority and jobs entirely from durable state.
	let restarted = f.for_workers().await.unwrap();
	semantic::worker::sweep(&restarted.store).await.unwrap();
	assert_eq!(
		search(&app, &token, workspace).await.1["model_version"],
		"2"
	);
	restarted.store.pool.close().await;
	restarted.store.control_pool.close().await;
	let current_points = request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{workspace}/semantic/entries"),
		json!(null),
	)
	.await
	.1;
	let current_point = current_points
		.as_array()
		.unwrap()
		.iter()
		.find(|e| e["id"] == cars["id"])
		.unwrap()["point_id"]
		.clone();
	let path = format!(
		"/api/workspaces/{workspace}/semantic/entries/{}",
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
	let after = search(&app, &token, workspace).await;
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
	let workspace = f
		.store
		.task(&TEST_FIXTURE, task)
		.await
		.unwrap()
		.workspace_id;
	configure(&app, &f.config.api_token, workspace, &spec(&endpoint), 0).await;
	let cars = put(
		&app,
		&token,
		workspace,
		"cars",
		"A car carries passengers.",
		0,
	)
	.await;
	semantic::worker::sweep(&f.store).await.unwrap();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"revoke-semantic","effect":"deny","subjects":{"ids":["alice"]},"actions":["semantic.read"],"resources":{"kinds":["semantic"]}}));
	let (status, response) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	let reindex_path = format!(
		"/api/workspaces/{workspace}/semantic/entries/{}/reindex",
		cars["id"].as_str().unwrap()
	);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&reindex_path,
			json!({"expected_revision":1})
		)
		.await
		.0,
		403,
		"write-only permission cannot disclose a stored source in mutation responses"
	);
	// No allowed candidates means no embedding request is necessary, even when
	// the embedding backend is down. The caller cannot infer denied sources.
	drop(server);
	let response = search(&app, &token, workspace).await;
	assert_eq!(response.0, 200, "{}", response.1);
	assert_eq!(response.1["matches"], json!([]));
	let (status, credentials) = request(
		&app,
		&f.config.api_token,
		"GET",
		"/api/authorization/acme/credentials",
		json!(null),
	)
	.await;
	assert_eq!(status, 200, "{credentials}");
	let credential = credentials
		.as_array()
		.unwrap()
		.iter()
		.find(|c| c["subject"] == "alice")
		.unwrap();
	let auth = aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	};
	auth.revoke_credential(
		"acme",
		Uuid::parse_str(credential["id"].as_str().unwrap()).unwrap(),
	)
	.await
	.unwrap();
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
	assert_eq!(search(&app, &token, workspace).await.0, 401);
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
	let workspace = f
		.store
		.task(&TEST_FIXTURE, task)
		.await
		.unwrap()
		.workspace_id;
	let mut config = spec(&endpoint);
	config.embedding.dimensions = 4; // Provider's actual width is three.
	configure(&app, &f.config.api_token, workspace, &config, 0).await;
	let cars = put(
		&app,
		&token,
		workspace,
		"cars",
		"A car carries passengers.",
		0,
	)
	.await;
	semantic::worker::sweep(&f.store).await.unwrap();
	let entries = request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{workspace}/semantic/entries"),
		json!(null),
	)
	.await;
	assert_eq!(entries.0, 200);
	assert_eq!(entries.1[0]["state"], "ERROR");
	assert_eq!(entries.1[0]["attempts"], 1);
	assert_eq!(search(&app, &token, workspace).await.0, 409);
	config.embedding.dimensions = 3;
	configure(&app, &f.config.api_token, workspace, &config, 1).await;
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(search(&app, &token, workspace).await.0, 200);
	let reindex_path = format!(
		"/api/workspaces/{workspace}/semantic/entries/{}/reindex",
		cars["id"].as_str().unwrap()
	);
	let first_reindex = request(
		&app,
		&token,
		"POST",
		&reindex_path,
		json!({"expected_revision":1}),
	)
	.await;
	let replay_reindex = request(
		&app,
		&token,
		"POST",
		&reindex_path,
		json!({"expected_revision":1}),
	)
	.await;
	assert_eq!(first_reindex.0, 200);
	assert_eq!(replay_reindex.0, 200);
	assert_eq!(first_reindex.1["point_id"], replay_reindex.1["point_id"]);
	semantic::worker::sweep(&f.store).await.unwrap();
	let (status, _) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/semantic/search"),
		json!({"query":"vehicle","limit":999,"max_tokens":2048}),
	)
	.await;
	assert_eq!(status, 400);
	let (status,_)=request(&app,&token,"POST",&format!("/api/workspaces/{workspace}/semantic/entries"),json!({"key":"cars","expected_revision":99,"source":{"kind":"memory","text":"changed"},"metadata":{}})).await;
	assert_eq!(status, 409);
	drop(server);
	let response = search(&app, &token, workspace).await;
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
	let workspace = f
		.store
		.task(&TEST_FIXTURE, task)
		.await
		.unwrap()
		.workspace_id;
	configure(
		&app,
		&f.config.api_token,
		workspace,
		&spec(&embedding_endpoint),
		0,
	)
	.await;
	let cars = put(
		&app,
		&token,
		workspace,
		"cars",
		"A car carries passengers.",
		0,
	)
	.await;
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
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
		context["current"]["semantic_memory"]["workspace"]["matches"][0]["entry_id"],
		cars["id"]
	);
	assert_eq!(
		context["current"]["semantic_memory"]["workspace"]["model_version"],
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
		request(
			&app,
			&f.config.api_token,
			"POST",
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
		&format!("/api/workspaces/{workspace}/semantic/entries"),
		json!(null),
	)
	.await
	.1;
	assert_eq!(
		visible.as_array().unwrap().len(),
		0,
		"inline source reads require the semantic entry permission"
	);
	policy["policies"].as_array_mut().unwrap().pop();
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let path = format!(
		"/api/workspaces/{workspace}/semantic/entries/{}",
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
			&format!("/api/runs/{}", run.id),
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
	let workspace = f
		.store
		.task(&TEST_FIXTURE, task)
		.await
		.unwrap()
		.workspace_id;
	configure(&app, &f.config.api_token, workspace, &spec(&endpoint), 0).await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
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
			&qualified_agent(f.config.deployment.as_str(), "research", "1.0.0"),
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
			workspace,
			"human",
			"Vehicle safety",
			Some("semantic-vehicle-safety"),
		)
		.await
		.unwrap();
	let message = f
		.store
		.snapshot(&TEST_FIXTURE, workspace)
		.await
		.unwrap()
		.messages[0]
		.id;
	let mut ids = vec![];
	for (kind, id, agent) in [
		("artifact", artifact.id, None),
		("message", message, Some("aidash://private/agent@1.0.0")),
	] {
		let (status,entry)=request(&app,&token,"POST",&format!("/api/workspaces/{workspace}/semantic/entries"),json!({"key":kind,"expected_revision":0,"source":{"kind":kind,"id":id},"agent":agent,"metadata":{"topic":"transport"}})).await;
		assert_eq!(status, 200, "{entry}");
		ids.push(entry);
	}
	semantic::worker::sweep(&f.store).await.unwrap();
	let found = search(&app, &token, workspace).await;
	assert_eq!(found.0, 200, "{}", found.1);
	assert_eq!(
		found.1["matches"][0]["entry_id"], ids[0]["id"],
		"agent-scoped data is not implicitly included"
	);
	let path = format!("/api/workspaces/{workspace}/semantic/search");
	let (_,found)=request(&app,&token,"POST",&path,json!({"query":"vehicle","agent":"aidash://private/agent@1.0.0","metadata":{"topic":"transport"},"limit":10,"max_tokens":4096})).await;
	assert_eq!(found["matches"].as_array().unwrap().len(), 2);
	let (_, filtered) = request(
		&app,
		&token,
		"POST",
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
	assert_eq!(search(&app, &token, workspace).await.0, 409);
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
		search(&app, &token, workspace).await.1["matches"][0]["text"]
			.as_str()
			.unwrap()
			.contains("brakes")
	);
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-original-artifact","effect":"deny","subjects":{"ids":["alice"]},"actions":["artifact.read"],"resources":{"kinds":["artifact"],"ids":[artifact.id.to_string()]}}));
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		search(&app, &token, workspace).await.1["matches"],
		json!([])
	);
	let (_, entries) = request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{workspace}/semantic/entries"),
		json!(null),
	)
	.await;
	assert!(!entries.to_string().contains(&artifact.id.to_string()));
	// Even a Deployment Administrator cannot attach a resource from a different workspace.
	let other_tenant = common::tenant_scope(&f.store, "other").await;
	let other = f
		.store
		.create_workspace(&other_tenant, "bob", "Other", "Isolation")
		.await
		.unwrap();
	configure(&app, &f.config.api_token, other.id, &spec(&endpoint), 0).await;
	assert_eq!(request(&app,&f.config.api_token,"POST",&format!("/api/workspaces/{}/semantic/entries",other.id),json!({"key":"forged","expected_revision":0,"source":{"kind":"artifact","id":artifact.id},"metadata":{}})).await.0,403);
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
	let workspace = f
		.store
		.task(&TEST_FIXTURE, task)
		.await
		.unwrap()
		.workspace_id;
	let config = spec(&embedding);
	let index = configure(&app, &f.config.api_token, workspace, &config, 0).await;
	let entry = put(
		&app,
		&token,
		workspace,
		"restart",
		"A car carries passengers.",
		0,
	)
	.await;
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(search(&app, &token, workspace).await.0, 200);
	// Act: restart the exact PostgreSQL instance owned by the application fixture.
	environment.await.restart_database().await;
	assert_eq!(
		search(&app, &token, workspace).await.1["matches"][0]["entry_id"],
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
		search(&app, &token, workspace).await.0,
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
	assert_eq!(search(&app, &token, workspace).await.0, 200);
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
	tool_rounds: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
#[fixture]
fn controlled_context_state() -> ControlledContextState {
	ControlledContextState {
		captured: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
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
	std::sync::Arc::new(reinhardt::test::stub::StubRouter::new()
.route("/v1/chat/completions", http::Method::POST, reply(move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();
			let received = received.clone();
			let tool_rounds = tool_rounds.clone();
			async move {
				// A Legacy context is one JSON text; an Ordered context sends its
				// stable and volatile parts as separate JSON text parts.
				let content = &body["messages"][1]["content"];
				let context: Value = match content.as_str() {
					Some(text) => serde_json::from_str(text).unwrap(),
					None => json!({"ordered":content.as_array().unwrap().iter().map(|part| serde_json::from_str::<Value>(part["text"].as_str().unwrap()).unwrap()).collect::<Vec<_>>()}),
				};
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
                let context:Value=serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
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
