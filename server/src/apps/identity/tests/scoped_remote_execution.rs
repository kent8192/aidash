use common::upstream_fixtures;
use futures_util::{FutureExt, future::BoxFuture};
use reinhardt::ServerRouter as Router;
use upstream_fixtures::reply;
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
#[path = "scoped_remote_execution/native_memory.rs"]
mod native_memory;
use aidash_server::{
	domain::{NewTask, qualified_agent},
	federation::{Federation, Home},
	harness::Harness,
	registry::EntityRef,
};
use common::{cleanup, request};
use futures_util::StreamExt;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Mutex;
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Clone)]
struct ModelScript {
	requests: Arc<Mutex<Vec<Value>>>,
	hold: Arc<AtomicBool>,
	entered: Arc<Notify>,
	release: Arc<Notify>,
	reservations: Arc<Mutex<Option<ReservationCheck>>>,
	compactions: Arc<Mutex<Vec<Value>>>,
	compaction_status: Arc<AtomicUsize>,
	compaction_reservations: Arc<Mutex<Option<ReservationCheck>>>,
	force_memory_mutate: Arc<AtomicBool>,
}

#[derive(Clone)]
struct ReservationCheck {
	pools: Vec<sqlx::PgPool>,
	dispatcher: sqlx::PgPool,
	grant: Uuid,
	admission: Uuid,
	purpose: &'static str,
}
impl ReservationCheck {
	async fn before_http(&self) {
		let purposes = if self.purpose == "retrieval" {
			vec!["embedding", "memory"]
		} else {
			vec![self.purpose]
		};
		for pool in &self.pools {
			let count: i64 = {
				let query_bind_1 = self.grant;
				let query_bind_2 = self.admission;
				sqlx::query_scalar(
					&Query::select()
						.expr(Expr::cust("COUNT(*)"))
						.from(Alias::new("generation_remote_usage"))
						.and_where(Expr::col("grant_id").eq(Expr::value(query_bind_1)))
						.and_where(Expr::col("admission_id").eq(Expr::value(query_bind_2)))
						.and_where(Expr::col("purpose").is_in(purposes.clone()))
						.and_where(Expr::col("state").eq("RESERVED"))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(pool)
				.await
			}
			.unwrap();
			assert_eq!(
				count, 1,
				"every allowance owner must have committed its reservation before provider HTTP"
			);
		}
		let count: i64 = {
			let query_bind_1 = self.grant.to_string();
			sqlx::query_scalar(
				&Query::select()
					.expr(Expr::cust("COUNT(*)"))
					.from(Alias::new("generation_remote_dispatches"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(usage->>'grant_id'=? AND state='DISPATCHED')".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.and_where(Expr::cust("usage->>'purpose'").is_in(purposes))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&self.dispatcher)
			.await
		}
		.unwrap();
		assert_eq!(
			count, 1,
			"dispatch admission must commit before provider HTTP"
		);
	}
}
struct Pair {
	// Rebuilt applications must not release both node environments.
	_owners: Vec<common::RuntimeFixture>,
	model: ModelScript,
	drop_reply: Arc<Mutex<Option<String>>>,
	a: Federation,
	b: Federation,
	aa: common::TestApplication,
	ba: common::TestApplication,
	source_policy: Value,
	receiver_policy: Value,
	token: String,
	receiver_token: String,
	task: Uuid,
	grant: Uuid,
	admission: Uuid,
	requests: Arc<Mutex<Vec<Value>>>,
	servers: Vec<upstream_fixtures::FixedServerGuard>,
	providers: Vec<Arc<reinhardt::test::fixtures::server::TestServerGuard>>,
	au: String,
	bu: String,
	aschema: String,
	bschema: String,
	semantic: Option<SemanticFixture>,
	native: Option<Value>,
	generation: Option<(Value, Value)>,
	_memory_recovery_directories: Vec<Arc<tempfile::TempDir>>,
}
struct SemanticFixture {
	requests: Arc<Mutex<Vec<Value>>>,
	response: Arc<Mutex<Option<Value>>>,
	entry: Uuid,
	failing: Arc<AtomicBool>,
	reservations: Arc<Mutex<Option<ReservationCheck>>>,
}
impl Pair {
	async fn close(mut self) {
		self.providers.clear();
		for server in self.servers.drain(..) {
			server.abort();
			let _ = server.stopped().await;
		}
		cleanup(self.a.clone(), &self.au, &self.aschema).await;
		cleanup(self.b.clone(), &self.bu, &self.bschema).await;
	}
	fn activation(&self) -> String {
		format!(
			"/api/tasks/{}/remote-grants/{}/activate",
			self.task, self.grant
		)
	}
	async fn run(&self) -> aidash_server::domain::Run {
		self.b.store.run(self.admission).await.unwrap()
	}
	async fn step(&self) {
		let worker = Harness {
			federation: self.b.clone(),
		};
		// One step can include several separately bounded peer/provider calls.
		// Allow their combined duration on loaded CI runners without changing
		// any production timeout, retry deadline or lease.
		let progress = tokio::time::timeout(std::time::Duration::from_secs(60), async {
			loop {
				if worker.worker_once().await.unwrap() || self.run().await.phase().is_terminal() {
					// Reconciliation can finish a generated run before another
					// lease is available. Callers still assert its exact final state.
					break;
				}
				tokio::time::sleep(std::time::Duration::from_millis(100)).await;
			}
		})
		.await;
		if progress.is_err() {
			let run = self.run().await;
			panic!(
				"scoped step must progress within its bounded retry delay: phase={}, control={}, state={:?}",
				run.phase().as_str(),
				run.control.as_str(),
				run.state,
			);
		}
	}
}

async fn reconnect(f: &mut Federation, notify: Arc<Notify>) {
	let pool = f
		.store
		.pool
		.options()
		.clone()
		.connect_with(f.store.pool.connect_options().as_ref().clone())
		.await
		.unwrap();
	let store = aidash_server::store::Store::from_pool(pool, f.config.node_id.clone())
		.await
		.unwrap();
	f.registry =
		aidash_server::registry::Registry::new(store.pool.clone(), &f.config.node_id).unwrap();
	f.store.pool.close().await;
	f.store.control_pool.close().await;
	f.store = store;
	// Act: replace notification state after reconnecting the durable runtime.
	f.notify = notify;
}

#[rstest::fixture]
async fn scoped_pair(
	#[default(false)] semantic: bool,
	#[default(false)] generated: bool,
	#[default(false)] approval: bool,
	#[default(false)] compactor: bool,
	#[default(false)] native: bool,
	#[default((false,false,32))] large_native: (bool, bool, usize),
	#[future(awt)]
	#[from(scoped_infrastructure)]
	#[with(native)]
	infrastructure: ScopedInfrastructure,
) -> Pair {
	let ScopedInfrastructure {
		model_state,
		model_server,
		drop_reply,
		source,
		receiver,
		embedding_provider,
	} = infrastructure;
	let source_directory = source.application.directory;
	let receiver_directory = receiver.application.directory;
	let _aa = source.application.application;
	let _ba = receiver.application.application;
	let _aserver = source.server;
	let _bserver = receiver.server;
	let (large_native_graph, large_native_journal, native_graph_visits) = large_native;
	let _ = tracing_subscriber::fmt()
		.with_env_filter("aidash=debug")
		.with_test_writer()
		.try_init();
	let source = _aa;
	let receiver = _ba;
	let (a, au, aschema) = source.runtime.parts();
	let (b, bu, bschema) = receiver.runtime.parts();
	let owners = vec![source.runtime, receiver.runtime];
	let aa = source.application;
	let ba = receiver.application;
	let memory_recovery_directories = if native {
		vec![source_directory, receiver_directory]
	} else {
		vec![]
	};
	let requests = model_state.requests.clone();

	let endpoint = model_server.url.clone();
	let (mut source_policy, token, mut task) =
		common::bootstrap_with_context(&a, &aa, &endpoint, semantic).await;
	let (receiver_policy, _, _) =
		common::bootstrap_with_context(&b, &ba, &endpoint, semantic).await;
	let compactor_definition = json!({"id":"remote-compactor","version":"1.0.0","kind":"compactor","name":{"en":"Approved remote compactor"},"description":{"en":"Local fixture"},"config":{"provider":"typesafe-system-one","endpoint":format!("{endpoint}/systemone"),"model":"fixture-jev","credential_env":"AIDASH_SECRET_TEST_PEER","max_request_bytes":400000,"max_questions":200,"max_response_bytes":16000}});
	let (status, body) = request(
		&ba,
		&b.config.api_token,
		"POST",
		"/api/registry",
		compactor_definition,
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(
		request(
			&ba,
			&b.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"remote-compactor","version":"1.0.0"},"expected_revision":0,"enabled":true})
		)
		.await
		.0,
		200
	);
	source_policy["subjects"][qualified_agent(&b.config.node_id, "research", "1.0.0")] =
		json!({"kind":"agent"});
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":source_policy})
		)
		.await
		.0,
		200
	);
	for (local, other) in [(&a, &b), (&b, &a)] {
		{
			let query_bind_1 = &other.config.node_id;
			let query_bind_2 = &other.config.endpoint;
			sqlx::query(
				&Query::insert()
					.into_table(Alias::new("peers"))
					.columns(
						[
							"node_id",
							"endpoint",
							"credential_env",
							"protocol_version",
							"enabled",
						]
						.map(Alias::new),
					)
					.from_subquery(
						Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(Expr::cust("'AIDASH_SECRET_TEST_PEER'"))
							.expr(Expr::cust("'0.2'"))
							.expr(Expr::cust("TRUE"))
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(local.store.pool.driver())
			.await
		}
		.unwrap();
	}
	let (_, credential) = request(
		&ba,
		&b.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(request(&ba,&b.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":a.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":credential["credential"]["id"],"enabled":true,"expected_revision":0})).await.0,200);
	if semantic {
		let (_, home_reader) = request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"alice"}),
		)
		.await;
		let (status,body) = request(&aa,&a.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":b.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":home_reader["credential"]["id"],"enabled":true,"expected_revision":0})).await;
		assert_eq!(status, 200, "{body}");
	}
	let servers = vec![_aserver, _bserver];
	let mut providers = vec![model_server];
	let semantic = if semantic {
		let workspace = a.store.task(task).await.unwrap().workspace_id;
		let (fixture, server) = configure_semantic(
			&a,
			&aa,
			&token,
			workspace,
			&b.config.node_id,
			(native, large_native_journal),
			embedding_provider.await,
		)
		.await;
		providers.push(server);
		Some(fixture)
	} else {
		None
	};
	let native = if native {
		assert!(semantic.is_some());
		let workspace = a.store.task(task).await.unwrap().workspace_id;
		let data = native_remote_fixture(
			&a,
			&b,
			&aa,
			&ba,
			&token,
			workspace,
			(
				large_native_graph,
				large_native_journal,
				native_graph_visits,
			),
		)
		.await;
		source_policy["subjects"][qualified_agent(&b.config.node_id, "research-native", "1.0.0")] =
			json!({"kind":"agent"});
		let (status, body) = request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":source_policy}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		Some(data)
	} else {
		None
	};
	let generation = if generated {
		let (created, input, prepared) = prepare_generated_pair(
			&a,
			&b,
			&aa,
			&ba,
			&token,
			task,
			(approval, semantic.is_some(), native.is_some()),
		)
		.await;
		task = created;
		Some((input, prepared))
	} else {
		None
	};
	let agent = generation.as_ref().map_or_else(
		|| json!({"id":if native.is_some() { "research-native" } else { "research" },"version":"1.0.0"}),
		|(_, prepared)| prepared["agent"].clone(),
	);
	let grant = Uuid::new_v4();
	let mut mode = if semantic.is_some() {
		json!({"mode":"required_home","embedding":{"id":"home-embedding","version":"1.0.0"}})
	} else {
		json!({"mode":"disabled"})
	};
	if let Some(native) = &native {
		mode["native"] = native["selection"].clone();
	}
	if compactor {
		mode["compactor"] = json!({"id":"remote-compactor","version":"1.0.0"});
	}
	let admission = if approval {
		Uuid::nil()
	} else {
		let (status, prepared) = request(
			&aa,
			&token,
			"POST",
			&format!("/api/tasks/{task}/remote-grants"),
			json!({"id":grant,"node_id":b.config.node_id,"agent":agent,"ttl_seconds":if large_native_journal {3600} else {300},"semantic":mode}),
		)
		.await;
		assert_eq!(status, 200, "{prepared}");
		let (status, activated) = request(
			&aa,
			&token,
			"POST",
			&format!("/api/tasks/{task}/remote-grants/{grant}/activate"),
			json!({}),
		)
		.await;
		assert_eq!(status, 200, "{activated}");
		serde_json::from_value(activated["admission_id"].clone()).unwrap()
	};
	Pair {
		_owners: owners,
		model: model_state,
		drop_reply,
		a,
		b,
		aa,
		ba,
		source_policy,
		receiver_policy,
		token,
		task,
		grant,
		admission,
		requests,
		receiver_token: credential["token"].as_str().unwrap().into(),
		servers,
		providers,
		au,
		bu,
		aschema,
		bschema,
		semantic,
		native,
		generation,
		_memory_recovery_directories: memory_recovery_directories,
	}
}

async fn native_remote_fixture(
	a: &Federation,
	b: &Federation,
	aa: &common::TestApplication,
	ba: &common::TestApplication,
	token: &str,
	workspace: Uuid,
	(large_graph, large_journal, graph_visits): (bool, bool, usize),
) -> Value {
	let (_, home_agent) = request(
		aa,
		&a.config.api_token,
		"GET",
		"/api/registry/research/1.0.0",
		Value::Null,
	)
	.await;
	let (_, embedding) = request(
		aa,
		&a.config.api_token,
		"GET",
		"/api/registry/home-embedding/1.0.0",
		Value::Null,
	)
	.await;
	let reference = |id: &str| json!({"id":id,"version":"1.0.0"});
	let zero = json!({"input_per_million":0,"output_per_million":0});
	let provider = reference("native-memory");
	let mut policy = json!({"engine":"hindsight_rust","policy":{
        "extraction":home_agent["config"]["model"],"derivation":home_agent["config"]["model"],"reflection":home_agent["config"]["model"],
        "embedding":reference("home-embedding"),"reranker":reference("native-reranker"),"tokenizer":reference("native-tokenizer"),
        "prices":{"extraction":zero,"derivation":zero,"reflection":zero,"embedding":zero,"reranker":zero},
        "retention":{"unit_max_age_days":null,"candidate_days":7,"history_days":30,"history_versions":16,"model_result_days":7,"backup_days":7,"purge_after_seconds":60,"purge_batch":32,"max_unit_records":128,"max_model_operations":1024},
        "bounds":{"max_unit_bytes":8192,"max_input_bytes":8192,"max_units":16,"max_candidates":8,"max_entities":8,"max_evidence":8,"max_links":8,"max_graph_hops":3,"max_graph_visits":32,"max_results":4,"max_context_tokens":8192,"max_model_calls":4,"max_model_tokens":8192,"max_cost_micros":10000,"max_retries":2,"max_call_seconds":30},
        "semantic_link_min_similarity_millionths":700000,"learn_from_runs":false,"maintain_observations":false,"refresh_mental_models":false}});
	policy["policy"]["bounds"]["max_graph_visits"] = json!(graph_visits);
	if large_graph {
		policy["policy"]["bounds"]["max_graph_visits"] = json!(4096);
	}
	if large_journal {
		policy["policy"]["bounds"]["max_units"] = json!(2048);
		policy["policy"]["bounds"]["max_candidates"] = json!(256);
		policy["policy"]["retention"]["max_unit_records"] = json!(2048);
	}
	for (runtime, app) in [(a, aa), (b, ba)] {
		let mut roles = vec![
			("reranker", "native-reranker", json!({"provider":"rrf"})),
			(
				"tokenizer",
				"native-tokenizer",
				json!({"provider":"utf8_upper_bound"}),
			),
			("memory", "native-memory", policy.clone()),
			(
				"source",
				"native-shared",
				json!({"memory":provider,"scope":"workspace","max_tokens":8192}),
			),
		];
		if runtime.config.node_id == b.config.node_id {
			roles.insert(
				0,
				("embedding", "home-embedding", embedding["config"].clone()),
			);
		}
		for (kind, id, config) in roles {
			let (status,body)=request(app,&runtime.config.api_token,"POST","/api/registry",json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id,"ja":id},"description":{"en":"Native remote fixture"},"config":config})).await;
			assert_eq!(status, 200, "{body}");
			let (status, body) = request(
				app,
				&runtime.config.api_token,
				"POST",
				"/api/authorization/acme/catalog",
				json!({"entry":reference(id),"expected_revision":0,"enabled":true}),
			)
			.await;
			assert_eq!(status, 200, "{body}");
		}
		let (_, mut agent) = request(
			app,
			&runtime.config.api_token,
			"GET",
			"/api/registry/research/1.0.0",
			Value::Null,
		)
		.await;
		// This is a new authored version, not a copy of server-derived normalization.
		agent
			.as_object_mut()
			.unwrap()
			.remove("binding_normalization");
		agent["id"] = json!(if runtime.config.node_id == a.config.node_id {
			"home-native"
		} else {
			"research-native"
		});
		let bindings = agent["config"]["bindings"].as_array_mut().unwrap();
		bindings.retain(|binding| binding["kind"] != "memory");
		for (kind, id) in [("memory", "native-memory"), ("source", "native-shared")] {
			bindings.push(json!({"kind":kind,"target":{"registry_node":runtime.config.node_id,"id":id,"version":"1.0.0"},"narrow":{}}));
		}
		let id = agent["id"].as_str().unwrap().to_owned();
		let (status, body) = request(
			app,
			&runtime.config.api_token,
			"POST",
			"/api/registry",
			agent,
		)
		.await;
		assert_eq!(status, 200, "{body}");
		let (status, body) = request(
			app,
			&runtime.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":reference(&id),"expected_revision":0,"enabled":true}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		if runtime.config.node_id == b.config.node_id {
			let (_, mut policy) = request(
				app,
				&runtime.config.api_token,
				"GET",
				"/api/authorization/acme",
				Value::Null,
			)
			.await;
			let revision = policy["revision"].clone();
			policy["bundle"]["subjects"]
				[qualified_agent(&b.config.node_id, "research-native", "1.0.0")] = json!({"kind":"agent"});
			let (status, body) = request(
				app,
				&runtime.config.api_token,
				"POST",
				"/api/authorization/acme",
				json!({"expected_revision":revision,"bundle":policy["bundle"]}),
			)
			.await;
			assert_eq!(status, 200, "{body}");
		}
	}
	let (status, participant) = request(
		aa,
		token,
		"POST",
		&format!("/api/workspaces/{workspace}/memory/participants"),
		json!({"agent":reference("home-native")}),
	)
	.await;
	assert_eq!(status, 200, "{participant}");
	let private = Uuid::new_v4();
	let shared = Uuid::new_v4();
	for (id, bank, text) in [
		(
			private,
			participant["bank"].clone(),
			"Native private claim: 日本の研究手順 is durably attributed.",
		),
		(
			shared,
			json!({"home":a.config.node_id,"tenant":"acme","workspace":workspace,"participant":null}),
			"Native shared claim: 自転車の設計 uses reciprocal rank fusion.",
		),
	] {
		if id == shared {
			let (status,body)=request(aa,token,"POST",&format!("/api/workspaces/{workspace}/memory/operate"),json!({"operation_id":Uuid::new_v4(),"provider":provider,"bank":bank,"action":{"action":"configure_bank","expected_revision":0}})).await;
			assert_eq!(status, 200, "{body}");
		}
		let (status,body)=request(aa,token,"POST",&format!("/api/workspaces/{workspace}/memory/units/mutate"),json!({"operation_id":Uuid::new_v4(),"provider":provider,"bank":bank,"changes":[{"operation":"add","id":id,"content":{"text":text,"kind":"world","learning":"fact","verification":"unverified","occurred":null,"entities":[],"evidence":[],"links":[]}}]})).await;
		assert_eq!(status, 200, "{body}");
	}
	if large_journal {
		use aidash_domain::memory::{
			Bank, Change, Content, Kind, Learning, Mutation, Verification,
		};
		use aidash_server::apps::knowledge::services::native_memory as memory;
		let actor = aidash_server::authorization::Authorization {
			pool: a.store.pool.clone(),
		}
		.authenticate(token)
		.await
		.unwrap();
		let bank: Bank = serde_json::from_value(participant["bank"].clone()).unwrap();
		let provider: aidash_domain::registry::EntityRef =
			serde_json::from_value(provider.clone()).unwrap();
		// Model loss of the disposable index after valid bank admission. Canonical
		// grant dependencies must remain readable across projection reconstruction,
		// independently of the HTTP index's maximum 1,024 projected sources.
		let mut tx = aidash_server::database::native::begin(&a.store.pool)
			.await
			.unwrap();
		aidash_server::database::native::query(
			&Query::delete()
				.from_table(Alias::new("semantic_points"))
				.and_where(
					Expr::col("entry_id").in_subquery(
						Query::select()
							.column(Alias::new("id"))
							.from(Alias::new("semantic_entries"))
							.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
							.to_owned(),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
		for table in [
			"semantic_entries",
			"semantic_collections",
			"semantic_indexes",
		] {
			aidash_server::database::native::query(
				&Query::delete()
					.from_table(Alias::new(table))
					.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await
			.unwrap();
		}
		tx.commit().await.unwrap();
		for batch in 0..5 {
			let changes = (0..if batch == 4 { 1 } else { 256 })
				.map(|offset| Change::Add {
					id: Uuid::from_u128(1000 + batch * 256 + offset),
					content: Content {
						text: "Admitted long-lived grant dependency".into(),
						kind: Kind::World,
						learning: Learning::Fact,
						verification: Verification::Unverified,
						occurred: None,
						mental_model: None,
						entities: vec![],
						evidence: vec![],
						links: vec![],
					},
				})
				.collect();
			memory::mutate(
				&a.store,
				&actor,
				Mutation {
					operation_id: Uuid::now_v7(),
					provider: provider.clone(),
					bank: bank.clone(),
					changes,
				},
			)
			.await
			.unwrap();
		}
		let (status, body) = request(aa, &a.config.api_token, "POST", &format!("/api/workspaces/{workspace}/semantic/index"), json!({"expected_revision":0,"spec":{
			"embedding":embedding["config"],"vector":{"provider":"postgres","endpoint":"local","credential_env":null},
			"enabled":true,"auto_context":false,"max_sources":100,"max_results":10,"max_result_tokens":32768,"max_input_bytes":32768
		}})).await;
		assert_eq!(status, 200, "{body}");
	}
	if !large_journal {
		let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
		loop {
			aidash_server::semantic::worker::sweep(&a.store)
				.await
				.unwrap();
			let ready: Vec<String> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("state"))
					.from(Alias::new("semantic_entries"))
					.and_where(Expr::col("id").is_in([private, shared].map(Expr::value)))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(a.store.pool.driver())
			.await
			.unwrap();
			if ready == vec!["READY".to_string(); 2] {
				break;
			}
			assert!(tokio::time::Instant::now() < deadline, "{ready:?}");
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
	}

	json!({"selection":{"participant":participant["id"],"expected_revision":participant["revision"],"provider":provider},"bank":participant["bank"],"provider":provider,"private":private,"shared":shared})
}

async fn configure_semantic(
	a: &Federation,
	app: &common::TestApplication,
	token: &str,
	workspace: Uuid,
	executor: &str,
	memory_mode: (bool, bool),
	provider: EmbeddingProvider,
) -> (
	SemanticFixture,
	Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	let (native, large_journal) = memory_mode;
	let EmbeddingProvider {
		requests,
		failing,
		reservations,
		response,
		server,
	} = provider;
	let endpoint = format!("{}/v1", server.url);
	let embedding = json!({"provider":"openai","endpoint":endpoint,"credential_env":null,"model":"home-vector","model_version":"1","dimensions":3});
	let (status, body) = request(app, &a.config.api_token, "POST", "/api/registry", json!({"id":"home-embedding","version":"1.0.0","kind":"embedding","name":{"en":"Home embedding"},"description":{"en":"Deterministic semantic fixture"},"config":embedding})).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		app,
		&a.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"home-embedding","version":"1.0.0"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(app, &a.config.api_token, "POST", &format!("/api/workspaces/{workspace}/semantic/index"), json!({"expected_revision":0,"spec":{
		"embedding":embedding,"vector":{"provider":"postgres","endpoint":"local","credential_env":null},
		"enabled":true,"auto_context":!native,"max_sources":100,"max_results":10,"max_result_tokens":if native {32768} else {4096},"max_input_bytes":32768
	}})).await;
	assert_eq!(status, 200, "{body}");

	let mut first = Uuid::nil();
	// Every canonical bank requires an enabled index, including paged journals.
	if !large_journal {
		for (key, owner, text) in [
			("shared", None, "Iridium archive marker: ochre falcon."),
			(
				"exact",
				Some(qualified_agent(executor, "research", "1.0.0")),
				"Exact executor marker: silver fern.",
			),
			(
				"other-node",
				Some(qualified_agent(&a.config.node_id, "research", "1.0.0")),
				"DO NOT DISCLOSE: same name at Home.",
			),
			(
				"other-version",
				Some(qualified_agent(executor, "research", "1.0.1")),
				"DO NOT DISCLOSE: other definition version.",
			),
		] {
			let (status, body) = request(app, token, "POST", &format!("/api/workspaces/{workspace}/semantic/entries"), json!({"key":key,"expected_revision":0,"source":{"kind":"memory","text":text},"agent":owner,"metadata":{}})).await;
			assert_eq!(status, 200, "{body}");
			if key == "shared" {
				first = serde_json::from_value(body["id"].clone()).unwrap();
			}
		}
		aidash_server::semantic::worker::sweep(&a.store)
			.await
			.unwrap();
	}

	(
		SemanticFixture {
			response,
			reservations,
			requests,
			entry: first,
			failing,
		},
		server,
	)
}

#[rstest::rstest]
#[tokio::test]
async fn remote_semantic_context_reaches_actual_model_with_home_scope_and_receipts(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	p.step().await;
	let run = p.run().await;
	assert_eq!(run.control.as_str(), "ACTIVE", "{run:?}");
	let requests = p.requests.lock().await;
	assert_eq!(requests.len(), 1, "{run:?}");
	assert!(
		requests[0]["tools"]
			.as_array()
			.unwrap()
			.iter()
			.all(|tool| tool["function"]["name"] != "memory_mutate")
	);
	let context: Value =
		serde_json::from_str(requests[0]["messages"][1]["content"].as_str().unwrap()).unwrap();
	let semantic = &context["current"]["semantic_memory"];
	assert_eq!(semantic["home_node"], p.a.config.node_id, "{context}");
	assert_eq!(
		semantic["executor"],
		qualified_agent(&p.b.config.node_id, "research", "1.0.0")
	);
	assert_eq!(semantic["grant_id"], p.grant.to_string());
	let encoded = serde_json::to_string(semantic).unwrap();
	assert!(encoded.contains("ochre falcon"));
	assert!(encoded.contains("silver fern"));
	assert!(!encoded.contains("DO NOT DISCLOSE"));
	assert_eq!(semantic["result"]["matches"].as_array().unwrap().len(), 2);
	assert_eq!(semantic["sources"].as_array().unwrap().len(), 2);
	drop(requests);
	let calls = p.semantic.as_ref().unwrap().requests.lock().await;
	assert_eq!(calls.len(), 5, "four indexing calls and one search");
	assert!(
		!calls.last().unwrap()["input"]
			.as_str()
			.unwrap()
			.contains("ochre falcon"),
		"the query does not contain the retrieved text"
	);
	drop(calls);
	let saved: i64 = {
		let query_bind_1 = p.admission;
		sqlx::query_scalar(
			&Query::select()
				.expr(reinhardt::query::Func::count(
					Expr::col(Alias::new("operation_id")).into(),
				))
				.from(Alias::new("semantic_remote_receipts"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(run_id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(saved, 1);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_dependencies_hide_both_node_outputs_and_journals_after_source_change(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	for _ in 0..10 {
		if p.run().await.phase().as_str() == "COMPLETED" {
			break;
		}
		p.step().await;
		assert_eq!(
			p.run().await.control.as_str(),
			"ACTIVE",
			"{:?}",
			p.run().await
		);
	}
	assert_eq!(
		p.run().await.phase().as_str(),
		"COMPLETED",
		"{:?}",
		p.run().await
	);
	let workspace = p.a.store.task(p.task).await.unwrap().workspace_id;
	let path = format!("/api/runs/{}", p.admission);
	let (status, body) = request(&p.ba, &p.receiver_token, "GET", &path, json!({})).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["run"]["id"], p.admission.to_string());
	let home = format!("/api/workspaces/{workspace}");
	let (status, body) = request(&p.aa, &p.token, "GET", &home, json!({})).await;
	assert_eq!(status, 200, "{body}");
	assert!(body.to_string().contains("Scoped remote result"), "{body}");
	assert!(
		body.to_string().contains("Scoped remote progress"),
		"{body}"
	);
	let artifact = body["artifacts"][0]["id"].clone();
	let receipt_path = format!("/api/tasks/{}/remote-grants/{}/semantic", p.task, p.grant);
	let (status, receipt) = request(&p.aa, &p.token, "GET", &receipt_path, json!({})).await;
	assert_eq!(status, 200, "{receipt}");
	assert_eq!(receipt["sources"].as_array().unwrap().len(), 2);
	assert!(
		!receipt.to_string().contains("ochre falcon"),
		"provenance must not return source text"
	);
	// Unbounded SSE uses the fixture-owned streaming client without buffering the body.
	let response =
		p.aa.streaming_http
			.get(p.aa.url(format!("/api/events/stream?workspace_id={workspace}")))
			.bearer_auth(&p.token)
			.send()
			.await
			.unwrap();
	assert_eq!(response.status(), 200);
	let mut stream = response.bytes_stream();
	tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	let source = p.semantic.as_ref().unwrap().entry;
	let (status, body) = request(
		&p.aa,
		&p.token,
		"DELETE",
		&format!("/api/workspaces/{workspace}/semantic/entries/{source}"),
		json!({"expected_revision":1}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(&p.ba, &p.receiver_token, "GET", &path, json!({})).await;
	assert_eq!(status, 403, "{body}");
	assert!(!body.to_string().contains("ochre falcon"));
	let (status, body) = request(&p.aa, &p.token, "GET", &home, json!({})).await;
	assert_eq!(status, 200, "{body}");
	assert!(!body.to_string().contains("Scoped remote result"), "{body}");
	assert!(
		!body.to_string().contains("Scoped remote progress"),
		"{body}"
	);
	assert_eq!(
		request(&p.aa, &p.token, "GET", &receipt_path, json!({}))
			.await
			.0,
		403
	);
	let (status,body)=request(&p.aa,&p.token,"POST",&format!("/api/workspaces/{workspace}/semantic/entries"),json!({"key":"derived-reingestion","expected_revision":0,"source":{"kind":"artifact","id":artifact},"metadata":{}})).await;
	assert_eq!(status, 403, "{body}");
	let (status, body) = request(
		&p.aa,
		&p.token,
		"POST",
		&format!("/api/workspaces/{workspace}/messages"),
		json!({"content":"independent-stream-tail"}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	tokio::time::timeout(std::time::Duration::from_secs(20), async {
		loop {
			let chunk = stream.next().await.unwrap().unwrap();
			let frame = String::from_utf8_lossy(&chunk);
			assert!(
				!frame.contains("Scoped remote result")
					&& !frame.contains("Scoped remote progress"),
				"{frame}"
			);
			if frame.contains("independent-stream-tail") {
				break;
			}
		}
	})
	.await
	.unwrap();
	drop(stream);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_journal_reads_require_the_current_viewer_at_home(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	p.step().await;
	let mut source_policy = p.source_policy.clone();
	let mut receiver_policy = p.receiver_policy.clone();
	source_policy["subjects"]["bob"] = json!({"kind":"user"});
	receiver_policy["subjects"]["bob"] = json!({"kind":"user"});
	for (node, app, bundle, revision) in [
		(&p.a, &p.aa, &source_policy, 2),
		(&p.b, &p.ba, &receiver_policy, 1),
	] {
		let (status, body) = request(
			app,
			&node.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":revision,"bundle":bundle}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
	let (_, viewer) = request(
		&p.ba,
		&p.b.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	let token = viewer["token"].as_str().unwrap();
	let path = format!("/api/runs/{}", p.admission);
	assert_eq!(
		request(&p.ba, token, "GET", &path, json!({})).await.0,
		403,
		"producer authority must not stand in for an unmapped reader"
	);
	let (_, home_viewer) = request(
		&p.aa,
		&p.a.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	let (status,body)=request(&p.aa,&p.a.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":p.b.config.node_id,"source_tenant":"acme","source_subject":"bob","credential_id":home_viewer["credential"]["id"],"enabled":true,"expected_revision":0})).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(&p.ba, token, "GET", &path, json!({})).await;
	assert_eq!(status, 200, "{body}");
	source_policy["policies"].as_array_mut().unwrap().push(json!({"id":"reader-source-denial","effect":"deny","subjects":{"ids":["bob"]},"actions":["semantic.read"],"resources":{"kinds":["*"]}}));
	let (status, body) = request(
		&p.aa,
		&p.a.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":3,"bundle":source_policy}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(request(&p.ba, token, "GET", &path, json!({})).await.0, 403);
	assert_eq!(
		request(&p.ba, &p.receiver_token, "GET", &path, json!({}))
			.await
			.0,
		200,
		"the producer still has authority; only the current reader was denied"
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_empty_receipt_replay_rechecks_candidates_without_an_empty_embedding_charge(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	let workspace = p.a.store.task(p.task).await.unwrap().workspace_id;
	let ids: Vec<Uuid> = {
		let query_bind_1 = workspace;
		let query_bind_2 = qualified_agent(&p.b.config.node_id, "research", "1.0.0");
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("semantic_entries"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(workspace_id=? AND (agent IS NULL OR agent=?))".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(p.a.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(ids.len(), 2);
	for id in ids {
		let (status, body) = request(
			&p.aa,
			&p.token,
			"DELETE",
			&format!("/api/workspaces/{workspace}/semantic/entries/{id}"),
			json!({"expected_revision":1}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
	p.step().await;
	*p.drop_reply.lock().await = Some("semantic.query".into());
	p.step().await;
	assert!(p.requests.lock().await.is_empty());
	assert_eq!(
		p.semantic.as_ref().unwrap().requests.lock().await.len(),
		4,
		"empty candidate retrieval does not call embedding"
	);
	let receipt: Value = {
		let query_bind_1 = p.grant;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("receipt"))
				.from(Alias::new("semantic_remote_operations"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(grant_id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.a.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(receipt["sources"], json!([]));
	let (status,body)=request(&p.aa,&p.token,"POST",&format!("/api/workspaces/{workspace}/semantic/entries"),json!({"key":"new-current-source","expected_revision":0,"source":{"kind":"memory","text":"Newly admitted cobalt raven."},"metadata":{}})).await;
	assert_eq!(status, 200, "{body}");
	aidash_server::semantic::worker::sweep(&p.a.store)
		.await
		.unwrap();
	p.step().await;
	let requests = p.requests.lock().await;
	assert_eq!(requests.len(), 1, "{:?}", p.run().await);
	let body: Value =
		serde_json::from_str(requests[0]["messages"][1]["content"].as_str().unwrap()).unwrap();
	assert!(
		body["current"]["semantic_memory"]
			.to_string()
			.contains("cobalt raven")
	);
	assert_eq!(
		body["current"]["semantic_memory"]["sources"]
			.as_array()
			.unwrap()
			.len(),
		1
	);
	drop(requests);
	assert_eq!(
		p.semantic.as_ref().unwrap().requests.lock().await.len(),
		6,
		"one new index embedding and one fresh retrieval embedding"
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn consumed_home_semantic_revision_blocks_next_remote_inference(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	p.step().await;
	assert_eq!(p.requests.lock().await.len(), 1, "{:?}", p.run().await);
	let workspace = p.a.store.task(p.task).await.unwrap().workspace_id;
	let source = p.semantic.as_ref().unwrap().entry;
	let (status, body) = request(
		&p.aa,
		&p.token,
		"DELETE",
		&format!("/api/workspaces/{workspace}/semantic/entries/{source}"),
		json!({"expected_revision":1}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	p.step().await;
	assert_eq!(p.run().await.control.as_str(), "PAUSED");
	assert_eq!(p.requests.lock().await.len(), 1);
	let control = format!("/api/tasks/{}/remote-grants/{}/control", p.task, p.grant);
	let management = format!("/api/runs/{}/management", p.admission);
	let (status, body) = request(&p.ba, &p.receiver_token, "GET", &management, json!({})).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["semantic_reason"], "invalidated");
	assert!(body.get("context").is_none() && body.get("pending").is_none());
	assert!(!body.to_string().contains("ochre falcon"));
	let (status, body) = request(
		&p.aa,
		&p.token,
		"POST",
		&control,
		json!({"action":"resume"}),
	)
	.await;
	assert_eq!(status, 409, "{body}");
	let (status, body) = request(
		&p.ba,
		&p.receiver_token,
		"POST",
		&management,
		json!({"action":"cancel"}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["control"], "CANCELLED");
	let (status, body) = request(
		&p.aa,
		&p.token,
		"POST",
		&control,
		json!({"action":"cancel"}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert!(!body.to_string().contains("ochre falcon"));
	let followup = format!("/api/tasks/{}/remote-grants/{}/follow-up", p.task, p.grant);
	let input = json!({"id":Uuid::new_v4(),"title":"New independent intent","description":"Use only current authorized material.","requirements":{}});
	let (status, body) = request(&p.aa, &p.token, "POST", &followup, input.clone()).await;
	assert_eq!(status, 200, "{body}");
	assert_ne!(body["id"], p.task.to_string());
	assert!(body["parent_id"].is_null());
	assert_eq!(body["description"], input["description"]);
	assert_eq!(
		body,
		request(&p.aa, &p.token, "POST", &followup, input).await.1
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn lost_semantic_reply_replays_receipt_without_a_second_embedding(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	*p.drop_reply.lock().await = Some("semantic.query".into());
	p.step().await;
	assert!(p.requests.lock().await.is_empty());
	let retry = p.run().await;
	assert_eq!(retry.recovery.retry.as_ref().unwrap().count, 1);
	let due: chrono::DateTime<chrono::Utc> = retry.recovery.retry.as_ref().unwrap().at;
	assert!(due > chrono::Utc::now());
	assert_eq!(
		p.run().await.control.as_str(),
		"ACTIVE",
		"{:?}",
		p.run().await
	);
	p.step().await;
	assert_eq!(p.requests.lock().await.len(), 1, "{:?}", p.run().await);
	assert_eq!(p.semantic.as_ref().unwrap().requests.lock().await.len(), 5);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn source_deleted_during_remote_model_request_rejects_the_response(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	p.model.hold.store(true, Ordering::Release);
	let worker = Harness {
		federation: p.b.clone(),
	};
	let running = tokio::spawn(async move { worker.worker_once().await });
	tokio::time::timeout(
		std::time::Duration::from_secs(20),
		p.model.entered.notified(),
	)
	.await
	.unwrap();
	let workspace = p.a.store.task(p.task).await.unwrap().workspace_id;
	let source = p.semantic.as_ref().unwrap().entry;
	let (status, body) = request(
		&p.aa,
		&p.token,
		"DELETE",
		&format!("/api/workspaces/{workspace}/semantic/entries/{source}"),
		json!({"expected_revision":1}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	p.model.release.notify_waiters();
	running.await.unwrap().unwrap();
	assert_eq!(
		p.run().await.control.as_str(),
		"PAUSED",
		"{:?}",
		p.run().await
	);
	assert!(
		!p.run()
			.await
			.context
			.to_string()
			.contains("Scoped remote progress")
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_retry_exhaustion_pauses_and_manual_resume_keeps_attempt_history(
	#[with(true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	p.semantic
		.as_ref()
		.unwrap()
		.failing
		.store(true, Ordering::Release);
	for failure in 1..=6 {
		p.step().await;
		let run = p.run().await;
		assert!(p.requests.lock().await.is_empty());
		assert_eq!(
			run.control.as_str(),
			if failure == 6 { "PAUSED" } else { "ACTIVE" },
			"{run:?}"
		);
		if failure < 6 {
			assert_eq!(run.recovery.retry.as_ref().unwrap().count, failure);
			let due: chrono::DateTime<chrono::Utc> = run.recovery.retry.as_ref().unwrap().at;
			assert!(due > chrono::Utc::now());
			// Advance only persisted fixture deadlines, after checking that the
			// production path saved a delay and retained its failure counter.
			{ let query_bind_1 = p.admission; sqlx::query(&Query::update()
					.table(Alias::new("runs")).value_expr(Alias::new("pending"), Expr::cust(
							"JSONB_SET(pending, '{recovery,retry,at}', TO_JSONB(CLOCK_TIMESTAMP()))",
						))
					.and_where(SimpleExpr::CustomWithExpr("(id=? AND SET_CONFIG('aidash.input_ledger_worker','true',true)='true')".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()]))
					.to_string(PostgresQueryBuilder))
			.execute(p.b.store.pool.driver())
			.await }
			.unwrap();
			{
				let query_bind_1 = p.grant;
				sqlx::query(
					&Query::update()
						.table(Alias::new("semantic_remote_operations"))
						.value_expr(Alias::new("next_attempt"), Expr::cust("CLOCK_TIMESTAMP()"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(grant_id=? AND state='WAITING')".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(p.a.store.pool.driver())
				.await
			}
			.unwrap();
		}
	}
	let count = || {
		Query::select()
			.expr(reinhardt::query::Func::count(
				Expr::col(Alias::new("id")).into(),
			))
			.from(Alias::new("semantic_remote_attempts"))
			.to_string(PostgresQueryBuilder)
	};
	let before: i64 = sqlx::query_scalar(&count())
		.fetch_one(p.a.store.pool.driver())
		.await
		.unwrap();
	assert_eq!(before, 6);
	p.semantic
		.as_ref()
		.unwrap()
		.failing
		.store(false, Ordering::Release);
	let (status, body) = request(
		&p.aa,
		&p.token,
		"POST",
		&format!("/api/tasks/{}/remote-grants/{}/control", p.task, p.grant),
		json!({"action":"resume"}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert!(p.run().await.recovery.retry.is_none());
	p.step().await;
	assert_eq!(p.requests.lock().await.len(), 1, "{:?}", p.run().await);
	let after: i64 = sqlx::query_scalar(&count())
		.fetch_one(p.a.store.pool.driver())
		.await
		.unwrap();
	assert_eq!(after, 7);
	let cycles: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::sum(
				Expr::col(Alias::new("cycle")).into(),
			))
			.from(Alias::new("semantic_remote_operations"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.a.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(cycles, 1);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn scoped_remote_worker_finishes_at_home_and_retries_keep_one_execution(
	#[future(awt)] scoped_pair: Pair,
) {
	let p = scoped_pair;
	let (status, replay) = request(&p.aa, &p.token, "POST", &p.activation(), json!({})).await;
	assert_eq!(status, 200, "{replay}");
	assert_eq!(replay["run_id"], p.admission.to_string());
	for _ in 0..10 {
		if p.run().await.phase().as_str() == "COMPLETED" {
			break;
		}
		p.step().await;
		let run = p.run().await;
		assert_eq!(
			run.control.as_str(),
			"ACTIVE",
			"{} {:?}",
			run.phase().as_str(),
			run.error
		);
		assert!(
			run.error.is_none(),
			"{} {:?}",
			run.phase().as_str(),
			run.error
		);
	}
	let run = p.run().await;
	assert_eq!(run.phase().as_str(), "COMPLETED", "{:?}", run.error);
	assert_eq!(run.agent_version, "1.0.0");
	let task = p.a.store.task(p.task).await.unwrap();
	assert_eq!(task.status.as_str(), "COMPLETED");
	let snapshot = p.a.store.snapshot(task.workspace_id).await.unwrap();
	assert_eq!(snapshot.artifacts.len(), 1);
	assert_eq!(snapshot.artifacts[0].content, "Scoped remote result");
	assert_eq!(
		snapshot
			.messages
			.iter()
			.filter(|m| m.content == "Scoped remote progress")
			.count(),
		1
	);
	let events =
		p.a.store
			.events(0, Some(task.workspace_id), 1000)
			.await
			.unwrap();
	let completed = events
		.iter()
		.filter(|event| event.kind == "task.remote_tool_completed")
		.collect::<Vec<_>>();
	assert!(!completed.is_empty());
	assert!(
		completed
			.iter()
			.all(|event| event.data["remote_run_id"] == json!(p.admission)
				&& event.data["detail"]["call"]["name"].is_string())
	);
	assert!(
		completed
			.iter()
			.all(|event| event.data["detail"]["call"]["arguments"].is_null())
	);
	assert!(
		!events
			.iter()
			.any(|event| event.kind == "task.remote_run_recovered")
	);
	assert_eq!(p.requests.lock().await.len(), 2);
	let input = p
		.requests
		.lock()
		.await
		.iter()
		.map(ToString::to_string)
		.collect::<String>();
	assert!(!input.contains(&p.token));
	assert!(!input.contains(&std::env::var("AIDASH_SECRET_TEST_PEER").unwrap()));
	let (status, replay) = request(&p.aa, &p.token, "POST", &p.activation(), json!({})).await;
	assert_eq!(status, 200, "{replay}");
	assert_eq!(replay["phase"], "COMPLETED");
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn transaction_finalization_uses_the_actual_home_and_executor_admission(
	#[future(awt)] scoped_pair: Pair,
) {
	use aidash_server::transactions::{Manifest, coordinator, participant};
	let p = scoped_pair;
	for (local, app, remote) in [(&p.a, &p.aa, &p.b), (&p.b, &p.ba, &p.a)] {
		assert_eq!(
			request(
				app,
				&local.config.api_token,
				"POST",
				"/api/transactions/trust",
				json!({"node_id":remote.config.node_id,"enabled":true})
			)
			.await
			.0,
			200
		);
	}
	for _ in 0..4 {
		if p.a.store.task(p.task).await.unwrap().status
			== aidash_server::domain::TaskStatus::Running
		{
			break;
		}
		p.step().await;
	}
	let task = p.a.store.task(p.task).await.unwrap();
	assert_eq!(task.status.as_str(), "RUNNING");
	let run = p.run().await;
	{ let query_bind_1 = run.id; let query_bind_2 = common::tool_pending(json!({"response":{"text":"Atomic remote answer","tool_calls":[],"input_tokens":0,"output_tokens":0},"cursor":0})); sqlx::query(&Query::update().table(Alias::new("runs"))
		.value(Alias::new("phase"),"TOOL_CALL").value_expr(Alias::new("pending"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_2.to_owned()).into()]))
		.and_where(SimpleExpr::CustomWithExpr("(id=?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).to_string(PostgresQueryBuilder))
		.execute(p.b.store.pool.driver()).await }.unwrap();
	let mut manifest:Manifest=serde_json::from_value(json!({"id":Uuid::new_v4(),"coordinator":p.a.config.node_id,"isolation":"serializable","deadline":chrono::Utc::now()+chrono::Duration::minutes(5),
		"participants":[{"node_id":p.a.config.node_id,"mutations":[{"kind":"complete_task","task_id":task.id,"expected_revision":task.revision,"artifact":{"kind":"text","name":"Answer","content":"Atomic remote answer"}}]},
		{"node_id":p.b.config.node_id,"mutations":[{"kind":"finish_run","run_id":run.id,"task_id":task.id,"expected_revision":run.revision}]}]})).unwrap();
	manifest
		.participants
		.sort_by(|a, b| a.node_id.cmp(&b.node_id));
	let (status, body) = request(
		&p.aa,
		&p.token,
		"POST",
		"/api/transactions",
		json!(manifest),
	)
	.await;
	assert_eq!(status, 202, "{body}");
	for _ in 0..2 {
		coordinator::advance(&p.a, manifest.id).await.unwrap();
	}
	let (status, body) = request(
		&p.aa,
		&p.token,
		"GET",
		&format!("/api/transactions/{}", manifest.id),
		Value::Null,
	)
	.await;
	assert_eq!(
		status, 200,
		"current authorized controls remain available during reservation: {body}"
	);
	for _ in 0..20 {
		if coordinator::advance(&p.a, manifest.id)
			.await
			.unwrap()
			.complete
		{
			break;
		}
	}
	let result = coordinator::status(&p.a, manifest.id).await.unwrap();
	assert!(result.complete, "{result:?}");
	assert_eq!(result.decision.as_deref(), Some("COMMIT"), "{result:?}");
	for local in [&p.a, &p.b] {
		participant::finish(local, &p.a.config.node_id, &manifest)
			.await
			.unwrap();
	}
	assert_eq!(
		p.a.store.task(task.id).await.unwrap().status.as_str(),
		"COMPLETED"
	);
	assert_eq!(p.run().await.phase().as_str(), "COMPLETED");
	let artifacts =
		p.a.store
			.snapshot(task.workspace_id)
			.await
			.unwrap()
			.artifacts;
	assert_eq!(artifacts.len(), 1);
	assert_eq!(artifacts[0].created_by, task.owner.unwrap());
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn scoped_remote_agent_cannot_delegate_through_an_excluded_default_operation(
	#[future(awt)] scoped_pair: Pair,
) {
	let p = scoped_pair;
	let run = p.run().await;
	let excluded = run
		.context
		.binding_snapshot
		.as_ref()
		.unwrap()
		.bindings
		.iter()
		.find(|binding| binding.definition.config["operation"] == "task_delegate")
		.unwrap();
	assert!(
		excluded.excluded_reason.is_some(),
		"the factual Provider contract is local-only"
	);
	let home = Home::new(p.b.clone(), run);
	let child = home
		.create_task(
			"scoped-child-create",
			&NewTask {
				title: "Scoped delegated child".into(),
				description: "Created under a remote run".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: Some(p.task),
			},
		)
		.await
		.unwrap();
	let agent = EntityRef {
		id: "research".into(),
		version: "1.0.0".into(),
	};
	// Calling Home directly cannot recover an operation deliberately excluded
	// under Q23 or turn its descriptor into a broader remote contract.
	for _ in 0..2 {
		assert!(
			home.delegate_with_key(
				"scoped-child-delegate",
				child.id,
				&p.a.config.node_id,
				&agent
			)
			.await
			.is_err()
		);
	}
	assert!(
		!p.a.store
			.runs()
			.await
			.unwrap()
			.iter()
			.any(|run| run.task_id == child.id)
	);
	assert!(p.a.store.task(child.id).await.unwrap().owner.is_none());
	p.close().await;
}

#[rstest::rstest]
#[case("source_policy")]
#[case("receiver_policy")]
#[case("expiry")]
#[case("grant_revocation")]
#[case("source_credential")]
#[case("receiver_mapping")]
#[case("task_revision")]
#[case("disabled_agent")]
#[case("definition_substitution")]
#[tokio::test]
async fn a_changed_execution_boundary_stops_before_inference(
	#[case] fault: &str,
	#[future(awt)] scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	assert_eq!(p.run().await.phase().as_str(), "THINKING");
	match fault {
		"source_policy" | "receiver_policy" => {
			let (f, app, mut bundle, revision) = if fault == "source_policy" {
				(&p.a, &p.aa, p.source_policy.clone(), 2)
			} else {
				(&p.b, &p.ba, p.receiver_policy.clone(), 1)
			};
			bundle["policies"].as_array_mut().unwrap().push(json!({"id":"deny-model","effect":"deny","subjects":{"any":true},"actions":["model.infer"],"resources":{"kinds":["model"]}}));
			assert_eq!(
				request(
					app,
					&f.config.api_token,
					"POST",
					"/api/authorization/acme",
					json!({"expected_revision":revision,"bundle":bundle})
				)
				.await
				.0,
				200
			);
		}
		"expiry" => {
			{
				let query_bind_1 = p.grant;
				sqlx::query(
					&Query::update()
						.table(Alias::new("authorization_remote_grants"))
						.value_expr(
							Alias::new("expires_at"),
							Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(p.a.store.pool.driver())
				.await
			}
			.unwrap();
		}
		"grant_revocation" => {
			assert_eq!(
				request(
					&p.aa,
					&p.token,
					"POST",
					&format!("/api/tasks/{}/remote-grants/{}/revoke", p.task, p.grant),
					json!({})
				)
				.await
				.0,
				200
			);
		}
		"source_credential" => {
			let id: Uuid = {
				let query_bind_1 = p.grant;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("credential_id"))
						.from(Alias::new("authorization_remote_grants"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(p.a.store.pool.driver())
				.await
			}
			.unwrap();
			assert_eq!(
				request(
					&p.aa,
					&p.a.config.api_token,
					"POST",
					&format!("/api/authorization/acme/credentials/{id}/revoke"),
					json!({})
				)
				.await
				.0,
				200
			);
		}
		"receiver_mapping" => {
			let id: Uuid = {
				let query_bind_1 = &p.a.config.node_id;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("credential_id"))
						.from(Alias::new("authorization_peer_mappings"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(source_node=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(p.b.store.pool.driver())
				.await
			}
			.unwrap();
			assert_eq!(request(&p.ba,&p.b.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":p.a.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":id,"enabled":false,"expected_revision":1})).await.0,200);
		}
		"task_revision" => {
			{
				let query_bind_1 = p.task;
				sqlx::query(
					&Query::update()
						.table(Alias::new("tasks"))
						.value_expr(Alias::new("revision"), Expr::cust("revision+1"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(p.a.store.pool.driver())
				.await
			}
			.unwrap();
		}
		"disabled_agent" => {
			assert_eq!(
				request(
					&p.ba,
					&p.b.config.api_token,
					"POST",
					"/api/authorization/acme/catalog",
					json!({"entry":{"id":"research","version":"1.0.0"},"expected_revision":1,"enabled":false})
				)
				.await
				.0,
				200
			);
		}
		"definition_substitution" => {
			{
				let query_bind_1 = p.admission;
				sqlx::query(
					&Query::update()
						.table(Alias::new("runs"))
						.value_expr(Alias::new("agent_version"), Expr::cust("'9.9.9'"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(p.b.store.pool.driver())
				.await
			}
			.unwrap();
		}
		_ => panic!("unknown fault"),
	}
	p.step().await;
	let run = p.run().await;
	assert_eq!(
		run.control.as_str(),
		"PAUSED",
		"{fault}: {} {:?}",
		run.phase().as_str(),
		run.error
	);
	assert!(p.requests.lock().await.is_empty());
	let task = p.a.store.task(p.task).await.unwrap();
	assert_eq!(task.status.as_str(), "RUNNING");
	assert!(
		p.a.store
			.snapshot(task.workspace_id)
			.await
			.unwrap()
			.artifacts
			.is_empty()
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn remote_inputs_are_durable_and_idempotent_and_controls_remain_scoped(
	#[future(awt)] scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	let path = format!("/api/tasks/{}/remote-grants/{}/messages", p.task, p.grant);
	let input =
		json!({"id":Uuid::new_v4(),"content":"Keep this accepted correction after reconnect."});
	let first = request(&p.aa, &p.token, "POST", &path, input.clone()).await;
	assert_eq!(first.0, 200, "{}", first.1);
	assert_eq!(
		first,
		request(&p.aa, &p.token, "POST", &path, input.clone()).await
	);
	let mut changed = input.clone();
	changed["content"] = json!("Cannot replace an admitted message");
	assert_eq!(
		request(&p.aa, &p.token, "POST", &path, changed).await.0,
		409
	);
	assert_eq!(p.b.store.run_inputs(p.admission).await.unwrap().len(), 1);
	let task = p.a.store.task(p.task).await.unwrap();
	assert_eq!(
		p.a.store
			.snapshot(task.workspace_id)
			.await
			.unwrap()
			.messages
			.iter()
			.filter(|m| m.content == input["content"])
			.count(),
		1
	);
	let control = format!("/api/tasks/{}/remote-grants/{}/control", p.task, p.grant);
	assert_eq!(
		request(&p.aa, &p.token, "POST", &control, json!({"action":"pause"}))
			.await
			.0,
		200
	);
	assert_eq!(p.run().await.control.as_str(), "PAUSED");
	let states = request(
		&p.aa,
		&p.token,
		"GET",
		&format!("/api/tasks/{}/remote-executions", p.task),
		json!({}),
	)
	.await;
	assert_eq!(states.0, 200, "{}", states.1);
	assert_eq!(states.1[0]["execution"]["control"], "PAUSED");
	assert_eq!(
		request(
			&p.aa,
			&p.token,
			"POST",
			&control,
			json!({"action":"resume"})
		)
		.await
		.0,
		200
	);
	for _ in 0..10 {
		if p.run().await.phase().as_str() == "COMPLETED" {
			break;
		}
		p.step().await;
		let run = p.run().await;
		assert_eq!(
			run.control.as_str(),
			"ACTIVE",
			"{} {:?}",
			run.phase().as_str(),
			run.error
		);
		assert!(run.error.is_none(), "{:?}", run.error);
	}
	assert_eq!(p.run().await.phase().as_str(), "COMPLETED");
	assert!(
		p.requests
			.lock()
			.await
			.iter()
			.any(|r| r.to_string().contains("Keep this accepted correction"))
	);
	assert_eq!(first, request(&p.aa, &p.token, "POST", &path, input).await);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn revocation_during_remote_inference_discards_response_and_cancel_closes_both_sides(
	#[future(awt)] scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.step().await;
	p.model.hold.store(true, Ordering::Release);
	let worker = Harness {
		federation: p.b.clone(),
	};
	let step = tokio::spawn(async move { worker.worker_once().await.unwrap() });
	// Retained bindings require several bounded peer checks before provider HTTP.
	// Match Pair::step's combined fixture budget on loaded CI runners.
	tokio::time::timeout(
		std::time::Duration::from_secs(60),
		p.model.entered.notified(),
	)
	.await
	.expect("remote inference must reach the held provider within the fixture budget");
	assert_eq!(
		request(
			&p.aa,
			&p.token,
			"POST",
			&format!("/api/tasks/{}/remote-grants/{}/revoke", p.task, p.grant),
			json!({})
		)
		.await
		.0,
		200
	);
	p.model.release.notify_one();
	assert!(
		tokio::time::timeout(std::time::Duration::from_secs(60), step)
			.await
			.expect("revoked inference must finish within the fixture budget")
			.unwrap()
	);
	assert_eq!(p.run().await.control.as_str(), "PAUSED");
	let task = p.a.store.task(p.task).await.unwrap();
	let snapshot = p.a.store.snapshot(task.workspace_id).await.unwrap();
	assert!(snapshot.artifacts.is_empty());
	assert!(snapshot.messages.is_empty());
	let control = format!("/api/tasks/{}/remote-grants/{}/control", p.task, p.grant);
	assert_eq!(
		request(
			&p.aa,
			&p.token,
			"POST",
			&control,
			json!({"action":"resume"})
		)
		.await
		.0,
		403
	);
	let cancelled = request(
		&p.aa,
		&p.token,
		"POST",
		&control,
		json!({"action":"cancel"}),
	)
	.await;
	assert_eq!(cancelled.0, 200, "{}", cancelled.1);
	p.step().await;
	assert_eq!(p.run().await.phase().as_str(), "CANCELLED");
	assert_eq!(
		p.a.store.task(p.task).await.unwrap().status.as_str(),
		"CANCELLED"
	);
	assert_eq!(
		request(
			&p.aa,
			&p.token,
			"POST",
			&control,
			json!({"action":"cancel"})
		)
		.await
		.0,
		200
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn peer_outage_and_both_node_restarts_reconcile_one_scoped_execution(
	#[future(awt)] scoped_pair: Pair,
	#[from(signal_state)] source_notify: Arc<Notify>,
	#[from(signal_state)] receiver_notify: Arc<Notify>,
) {
	let mut p = scoped_pair;
	p.step().await;
	for server in p.servers.drain(..) {
		server.abort();
		let _ = server.stopped().await;
	}
	p.step().await;
	assert!(p.run().await.recovery.retry.is_some());
	assert!(p.run().await.error.is_some());
	assert!(p.requests.lock().await.is_empty());
	assert!(
		p.a.store
			.snapshot(p.a.store.task(p.task).await.unwrap().workspace_id)
			.await
			.unwrap()
			.artifacts
			.is_empty()
	);
	reconnect(&mut p.a, source_notify).await;
	reconnect(&mut p.b, receiver_notify).await;
	// Act: rebuild both native applications over the reconnected durable stores.
	p.aa = common::application(p.a.clone()).await;
	p.ba = common::application(p.b.clone()).await;
	for (f, application) in [(&p.a, p.aa.clone()), (&p.b, p.ba.clone())] {
		let app: Arc<dyn reinhardt::Handler> = Arc::new(
			aidash_server::routes()
				.into_server()
				.with_di_context(application.context.clone())
				.with_middleware(SourceReplyMiddleware(p.drop_reply.clone())),
		);
		let endpoint = reqwest::Url::parse(&f.config.endpoint).unwrap();
		let listener = tokio::net::TcpListener::bind(("127.0.0.1", endpoint.port().unwrap()))
			.await
			.unwrap();
		// Act: replace the stopped peer transport at its original address.
		p.servers.push(upstream_fixtures::FixedServerGuard::spawn(
			Arc::new(listener),
			app,
			Some(application.context.clone()),
		));
	}
	let replay = request(&p.aa, &p.token, "POST", &p.activation(), json!({})).await;
	assert_eq!(replay.0, 200, "{}", replay.1);
	assert_eq!(replay.1["run_id"], p.admission.to_string());
	assert_eq!(
		replay.1["control"], "ACTIVE",
		"activation preserves the durable control state"
	);
	let resumed = request(
		&p.aa,
		&p.token,
		"POST",
		&format!("/api/tasks/{}/remote-grants/{}/control", p.task, p.grant),
		json!({"action":"resume"}),
	)
	.await;
	assert_eq!(resumed.0, 200, "{}", resumed.1);
	for _ in 0..10 {
		if p.run().await.phase().as_str() == "COMPLETED" {
			break;
		}
		p.step().await;
	}
	assert_eq!(p.run().await.phase().as_str(), "COMPLETED");
	let runs = p.b.store.runs().await.unwrap();
	assert_eq!(
		runs.iter()
			.filter(|run| run.home_node == p.a.config.node_id && run.task_id == p.task)
			.count(),
		1
	);
	let count: i64 = {
		let query_bind_1 = p.grant;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("authorization_remote_admissions"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(grant_id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(count, 1);
	let snapshot =
		p.a.store
			.snapshot(p.a.store.task(p.task).await.unwrap().workspace_id)
			.await
			.unwrap();
	assert_eq!(snapshot.artifacts.len(), 1);
	assert_eq!(
		snapshot
			.messages
			.iter()
			.filter(|message| message.content == "Scoped remote progress")
			.count(),
		1
	);
	assert_eq!(p.requests.lock().await.len(), 2);
	p.close().await;
}

#[rstest::rstest]
#[case("message")]
#[case("run_message_complete")]
#[tokio::test]
async fn lost_home_effect_reply_reconciles_without_duplicate_effects(
	#[case] operation: &str,
	#[future(awt)] scoped_pair: Pair,
) {
	let p = scoped_pair;
	*p.drop_reply.lock().await = Some(operation.to_owned());
	for _ in 0..15 {
		if p.run().await.phase().as_str() == "COMPLETED" {
			break;
		}
		p.step().await;
		let run = p.run().await;
		if run.control == aidash_server::domain::RunControl::Paused {
			let response = request(
				&p.aa,
				&p.token,
				"POST",
				&format!("/api/tasks/{}/remote-grants/{}/control", p.task, p.grant),
				json!({"action":"resume"}),
			)
			.await;
			assert_eq!(response.0, 200, "{}", response.1);
		}
	}
	assert!(
		p.drop_reply.lock().await.is_none(),
		"the fault must be exercised"
	);
	assert_eq!(
		p.run().await.phase().as_str(),
		"COMPLETED",
		"{:?}",
		p.run().await.error
	);
	let snapshot =
		p.a.store
			.snapshot(p.a.store.task(p.task).await.unwrap().workspace_id)
			.await
			.unwrap();
	assert_eq!(snapshot.artifacts.len(), 1);
	assert_eq!(
		snapshot
			.messages
			.iter()
			.filter(|message| message.content == "Scoped remote progress")
			.count(),
		1
	);
	assert_eq!(
		p.requests.lock().await.len(),
		2,
		"completed inference is never replayed after a lost home reply"
	);
	p.close().await;
}

async fn prepare_generated_pair(
	a: &Federation,
	b: &Federation,
	aa: &common::TestApplication,
	ba: &common::TestApplication,
	token: &str,
	task: Uuid,
	(approval, semantic, native): (bool, bool, bool),
) -> (Uuid, Value, Value) {
	let workspace = a.store.task(task).await.unwrap().workspace_id;
	let (_, mut parent_template) = request(
		aa,
		&a.config.api_token,
		"GET",
		"/api/registry/research/1.0.0",
		Value::Null,
	)
	.await;
	let (_, child_template) = request(
		ba,
		&b.config.api_token,
		"GET",
		if native {
			"/api/registry/research-native/1.0.0"
		} else {
			"/api/registry/research/1.0.0"
		},
		Value::Null,
	)
	.await;
	parent_template["capabilities"] = json!(["unique-parent-specialist"]);
	let model_ref = child_template["config"]["model"].clone();
	let (_, model) = request(
		ba,
		&b.config.api_token,
		"GET",
		&format!(
			"/api/registry/{}/{}",
			model_ref["id"].as_str().unwrap(),
			model_ref["version"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	let descriptor = |node: &str, entry: &Value| json!({"node_id":node,"entry":{"id":entry["id"],"version":entry["version"]},"digest":aidash_server::registry::digest(entry),"configuration_digest":aidash_server::registry::digest(&entry["config"])});
	let embedding_descriptor = if semantic {
		let (status, embedding) = request(
			aa,
			&a.config.api_token,
			"GET",
			"/api/registry/home-embedding/1.0.0",
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{embedding}");
		Some(descriptor(&a.config.node_id, &embedding))
	} else {
		None
	};
	let model_descriptor = descriptor(&b.config.node_id, &model);
	let (_, compactor) = request(
		ba,
		&b.config.api_token,
		"GET",
		"/api/registry/remote-compactor/1.0.0",
		Value::Null,
	)
	.await;
	let compactor_descriptor = descriptor(&b.config.node_id, &compactor);
	let common = json!({"enabled":true,"permissions":{"roles":[],"groups":[],"attributes":{}},"approval_required":false,"limits":{"max_agents":4,"max_concurrent":4,"max_depth":4,"token_budget":4000000,"tokens_per_agent":800000,"lifetime_seconds":3600}});
	let mut parent_spec = common.clone();
	parent_spec["template"] = parent_template;
	if semantic {
		parent_spec["embedding"] = json!({"provider":{"id":"home-embedding","version":"1.0.0"},"calls_per_agent":10,"call_budget":40});
	}
	parent_spec["remote"] = json!({"inference":[model_descriptor],"compaction":{"provider":compactor_descriptor,"calls_per_agent":2,"call_budget":8},"memory":if native {vec![embedding_descriptor.clone().unwrap()]} else {vec![]}});
	let (status, body) = request(
		aa,
		&a.config.api_token,
		"POST",
		"/api/generation/acme/policies/remote-parent",
		json!({"expected_revision":0,"spec":parent_spec}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status,parent)=request(aa,token,"POST",&format!("/api/workspaces/{workspace}/tasks"),json!({"title":"Generated origin","description":"Delegate scoped memory research","requirements":{"capability":"unique-parent-specialist"}})).await;
	assert_eq!(status, 200, "{parent}");
	let parent_id: Uuid = serde_json::from_value(parent["id"].clone()).unwrap();
	let (status, parent) = request(
		aa,
		token,
		"POST",
		&format!("/api/generation/acme/tasks/{parent_id}/assign"),
		json!({"policy_id":"remote-parent","reason":"ancestor restriction fixture"}),
	)
	.await;
	assert_eq!(status, 200, "{parent}");
	assert_eq!(parent["kind"], "generated");
	aidash_server::generation::provision::reconcile(a)
		.await
		.unwrap();
	let parent_run: Uuid = {
		let query_bind_1 = parent_id;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(a.store.pool.driver())
		.await
	}
	.unwrap();
	let parent_run = a.store.run(parent_run).await.unwrap();
	// Seed the trusted origin to isolate the two-owner allowance assertions.
	// The cluster driver creates this child through the actual task_create tool.
	let chain = vec![
		"alice".to_owned(),
		qualified_agent(
			&a.config.node_id,
			&parent_run.agent_id,
			&parent_run.agent_version,
		),
	];
	{
		let query_bind_1 = task;
		let query_bind_2 = parent_run.id;
		let query_bind_3 = chain;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("authorization_task_origins"))
				.columns(
					[
						"task_id",
						"source_run_id",
						"tenant",
						"root_subject",
						"subject_chain",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(Expr::cust("'acme'"))
						.expr(Expr::cust("'alice'"))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![
								Expr::value(reinhardt::query::Value::Array(
									reinhardt::query::ArrayType::String,
									Some(Box::new(
										query_bind_3
											.into_iter()
											.map(reinhardt::query::IntoValue::into_value)
											.collect(),
									)),
								))
								.into(),
							],
						))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(a.store.pool.driver())
		.await
	}
	.unwrap();
	let mut child_spec = common;
	child_spec["approval_required"] = json!(approval);
	child_spec["template"] = child_template;
	child_spec["compaction"] = json!({"provider":{"id":"remote-compactor","version":"1.0.0"},"calls_per_agent":2,"call_budget":8});
	if let Some(embedding_descriptor) = embedding_descriptor {
		child_spec["remote"] = json!({"embedding":{"provider":embedding_descriptor,"calls_per_agent":10,"call_budget":40},"memory":if native {vec![embedding_descriptor.clone()]} else {vec![]}});
	}
	let (status, body) = request(
		ba,
		&b.config.api_token,
		"POST",
		"/api/generation/acme/policies/remote-child",
		json!({"expected_revision":0,"spec":child_spec}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let input = json!({"id":Uuid::new_v4(),"node_id":b.config.node_id,"policy_id":"remote-child","policy_revision":1,"ttl_seconds":600,"reason":"foreign task specialist"});
	let route = format!("/api/tasks/{task}/remote-generation");
	let (status, prepared) = request(aa, token, "POST", &route, input.clone()).await;
	assert_eq!(status, 200, "{prepared}");
	assert_eq!(prepared["prepared"], !approval);
	let (status, replayed) = request(aa, token, "POST", &route, input.clone()).await;
	assert_eq!(status, 200, "{replayed}");
	assert_eq!(replayed, prepared);
	let count: i64 = {
		let query_bind_1 = task;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("tasks"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(
		count, 0,
		"foreign preparation cannot create a fake local task"
	);
	let count: i64 = {
		let query_bind_1 = &a.config.node_id;
		let query_bind_2 = task;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(home_node=? AND task_id=?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(count, 0, "preparation cannot activate before the grant");
	(task, input, prepared)
}

#[rstest::rstest]
#[tokio::test]
async fn foreign_generation_waits_for_approval_and_replays_one_exact_definition(
	#[with(true, true, true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	let (input, pending) = p.generation.as_ref().unwrap();
	assert_eq!(pending["status"], "PENDING_APPROVAL");
	let route = format!("/api/tasks/{}/remote-generation", p.task);
	let (first, second) = tokio::join!(
		request(&p.aa, &p.token, "POST", &route, input.clone()),
		request(&p.aa, &p.token, "POST", &route, input.clone()),
	);
	for (status, body) in [first, second] {
		assert_eq!(status, 200, "{body}");
		assert_eq!(body, *pending);
	}
	let grant = json!({"id":p.grant,"node_id":p.b.config.node_id,"agent":pending["agent"],"ttl_seconds":300,"semantic":{"mode":"required_home","embedding":{"id":"home-embedding","version":"1.0.0"}}});
	let grant_route = format!("/api/tasks/{}/remote-grants", p.task);
	let mut nil_grant = grant.clone();
	nil_grant["id"] = json!(Uuid::nil());
	assert_eq!(
		request(&p.aa, &p.token, "POST", &grant_route, nil_grant)
			.await
			.0,
		400
	);
	assert_ne!(
		request(&p.aa, &p.token, "POST", &grant_route, grant.clone())
			.await
			.0,
		200
	);
	let control = format!(
		"/api/generation/acme/requests/{}/control",
		pending["request_id"].as_str().unwrap()
	);
	let decision = json!({"action":"approve","reason":"Approve the exact foreign preparation"});
	for _ in 0..2 {
		let (status, body) = request(
			&p.ba,
			&p.b.config.api_token,
			"POST",
			&control,
			decision.clone(),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		assert_eq!(body["status"], "QUEUED");
	}
	let (first, second) = tokio::join!(
		request(&p.aa, &p.token, "POST", &route, input.clone()),
		request(&p.aa, &p.token, "POST", &route, input.clone()),
	);
	for (status, body) in [first, second] {
		assert_eq!(status, 200, "{body}");
		assert_eq!(body["prepared"], true);
		assert_eq!(body["agent"], pending["agent"]);
		assert_eq!(body["request_id"], pending["request_id"]);
	}
	let count: i64 = {
		let query_bind_1 = &p.a.config.node_id;
		let query_bind_2 = p.task;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(home_node=? AND task_id=?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(
		count, 0,
		"approval publishes the definition but cannot activate a Run"
	);
	let (status, body) = request(&p.aa, &p.token, "POST", &grant_route, grant).await;
	assert_eq!(status, 200, "{body}");
	let (status, activated) = request(&p.aa, &p.token, "POST", &p.activation(), json!({})).await;
	assert_eq!(status, 200, "{activated}");
	let (status, replayed) = request(&p.aa, &p.token, "POST", &p.activation(), json!({})).await;
	assert_eq!(status, 200, "{replayed}");
	assert_eq!(replayed["admission_id"], activated["admission_id"]);
	let rows: (i64, i64) = {
		let query_bind_1 = &p.a.config.node_id;
		let query_bind_2 = p.task;
		sqlx::query_as(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.expr(Expr::cust("COUNT(DISTINCT agent_id)"))
				.from(Alias::new("generation_requests"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(home_node=? AND task_id=? AND status='ACTIVE')".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(rows, (1, 1));
	p.close().await;
}

#[rstest::rstest]
#[case("deny", "DENIED")]
#[case("cancel", "STOPPED")]
#[case("expire", "EXPIRED")]
#[tokio::test]
async fn foreign_preparation_termination_releases_unused_allocations_once(
	#[with(true, true, true)]
	#[future(awt)]
	scoped_pair: Pair,
	#[case] action: &str,
	#[case] expected: &str,
) {
	let p = scoped_pair;
	let (_, pending) = p.generation.as_ref().unwrap();
	let job: Uuid = serde_json::from_value(pending["request_id"].clone()).unwrap();
	let route = format!("/api/generation/acme/requests/{job}/control");
	for _ in 0..2 {
		match action {
			"deny" => {
				let (status, body) = request(
					&p.ba,
					&p.b.config.api_token,
					"POST",
					&route,
					json!({"action":"deny","reason":"Decline foreign preparation"}),
				)
				.await;
				assert_eq!(status, 200, "{body}");
			}
			"cancel" => {
				let route = format!(
					"/api/tasks/{}/remote-generation/{}/cancel",
					p.task,
					pending["intent_id"].as_str().unwrap()
				);
				let (status, body) = request(&p.aa, &p.token, "POST", &route, json!({})).await;
				assert_eq!(status, 200, "{body}");
			}
			"expire" => {
				{
					let query_bind_1 = job;
					sqlx::query(
						&Query::update()
							.table(Alias::new("generation_requests"))
							.value_expr(
								Alias::new("expires_at"),
								Expr::cust("CLOCK_TIMESTAMP()-INTERVAL '1 second'"),
							)
							.and_where(SimpleExpr::CustomWithExpr(
								"(id=?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.to_string(PostgresQueryBuilder),
					)
					.execute(p.b.store.pool.driver())
					.await
				}
				.unwrap();
				aidash_server::generation::provision::reconcile(&p.b)
					.await
					.unwrap();
			}
			_ => unreachable!(),
		}
	}
	let state: (String, bool, Option<Uuid>) = {
		let query_bind_1 = job;
		sqlx::query_as(
			&Query::select()
				.columns(["status", "quota_released", "admission_id"].map(Alias::new))
				.from(Alias::new("generation_requests"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(state, (expected.into(), true, None));
	let budgets: (i64, i64, i64) = sqlx::query_as(
		&Query::select()
			.columns(
				[
					"allocated_tokens",
					"allocated_embedding_calls",
					"allocated_compaction_calls",
				]
				.map(Alias::new),
			)
			.from(Alias::new("generation_policies"))
			.and_where(Expr::cust("tenant='acme' AND id='remote-child'"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.b.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(
		budgets,
		(0, 0, 0),
		"only unused allocations are released, exactly once"
	);
	let (status, _) = request(
		&p.ba,
		&p.b.config.api_token,
		"POST",
		&route,
		json!({"action":"approve","reason":"Late approval must not resurrect terminated work"}),
	)
	.await;
	assert_eq!(status, 409);
	assert_eq!(p.requests.lock().await.len(), 0);
	assert_eq!(
		p.semantic.as_ref().unwrap().requests.lock().await.len(),
		4,
		"preparation makes no query embedding call"
	);
	let (input, _) = p.generation.as_ref().unwrap();
	let mut replacement = input.clone();
	replacement["id"] = json!(Uuid::new_v4());
	let (status, fresh) = request(
		&p.aa,
		&p.token,
		"POST",
		&format!("/api/tasks/{}/remote-generation", p.task),
		replacement,
	)
	.await;
	assert_eq!(status, 200, "{fresh}");
	assert_ne!(fresh["request_id"], pending["request_id"]);
	let retained: i64 = {
		let query_bind_1 = &p.a.config.node_id;
		let query_bind_2 = p.task;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("generation_requests"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(home_node=? AND task_id=?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(retained, 2, "terminal history must survive replacement");
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn cancelled_foreign_intent_retries_delivery_after_an_uncertain_ack(
	#[future(awt)]
	#[with(true, true, true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	let (_, prepared) = p.generation.as_ref().unwrap();
	let intent: Uuid = serde_json::from_value(prepared["intent_id"].clone()).unwrap();
	let route = format!("/api/tasks/{}/remote-generation/{intent}/cancel", p.task);
	assert_eq!(
		request(&p.aa, &p.token, "POST", &route, json!({})).await.0,
		200
	);
	let delivered = || {
		let pool = p.a.store.pool.driver().clone();
		async move {
			{
				let query_bind_1 = intent;
				sqlx::query_scalar::<_, bool>(
					&Query::select()
						.column(Alias::new("cancel_delivered"))
						.from(Alias::new("generation_remote_intents"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&pool)
				.await
			}
			.unwrap()
		}
	};
	assert!(delivered().await);
	// Model an acknowledgement lost before Home persisted its delivery mark.
	{
		let query_bind_1 = intent;
		sqlx::query(
			&Query::update()
				.table(Alias::new("generation_remote_intents"))
				.value(Alias::new("cancel_delivered"), false)
				.value_expr(
					Alias::new("cancel_retry_at"),
					Expr::cust("CLOCK_TIMESTAMP()-INTERVAL '1 second'"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(p.a.store.pool.driver())
		.await
	}
	.unwrap();
	aidash_server::generation::provision::reconcile(&p.a)
		.await
		.unwrap();
	assert!(delivered().await);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn generated_foreign_executor_and_home_ancestor_share_durable_provider_allowances(
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
) {
	let pair = scoped_pair;
	let expectation = ReservationCheck {
		pools: vec![
			pair.a.store.pool.driver().clone(),
			pair.b.store.pool.driver().clone(),
		],
		dispatcher: pair.a.store.pool.driver().clone(),
		grant: pair.grant,
		admission: pair.admission,
		purpose: "embedding",
	};
	*pair.semantic.as_ref().unwrap().reservations.lock().await = Some(expectation.clone());
	*pair.model.reservations.lock().await = Some(ReservationCheck {
		dispatcher: pair.b.store.pool.driver().clone(),
		purpose: "inference",
		..expectation
	});

	for _ in 0..4 {
		pair.step().await;
		if !pair.requests.lock().await.is_empty() || pair.run().await.control.as_str() == "PAUSED" {
			break;
		}
	}
	let requests = pair.requests.lock().await;
	assert_eq!(requests.len(), 1, "{:?}", pair.run().await);
	let input: Value =
		serde_json::from_str(requests[0]["messages"][1]["content"].as_str().unwrap()).unwrap();
	assert!(
		input["current"]["semantic_memory"]
			.to_string()
			.contains("ochre falcon")
	);
	drop(requests);

	for (dispatcher, owner) in [(&pair.a, &pair.b), (&pair.b, &pair.a)] {
		let records: Vec<(Value, Value)> = sqlx::query_as(
			&Query::select()
				.columns(["usage", "finalization"].map(Alias::new))
				.from(Alias::new("generation_remote_dispatches"))
				.and_where(Expr::cust("state='SETTLED'"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(dispatcher.store.pool.driver())
		.await
		.unwrap();
		assert_eq!(records.len(), 1);
		for (usage, finalization) in records {
			let client = dispatcher.client.clone();
			for _ in 0..3 {
				let response = client
					.post(format!(
						"{}/federation/v0.1/scoped/usage/finalize",
						owner.config.endpoint
					))
					.bearer_auth(std::env::var("AIDASH_SECRET_TEST_PEER").unwrap())
					.header("x-aidash-node", &dispatcher.config.node_id)
					.header("x-aidash-protocol", "0.2")
					.json(&json!({"usage":usage,"result":finalization}))
					.send()
					.await
					.unwrap();
				assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
			}
		}
	}
	for node in [&pair.a, &pair.b] {
		let usage: Vec<(String, String, i64, Option<i64>)> = {
			let query_bind_1 = pair.grant;
			let query_bind_2 = pair.admission;
			sqlx::query_as(
				&Query::select()
					.columns(
						["purpose", "state", "reserved_tokens", "reported_tokens"].map(Alias::new),
					)
					.from(Alias::new("generation_remote_usage"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(grant_id=? AND admission_id=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.order_by(Alias::new("purpose"), reinhardt::query::Order::Asc)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(node.store.pool.driver())
			.await
		}
		.unwrap();
		assert_eq!(
			usage.len(),
			2,
			"both embedding and inference must debit {}: {usage:?}",
			node.config.node_id
		);
		let finalized: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("generation_remote_finalizations"))
				.and_where(Expr::cust("result IS NOT NULL"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(node.store.pool.driver())
		.await
		.unwrap();
		assert_eq!(finalized, 2, "each owner retains a terminal attempt fence");
		assert_eq!(usage[0].0, "embedding");
		assert_eq!(usage[0].1, "SETTLED");
		assert_eq!(usage[0].3, Some(1));
		assert_eq!(usage[1].0, "inference");
		assert_eq!(usage[1].1, "SETTLED");
		assert_eq!(usage[1].3, Some(2));
		let budget: (i64, i64) = sqlx::query_as(
			&Query::select()
				.columns(["used_tokens", "embedding_calls"].map(Alias::new))
				.from(Alias::new("generation_budgets"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(node.store.pool.driver())
		.await
		.unwrap();
		assert_eq!(
			budget,
			(3, 1),
			"exact one-call settlement at {}",
			node.config.node_id
		);
		let (app, token, route) = if node.config.node_id == pair.a.config.node_id {
			(
				&pair.aa,
				&pair.token,
				format!(
					"/api/tasks/{}/remote-grants/{}/semantic",
					pair.task, pair.grant
				),
			)
		} else {
			(
				&pair.ba,
				&pair.receiver_token,
				format!("/api/runs/{}/semantic", pair.admission),
			)
		};
		let (status, view) = request(app, token, "GET", &route, Value::Null).await;
		assert_eq!(status, 200, "{view}");
		assert_eq!(view["allowance_node"], node.config.node_id);
		assert_eq!(view["allowances"].as_array().unwrap().len(), 1);
		assert_eq!(view["allowances"][0]["used_tokens"], 3);
		assert_eq!(view["allowances"][0]["embedding_calls"], 1);
		let (status, usage) = request(
			app,
			token,
			"GET",
			&format!(
				"/api/generation/acme/requests/{}/usage",
				view["allowances"][0]["request_id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{usage}");
		assert_eq!(usage["inference_attempts"], 1);
	}
	pair.close().await;
}

#[rstest::rstest]
#[case(-1)]
#[case(1)]
#[tokio::test]
async fn embedding_callback_rejects_a_nonexact_reservation(
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
	#[case] delta: i64,
) {
	let p = scoped_pair;
	for _ in 0..4 {
		p.step().await;
		if !p.requests.lock().await.is_empty() {
			break;
		}
	}
	let (mut usage, boundary): (Value, Value) = sqlx::query_as(
		&Query::select()
			.columns(["usage", "boundary"].map(Alias::new))
			.from(Alias::new("generation_remote_dispatches"))
			.and_where(Expr::cust("usage->>'purpose'='embedding'"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.a.store.pool.driver())
	.await
	.unwrap();
	let exact = boundary["query"].as_str().unwrap().len() as i64 + 1024;
	assert_eq!(usage["reserved_tokens"], exact);
	usage["reserved_tokens"] = json!(exact + delta);
	usage["attempt_id"] = json!(Uuid::new_v4());
	let response =
		p.a.client
			.clone()
			.post(format!(
				"{}/federation/v0.1/scoped/usage/reserve",
				p.b.config.endpoint
			))
			.bearer_auth(std::env::var("AIDASH_SECRET_TEST_PEER").unwrap())
			.header("x-aidash-node", &p.a.config.node_id)
			.header("x-aidash-protocol", "0.2")
			.json(&json!({"usage":usage,"boundary":boundary}))
			.send()
			.await
			.unwrap();
	assert_eq!(response.status(), 403, "{}", response.text().await.unwrap());
	let count: i64 = {
		let query_bind_1 = usage["attempt_id"]
			.as_str()
			.unwrap()
			.parse::<Uuid>()
			.unwrap();
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("generation_remote_usage"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(attempt_id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(count, 0, "rejected callbacks must not debit an ancestor");
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn operator_polling_advances_past_hidden_event_pages(
	#[future(awt)]
	#[with(true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	let workspace = p.a.store.task(p.task).await.unwrap().workspace_id;
	let cursor: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COALESCE(MAX(sequence),0)"))
			.from(Alias::new("events"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.a.store.pool.driver())
	.await
	.unwrap();
	let mut tx = p.a.store.pool.begin().await.unwrap();
	p.a.store
		.event(
			&mut tx,
			None,
			"test.visible",
			json!({"marker":"before-hidden-burst"}),
		)
		.await
		.unwrap();
	for _ in 0..501 {
		p.a.store
			.event(
				&mut tx,
				Some(workspace),
				"test.hidden",
				json!({"secret":"hidden-burst"}),
			)
			.await
			.unwrap();
	}
	p.a.store
		.event(
			&mut tx,
			None,
			"test.visible",
			json!({"marker":"after-hidden-burst"}),
		)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	let (status, page) = request(
		&p.aa,
		&p.a.config.api_token,
		"GET",
		&format!("/api/events?after={cursor}"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{page}");
	assert!(
		page.to_string().contains("after-hidden-burst"),
		"operator polling stalled behind hidden events: {page}"
	);
	assert!(!page.to_string().contains("\"secret\""));
	let (status, state) = request(
		&p.aa,
		&p.a.config.api_token,
		"GET",
		"/api/state",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{state}");
	assert!(
		state["events"].to_string().contains("before-hidden-burst"),
		"{state}"
	);
	assert!(!state["events"].to_string().contains("\"secret\""));

	// A full visible page may span hidden candidate pages. Its cursor must
	// retain visible events beyond the response limit for the next request.
	let mut tx = p.a.store.pool.begin().await.unwrap();
	for index in 0..600 {
		p.a.store
			.event(&mut tx, None, "test.visible", json!({"index": index}))
			.await
			.unwrap();
	}
	tx.commit().await.unwrap();
	let first = async {
		let client = &(p.aa.client());
		let mut request = client.request(http::Method::GET, &format!("/api/events?after={cursor}"));
		request = request.header("authorization", format!("Bearer {}", p.a.config.api_token));
		request.send().await
	}
	.await
	.unwrap();
	assert_eq!(first.status(), 200);
	let first_cursor: i64 = first.headers()["x-aidash-event-cursor"]
		.to_str()
		.unwrap()
		.parse()
		.unwrap();
	let first_events: Vec<Value> = serde_json::from_slice(first.body()).unwrap();
	assert_eq!(first_events.len(), 500);
	assert_eq!(first_events.last().unwrap()["sequence"], first_cursor);
	let second = async {
		let client = &(p.aa.client());
		let mut request = client.request(
			http::Method::GET,
			&format!("/api/events?after={first_cursor}"),
		);
		request = request.header("authorization", format!("Bearer {}", p.a.config.api_token));
		request.send().await
	}
	.await
	.unwrap();
	assert_eq!(second.status(), 200);
	let second_events: Vec<Value> = serde_json::from_slice(second.body()).unwrap();
	let indices: Vec<i64> = first_events
		.iter()
		.chain(&second_events)
		.filter_map(|event| event["data"]["index"].as_i64())
		.collect();
	assert_eq!(indices, (0..600).collect::<Vec<_>>());
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn operator_content_views_cannot_bypass_both_node_subject_authority(
	#[future(awt)]
	#[with(true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	for _ in 0..8 {
		p.step().await;
		if p.run().await.phase().as_str() == "COMPLETED" {
			break;
		}
	}
	assert_eq!(p.run().await.phase().as_str(), "COMPLETED");
	let task = p.a.store.task(p.task).await.unwrap();
	let (status, viewer) = request(
		&p.ba,
		&p.receiver_token,
		"GET",
		&format!("/api/runs/{}", p.admission),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{viewer}");
	assert_eq!(viewer["run"]["id"], p.admission.to_string());
	assert!(
		p.model.requests.lock().await[0]
			.to_string()
			.contains("ochre falcon")
	);
	for (f, app) in [(&p.a, &p.aa), (&p.b, &p.ba)] {
		for path in [
			"/api/state".to_owned(),
			"/api/events".into(),
			"/api/tasks".into(),
		] {
			let (status, body) = request(app, &f.config.api_token, "GET", &path, Value::Null).await;
			assert_eq!(status, 200, "{path}: {body}");
			for hidden in [
				"ochre falcon",
				"Scoped remote result",
				"Scoped remote progress",
			] {
				assert!(
					!body.to_string().contains(hidden),
					"operator path {} {path} leaked {hidden}: {:?}",
					f.config.node_id,
					body.as_object().map(|fields| fields
						.iter()
						.filter(|(_, value)| value.to_string().contains(hidden))
						.collect::<Vec<_>>())
				);
			}
		}
		// The notification-backed stream must enforce the same boundary at
		// delivery, including receiver Run events without a Workspace FK.
		let (status, body) = request(
			app,
			&f.config.api_token,
			"POST",
			"/api/workspaces",
			json!({"title":"operator-stream-tail","goal":"Independent stream marker"}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		// Unbounded SSE uses the fixture-owned streaming client without buffering the body.
		let response = app
			.streaming_http
			.get(app.url("/api/events/stream"))
			.bearer_auth(&f.config.api_token)
			.send()
			.await
			.unwrap();
		assert_eq!(response.status(), 200);
		let mut stream = response.bytes_stream();
		tokio::time::timeout(std::time::Duration::from_secs(20), async {
			loop {
				let chunk = stream.next().await.unwrap().unwrap();
				let frame = String::from_utf8_lossy(&chunk);
				for hidden in [
					"ochre falcon",
					"Scoped remote result",
					"Scoped remote progress",
				] {
					assert!(
						!frame.contains(hidden),
						"operator stream leaked {hidden}: {frame}"
					);
				}
				if frame.contains("operator-stream-tail") {
					break;
				}
			}
		})
		.await
		.unwrap();
		drop(stream);
	}
	for path in [
		format!("/api/workspaces/{}", task.workspace_id),
		format!("/api/workspaces/{}/message-history", task.workspace_id),
	] {
		assert_eq!(
			request(&p.aa, &p.a.config.api_token, "GET", &path, Value::Null)
				.await
				.0,
			403
		);
	}
	assert_eq!(
		request(
			&p.ba,
			&p.b.config.api_token,
			"GET",
			&format!("/api/runs/{}", p.admission),
			Value::Null
		)
		.await
		.0,
		403
	);
	let (status, control) = request(
		&p.ba,
		&p.b.config.api_token,
		"GET",
		&format!("/api/runs/{}/management", p.admission),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{control}");
	assert!(control.get("context").is_none());
	assert!(control.get("pending").is_none());
	assert!(control.get("error").is_none());
	let snapshot = p.a.store.snapshot(task.workspace_id).await.unwrap();
	let artifact = snapshot.artifacts.first().unwrap();
	let (status,_)=request(&p.aa,&p.a.config.api_token,"POST",&format!("/api/workspaces/{}/semantic/entries",task.workspace_id),json!({"key":"operator-bypass","expected_revision":0,"source":{"kind":"artifact","id":artifact.id},"metadata":{}})).await;
	assert_eq!(status, 403);
	let (status, independent) = request(
		&p.aa,
		&p.a.config.api_token,
		"POST",
		"/api/workspaces",
		json!({"title":"Independent operator work","goal":"Unrelated legacy access"}),
	)
	.await;
	assert_eq!(status, 200, "{independent}");
	assert_eq!(
		request(
			&p.aa,
			&p.a.config.api_token,
			"GET",
			&format!("/api/workspaces/{}", independent["id"].as_str().unwrap()),
			Value::Null
		)
		.await
		.0,
		200
	);
	p.close().await;
}

#[rstest::rstest]
#[case("COMPLETED", "COMPLETED", None)]
#[case("FAILED", "FAILED", None)]
#[case("CANCELLED", "STOPPED", None)]
#[case("THINKING", "EXPIRED", Some("expire"))]
#[case("THINKING", "STOPPED", Some("stop"))]
#[tokio::test]
async fn generated_foreign_terminal_runs_retain_reads_with_current_dependency_authority(
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
	#[case] phase: &str,
	#[case] expected: &str,
	#[case] control: Option<&str>,
) {
	let p = scoped_pair;
	for _ in 0..8 {
		p.step().await;
		if p.run().await.phase().as_str() == "COMPLETED" {
			break;
		}
	}
	assert_eq!(p.run().await.phase().as_str(), "COMPLETED");
	// Retain produced output, then seed the durable worker terminal cut. The
	// generation lifecycle and every subsequent read use the production paths.
	if phase != "COMPLETED" {
		{
			let query_bind_1 = p.admission;
			let query_bind_2 = phase;
			let query_bind_3 = common::pending(match phase {
				"THINKING" => aidash_server::domain::RunState::Thinking(Default::default()),
				"FAILED" => {
					aidash_server::domain::RunState::Failed(aidash_server::domain::TerminalState {})
				}
				"CANCELLED" => aidash_server::domain::RunState::Cancelled(
					aidash_server::domain::TerminalState {},
				),
				_ => panic!("unexpected fixture phase"),
			});
			sqlx::query(
				&Query::update()
					.table(Alias::new("runs"))
					.value_expr(
						Alias::new("phase"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.value_expr(
						Alias::new("pending"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(p.b.store.pool.driver())
			.await
		}
		.unwrap();
	}
	let job = &p.generation.as_ref().unwrap().1["request_id"];
	if control == Some("expire") {
		{
			let query_bind_1 = serde_json::from_value::<Uuid>(job.clone()).unwrap();
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_requests"))
					.value_expr(
						Alias::new("expires_at"),
						Expr::cust("CLOCK_TIMESTAMP()-INTERVAL '1 second'"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(p.b.store.pool.driver())
			.await
		}
		.unwrap();
	} else if let Some(action) = control {
		let (status, body) = request(
			&p.ba,
			&p.b.config.api_token,
			"POST",
			&format!(
				"/api/generation/acme/requests/{}/control",
				job.as_str().unwrap()
			),
			json!({"action":action,"reason":"Retire an executor while retaining its journal"}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
	aidash_server::generation::provision::reconcile(&p.b)
		.await
		.unwrap();
	let run = p.run().await;
	let actual: String = {
		let query_bind_1 = serde_json::from_value::<Uuid>(job.clone()).unwrap();
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("status"))
				.from(Alias::new("generation_requests"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(actual, expected);
	let catalog: (bool, i64) = {
		let query_bind_1 = &run.agent_id;
		let query_bind_2 = &run.agent_version;
		sqlx::query_as(
			&Query::select()
				.columns(["enabled", "revision"].map(Alias::new))
				.from(Alias::new("authorization_catalog"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(tenant='acme' AND entry_id=? AND entry_version=?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	assert!(
		!catalog.0,
		"terminal lifecycle must retire the generated executor"
	);
	let path = format!("/api/runs/{}", p.admission);
	let (status, view) = request(&p.ba, &p.receiver_token, "GET", &path, Value::Null).await;
	assert_eq!(
		status, 200,
		"terminal execution must remain readable: {view}"
	);
	assert_eq!(view["run"]["id"], json!(p.admission));
	assert_eq!(view["run"]["phase"], phase);
	assert_eq!(view["memory"], Value::Null);
	let home_path = format!("/api/workspaces/{}", run.workspace_id);
	let (status, output) = request(&p.aa, &p.token, "GET", &home_path, Value::Null).await;
	assert_eq!(status, 200, "{output}");
	assert!(
		output.to_string().contains("Scoped remote result"),
		"{output}"
	);
	let (status, revoked) = request(
		&p.ba,
		&p.b.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"model","version":"1.0.0"},"expected_revision":1,"enabled":false}),
	)
	.await;
	assert_eq!(status, 200, "{revoked}");
	assert_ne!(
		request(&p.ba, &p.receiver_token, "GET", &path, Value::Null)
			.await
			.0,
		200
	);
	let (status, restored) = request(
		&p.ba,
		&p.b.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"model","version":"1.0.0"},"expected_revision":2,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{restored}");
	let snapshot = aidash_server::authorization::Authorization {
		pool: p.b.store.pool.clone(),
	}
	.snapshot("acme")
	.await
	.unwrap();
	let mut denied = serde_json::to_value(&snapshot.bundle).unwrap();
	denied["policies"].as_array_mut().unwrap().push(json!({
		"id":"deny-retired-agent-model", "effect":"deny",
		"subjects":{"ids":[aidash_server::domain::qualified_agent(&p.b.config.node_id,&run.agent_id,&run.agent_version)]},
		"actions":["model.infer"], "resources":{"kinds":["model"]}
	}));
	let (status, policy) = request(
		&p.ba,
		&p.b.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":snapshot.revision,"bundle":denied}),
	)
	.await;
	assert_eq!(status, 200, "{policy}");
	assert_ne!(
		request(&p.ba, &p.receiver_token, "GET", &path, Value::Null)
			.await
			.0,
		200,
		"current denies must still govern the retired executor's reads"
	);
	let (status, restored) = request(
		&p.ba,
		&p.b.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":snapshot.revision+1,"bundle":snapshot.bundle}),
	)
	.await;
	assert_eq!(status, 200, "{restored}");
	assert_eq!(
		request(&p.ba, &p.receiver_token, "GET", &path, Value::Null)
			.await
			.0,
		200
	);
	let (status, revoked) = request(
		&p.ba,
		&p.b.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":run.agent_id,"version":run.agent_version},"expected_revision":catalog.1,"enabled":false}),
	)
	.await;
	assert_eq!(status, 200, "{revoked}");
	assert_ne!(
		request(&p.ba, &p.receiver_token, "GET", &path, Value::Null)
			.await
			.0,
		200,
		"explicit catalog revocation must supersede automatic retirement"
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_cross_node_producer_cycle_terminates_and_still_requires_receiver_authority(
	#[future(awt)]
	#[with(true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	for _ in 0..8 {
		p.step().await;
		if p.run().await.phase().as_str() == "COMPLETED" {
			break;
		}
	}
	assert_eq!(p.run().await.phase().as_str(), "COMPLETED");
	let task = p.a.store.task(p.task).await.unwrap();
	let snapshot = p.a.store.snapshot(task.workspace_id).await.unwrap();
	let artifact = snapshot.artifacts.first().unwrap();
	// Reproduce a persisted observation cycle: A's grant reads an A Artifact
	// produced by that grant's B Run. Such provenance must neither recurse over
	// HTTP nor suppress the receiver's current-reader check.
	{
		let query_bind_1 = p.grant;
		let query_bind_2 = task.workspace_id;
		let query_bind_3 = artifact.id;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("authorization_remote_grant_reads"))
				.columns(
					["grant_id", "workspace_id", "resource_kind", "resource_id"].map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(Expr::cust("'artifact'"))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(p.a.store.pool.driver())
		.await
	}
	.unwrap();
	let path = format!("/api/tasks/{}/remote-grants/{}/semantic", p.task, p.grant);
	let (status, body) = tokio::time::timeout(
		std::time::Duration::from_secs(5),
		request(&p.aa, &p.token, "GET", &path, Value::Null),
	)
	.await
	.expect("cross-node cycle must terminate");
	assert_eq!(status, 200, "{body}");
	let mut denied = p.receiver_policy.clone();
	denied["policies"].as_array_mut().unwrap().push(json!({"id":"deny-cycle-read","effect":"deny","subjects":{"any":true},"actions":["run.read"],"resources":{"kinds":["run"]}}));
	assert_eq!(
		request(
			&p.ba,
			&p.b.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":denied})
		)
		.await
		.0,
		200
	);
	let (status, body) = tokio::time::timeout(
		std::time::Duration::from_secs(5),
		request(&p.aa, &p.token, "GET", &path, Value::Null),
	)
	.await
	.expect("denied cross-node cycle must also terminate");
	assert_eq!(status, 403, "{body}");
	let (status, body) = request(
		&p.aa,
		&p.token,
		"GET",
		&format!("/api/workspaces/{}", task.workspace_id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert!(body["artifacts"].as_array().unwrap().is_empty());
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn generated_home_allowance_failure_releases_only_predispatch_receiver_reservations(
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
) {
	let pair = scoped_pair;
	sqlx::query(
		&Query::update()
			.table(Alias::new("generation_budgets"))
			.value(Alias::new("embedding_call_limit"), 0)
			.to_string(PostgresQueryBuilder),
	)
	.execute(pair.a.store.pool.driver())
	.await
	.unwrap();
	for _ in 0..4 {
		pair.step().await;
		if pair.run().await.control.as_str() == "PAUSED" {
			break;
		}
	}
	let run = pair.run().await;
	assert_eq!(run.control.as_str(), "PAUSED", "{run:?}");
	assert_eq!(
		serde_json::to_value(run.recovery.semantic_reason).unwrap(),
		"allowance"
	);
	assert!(pair.requests.lock().await.is_empty());
	assert_eq!(
		pair.semantic.as_ref().unwrap().requests.lock().await.len(),
		4,
		"only pre-existing index embeddings are permitted"
	);
	let debit: (i64, i64) = sqlx::query_as(
		&Query::select()
			.columns(["used_tokens", "embedding_calls"].map(Alias::new))
			.from(Alias::new("generation_budgets"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(pair.b.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(debit, (0, 0));
	let state: String = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("generation_remote_usage"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(pair.b.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(state, "RELEASED");
	pair.close().await;
}

// Act: redirect an existing peer after the test mutates durable reconciliation state.
type PairFuture = futures_util::future::Shared<BoxFuture<'static, Arc<Mutex<Option<Pair>>>>>;
#[rstest::fixture]
fn pair_future(
	#[default(false)] _generated: bool,
	#[with(true, _generated)] scoped_pair: impl std::future::Future<Output = Pair> + Send + 'static,
) -> PairFuture {
	async move { Arc::new(Mutex::new(Some(scoped_pair.await))) }
		.boxed()
		.shared()
}
#[rstest::fixture]
fn status_router(
	#[default(false)] status_available: bool,
	#[from(pair_future)] pair: PairFuture,
	#[from(count_state)] inspections: Arc<AtomicUsize>,
) -> upstream_fixtures::RouterFuture {
	async move {
		let state = pair.await;
		let pair = state.lock().await;
		let p = pair.as_ref().unwrap();
		let activation = json!({"grant_id":p.grant,"admission_id":p.admission,"run_id":p.admission,"phase":"THINKING","control":"ACTIVE","error":null});
		let reply = Arc::new(upstream_fixtures::any_handler(move |request| {
			let activation = activation.clone(); let observed = inspections.clone();
			async move {
				if request.uri.path().ends_with("/scoped/execution/status") {
					return if status_available {reinhardt::Response::new(http::StatusCode::OK).with_json(&activation).unwrap()} else {reinhardt::Response::new(http::StatusCode::SERVICE_UNAVAILABLE)};
				}
				observed.fetch_add(1, Ordering::AcqRel);
				tokio::time::sleep(std::time::Duration::from_secs(10)).await;
				reinhardt::Response::new(http::StatusCode::OK).with_json(&false).unwrap()
			}
		}));
		Arc::new(Router::new().handler_arc("/", reply.clone()).handler_arc("/{*rest}", reply))
	}.boxed().shared()
}
#[rstest::fixture]
async fn status_scenario(
	#[default(false)] _status_available: bool,
	#[from(pair_future)] pair: PairFuture,
	#[from(count_state)] inspections: Arc<AtomicUsize>,
	#[from(status_router)]
	#[with(_status_available, pair.clone(), inspections.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(upstream_fixtures::async_upstream)]
	#[with(_router.clone())]
	#[future(awt)]
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) -> (
	Pair,
	Arc<AtomicUsize>,
	Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	let state = pair.await;
	let p = state.lock().await.take().unwrap();
	(p, inspections, server)
}
#[rstest::fixture]
fn reconciliation_router(
	#[from(signal_state)] entered: Arc<Notify>,
	#[from(signal_state)] release: Arc<Notify>,
) -> Arc<Router> {
	let reply = Arc::new(upstream_fixtures::any_handler(move |request| {
		let wait = release.clone();
		let reached = entered.clone();
		async move {
			assert!(
				request.uri.path().ends_with("/finalize")
					|| request.uri.path().ends_with("/cancel")
			);
			reached.notify_one();
			wait.notified().await;
			reinhardt::Response::new(http::StatusCode::OK)
				.with_json(&true)
				.unwrap()
		}
	}));
	Arc::new(
		Router::new()
			.handler_arc("/", reply.clone())
			.handler_arc("/{*rest}", reply),
	)
}
#[rstest::fixture]
async fn reconciliation_provider(
	#[from(signal_state)] entered: Arc<Notify>,
	#[from(signal_state)] release: Arc<Notify>,
	#[from(reconciliation_router)]
	#[with(entered.clone(), release.clone())]
	_router: Arc<Router>,
	#[from(upstream_fixtures::upstream)]
	#[with(_router.clone())]
	#[future(awt)]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) -> (
	Arc<Notify>,
	Arc<Notify>,
	Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	(entered, release, Arc::new(server))
}
async fn redirect_peer(f: &Federation, node: &str, endpoint: &str) {
	{
		let query_bind_1 = node;
		let query_bind_2 = endpoint;
		sqlx::query(
			&Query::update()
				.table(Alias::new("peers"))
				.value_expr(
					Alias::new("endpoint"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(node_id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
	}
	.unwrap();
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn remote_status_uses_one_deadline_and_skips_unavailable_dependencies(
	#[case] status_available: bool,
	#[from(status_scenario)]
	#[with(status_available)]
	#[future(awt)]
	status_scenario: (
		Pair,
		Arc<AtomicUsize>,
		Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	),
) {
	let (p, inspections, server) = status_scenario;
	redirect_peer(&p.a, &p.b.config.node_id, &server.url).await;
	let start = tokio::time::Instant::now();
	let response = tokio::time::timeout(
		std::time::Duration::from_secs(3),
		request(
			&p.aa,
			&p.token,
			"GET",
			&format!("/api/tasks/{}/remote-executions", p.task),
			Value::Null,
		),
	)
	.await;
	drop(server);
	let (status, body) =
		response.expect("status and dependency checks must share a bounded deadline");
	assert_eq!(status, 200, "{body}");
	assert_eq!(body[0]["unavailable"], true, "{body}");
	assert_eq!(body[0]["semantic"]["reason"], "unavailable", "{body}");
	if status_available {
		assert!(inspections.load(Ordering::Acquire) > 0);
	} else {
		assert_eq!(
			inspections.load(Ordering::Acquire),
			0,
			"an unavailable peer must not be traversed again"
		);
		assert!(start.elapsed() < std::time::Duration::from_secs(1));
	}
	p.close().await;
}

#[rstest::rstest]
#[case("finalization")]
#[case("cancellation")]
#[tokio::test]
async fn remote_reconciliation_releases_atomic_visibility_before_peer_io(
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
	#[future(awt)] reconciliation_provider: (
		Arc<Notify>,
		Arc<Notify>,
		Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	),
	#[case] kind: &str,
) {
	use reinhardt::query::{LockBehavior, LockType};
	let p = scoped_pair;
	let local = if kind == "finalization" {
		for _ in 0..4 {
			p.step().await;
			if !p.requests.lock().await.is_empty() {
				break;
			}
		}
		assert_eq!(p.requests.lock().await.len(), 1);
		sqlx::query(
			&Query::update()
				.table(Alias::new("generation_remote_dispatches"))
				.value(Alias::new("peer_finalized"), false)
				.and_where(Expr::cust(
					"state='SETTLED' AND usage->>'purpose'='inference'",
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(p.b.store.pool.driver())
		.await
		.unwrap();
		&p.b
	} else {
		let intent = p.generation.as_ref().unwrap().1["intent_id"]
			.as_str()
			.unwrap();
		let (status, body) = request(
			&p.aa,
			&p.token,
			"POST",
			&format!("/api/tasks/{}/remote-generation/{intent}/cancel", p.task),
			json!({}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		{
			let query_bind_1 = intent.parse::<Uuid>().unwrap();
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_remote_intents"))
					.value(Alias::new("cancel_delivered"), false)
					.value_expr(Alias::new("cancel_retry_at"), Expr::cust("NULL"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(p.a.store.pool.driver())
			.await
		}
		.unwrap();
		&p.a
	};
	let remote = if kind == "finalization" { &p.a } else { &p.b };
	let (entered, release, server) = reconciliation_provider;
	redirect_peer(local, &remote.config.node_id, &server.url).await;
	let f = local.clone();
	let reconcile =
		tokio::spawn(async move { aidash_server::generation::provision::reconcile(&f).await });
	tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
		.await
		.unwrap();
	let mut tx = local.store.control_pool.driver().begin().await.unwrap();
	let exclusive: Result<bool, _> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("singleton"))
			.from(Alias::new("atomic_gate"))
			.and_where(Expr::col(Alias::new("singleton")).eq(Expr::value(true)))
			.lock(LockType::Update)
			.lock_behavior(LockBehavior::Nowait)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *tx)
	.await;
	tx.rollback().await.unwrap();
	release.notify_one();
	let result = tokio::time::timeout(std::time::Duration::from_secs(5), reconcile)
		.await
		.unwrap()
		.unwrap();
	drop(server);
	assert!(
		exclusive.is_ok(),
		"peer I/O must not retain the shared atomic gate: {exclusive:?}"
	);
	result.unwrap();
	let pending: i64 = if kind == "finalization" {
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("generation_remote_dispatches"))
				.and_where(Expr::cust(
					"state IN ('ABORTED','SETTLED') AND NOT peer_finalized",
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(local.store.pool.driver())
		.await
		.unwrap()
	} else {
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("generation_remote_intents"))
				.and_where(Expr::cust("cancelled AND NOT cancel_delivered"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(local.store.pool.driver())
		.await
		.unwrap()
	};
	assert_eq!(pending, 0, "the acknowledged durable outbox must be closed");
	p.close().await;
}

#[rstest::rstest]
#[case("expired")]
#[case("disabled")]
#[tokio::test]
async fn generated_home_lineage_is_rechecked_when_semantic_memory_is_disabled(
	#[future(awt)]
	#[with(false, true)]
	scoped_pair: Pair,
	#[case] scenario: &str,
) {
	let p = scoped_pair;
	for _ in 0..4 {
		p.step().await;
		if !p.requests.lock().await.is_empty() {
			break;
		}
	}
	assert_eq!(p.requests.lock().await.len(), 1);
	assert_eq!(p.run().await.phase().as_str(), "TOOL_CALL");
	if scenario == "expired" {
		sqlx::query(
			&Query::update()
				.table(Alias::new("generation_requests"))
				.value_expr(
					Alias::new("expires_at"),
					Expr::cust("CLOCK_TIMESTAMP()-INTERVAL '1 second'"),
				)
				.and_where(Expr::cust("home_node='' AND status='ACTIVE'"))
				.to_string(PostgresQueryBuilder),
		)
		.execute(p.a.store.pool.driver())
		.await
		.unwrap();
	} else {
		let mut spec: Value = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("spec"))
				.from(Alias::new("generation_policies"))
				.and_where(Expr::cust("tenant='acme' AND id='remote-parent'"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.a.store.pool.driver())
		.await
		.unwrap();
		spec["enabled"] = json!(false);
		let (status, body) = request(
			&p.aa,
			&p.a.config.api_token,
			"POST",
			"/api/generation/acme/policies/remote-parent",
			json!({"expected_revision":1,"spec":spec}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
	let still_enabled: bool = sqlx::query_scalar(&Query::select().expr(Expr::cust("COUNT(*)=1"))
		.from(Alias::new("authorization_catalog"))
		.and_where(Expr::cust("tenant='acme' AND entry_id IN (SELECT agent_id FROM generation_requests WHERE home_node='') AND enabled"))
		.to_string(PostgresQueryBuilder)).fetch_one(p.a.store.pool.driver()).await.unwrap();
	assert!(still_enabled, "this cut must precede lifecycle retirement");
	p.step().await;
	let task = p.a.store.task(p.task).await.unwrap();
	let snapshot = p.a.store.snapshot(task.workspace_id).await.unwrap();
	assert!(
		!snapshot
			.messages
			.iter()
			.any(|message| message.content.contains("Scoped remote progress")),
		"a previously produced tool call must not run under expired/disabled Home ancestry"
	);
	let run = p.run().await;
	assert_eq!(run.control.as_str(), "PAUSED", "{run:?}");
	assert_eq!(
		serde_json::to_value(run.recovery.semantic_reason).unwrap(),
		"authority",
		"{run:?}"
	);
	assert_eq!(p.requests.lock().await.len(), 1);
	p.close().await;
}

async fn seed_remote_history(p: &Pair) {
	let mut history = vec![
		json!({"kind":"tool","call":{"id":"first","name":"read","arguments":{}},"result":"keep first"}),
		json!({"kind":"tool","call":{"id":"obsolete","name":"read","arguments":{}},"result":"old".repeat(50000)}),
	];
	for i in 0..6 {
		history.push(json!({"kind":"tool","call":{"id":format!("recent-{i}"),"name":"read","arguments":{}},"result":"recent"}));
	}
	{
		let query_bind_1 = p.admission;
		let mut context = serde_json::to_value(p.run().await.context).unwrap();
		context["history"] = json!(history);
		let query_bind_2 = context;
		let query_bind_3 = common::pending(aidash_server::domain::RunState::Thinking(
			aidash_server::domain::ThinkingState::default(),
		));
		sqlx::query(
			&Query::update()
				.table(Alias::new("runs"))
				.value(Alias::new("phase"), "THINKING")
				.value_expr(
					Alias::new("context"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.value_expr(
					Alias::new("pending"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_3.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(p.b.store.pool.driver())
		.await
	}
	.unwrap();
}

#[rstest::rstest]
#[tokio::test]
async fn remote_compaction_rejects_a_peer_claimed_small_reservation(
	#[future(awt)]
	#[with(true, true, false, true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	seed_remote_history(&p).await;
	p.step().await;
	assert_eq!(p.model.compactions.lock().await.len(), 1);
	let (mut usage, boundary): (Value, Value) = sqlx::query_as(
		&Query::select()
			.columns(["usage", "boundary"].map(Alias::new))
			.from(Alias::new("generation_remote_dispatches"))
			.and_where(Expr::cust("usage->>'purpose'='compaction'"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.b.store.pool.driver())
	.await
	.unwrap();
	let attempt = Uuid::new_v4();
	usage["attempt_id"] = json!(attempt);
	usage["operation_id"] = json!(attempt);
	usage["reserved_tokens"] = json!(1025);
	// An authenticated execution peer controls its own dispatch journal. Home
	// must reject the claimed amount even when that peer's fence agrees with it.
	{
		let query_bind_1 = attempt;
		let query_bind_2 = &usage;
		let query_bind_3 = aidash_server::registry::digest(&usage);
		let query_bind_4 = &p.a.config.node_id;
		let query_bind_5 = &boundary;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("generation_remote_dispatches"))
				.columns(["attempt_id", "usage", "digest", "peer_node", "boundary"].map(Alias::new))
				.from_subquery(
					Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_5.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	{
		let query_bind_1 = p.admission;
		let query_bind_2 = boundary["step"].as_i64().unwrap() as i32;
		sqlx::query(
			&Query::update()
				.table(Alias::new("runs"))
				.value_expr(
					Alias::new("step"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	let before: i64 = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("used_tokens"))
			.from(Alias::new("generation_budgets"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.a.store.pool.driver())
	.await
	.unwrap();
	let response =
		p.a.client
			.clone()
			.post(format!(
				"{}/federation/v0.1/scoped/usage/reserve",
				p.a.config.endpoint
			))
			.bearer_auth(std::env::var("AIDASH_SECRET_TEST_PEER").unwrap())
			.header("x-aidash-node", &p.b.config.node_id)
			.header("x-aidash-protocol", "0.2")
			.json(&json!({"usage":usage,"boundary":boundary}))
			.send()
			.await
			.unwrap();
	let status = response.status();
	let body = response.text().await.unwrap();
	assert_eq!(status, 403, "{body}");
	let after: i64 = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("used_tokens"))
			.from(Alias::new("generation_budgets"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.a.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(after, before);
	let count: i64 = {
		let query_bind_1 = attempt;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("generation_remote_usage"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(attempt_id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.a.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(count, 0, "a forged amount must not debit the Home ancestor");
	p.close().await;
}

#[rstest::rstest]
#[case("approved", 0, None)]
#[case("unnecessary", 0, None)]
#[case("ancestor_limit", 0, Some("allowance"))]
#[case("catalog_revoked", 0, Some("authority"))]
#[case("outage", 503, Some("unavailable"))]
#[case("bad_answers", 1, Some("provider_contract"))]
#[case("credential_rejected", 401, Some("configuration"))]
#[tokio::test]
async fn remote_compaction_uses_exact_approval_and_origin_owned_allowances(
	#[future(awt)]
	#[with(true, true, false, true)]
	scoped_pair: Pair,
	#[case] scenario: &str,
	#[case] status: usize,
	#[case] reason: Option<&str>,
) {
	let p = scoped_pair;
	if scenario != "unnecessary" {
		seed_remote_history(&p).await;
	}
	p.model.compaction_status.store(status, Ordering::Release);
	*p.model.compaction_reservations.lock().await = Some(ReservationCheck {
		pools: vec![
			p.a.store.pool.driver().clone(),
			p.b.store.pool.driver().clone(),
		],
		dispatcher: p.b.store.pool.driver().clone(),
		grant: p.grant,
		admission: p.admission,
		purpose: "compaction",
	});
	if scenario == "ancestor_limit" {
		sqlx::query(
			&Query::update()
				.table(Alias::new("generation_budgets"))
				.value(Alias::new("compaction_call_limit"), 0)
				.to_string(PostgresQueryBuilder),
		)
		.execute(p.a.store.pool.driver())
		.await
		.unwrap();
	}
	if scenario == "catalog_revoked" {
		assert_eq!(request(&p.ba,&p.b.config.api_token,"POST","/api/authorization/acme/catalog",json!({"entry":{"id":"remote-compactor","version":"1.0.0"},"expected_revision":1,"enabled":false})).await.0,200);
	}
	for _ in 0..4 {
		p.step().await;
		let run = p.run().await;
		if !p.requests.lock().await.is_empty()
			|| run.control.as_str() == "PAUSED"
			|| run.recovery.retry.is_some()
		{
			break;
		}
	}
	let run = p.run().await;
	if let Some(reason) = reason {
		assert_eq!(
			serde_json::to_value(run.recovery.semantic_reason).unwrap(),
			reason,
			"{run:?}"
		);
		assert_eq!(
			run.control.as_str(),
			if scenario == "outage" {
				"ACTIVE"
			} else {
				"PAUSED"
			}
		);
		assert_ne!(run.phase().as_str(), "FAILED");
		assert!(p.requests.lock().await.is_empty());
		assert!(
			!run.error
				.as_deref()
				.unwrap_or_default()
				.contains("private compactor response")
		);
	} else {
		assert_eq!(p.requests.lock().await.len(), 1, "{run:?}");
	}
	let count = usize::from(matches!(
		scenario,
		"approved" | "outage" | "bad_answers" | "credential_rejected"
	));
	let calls = p.model.compactions.lock().await;
	assert_eq!(calls.len(), count);
	if let Some(call) = calls.first() {
		assert_eq!(call["model"], "fixture-jev");
		assert!(
			call.to_string().contains("ochre falcon"),
			"approved context must reach the exact compactor"
		);
	}
	drop(calls);
	for node in [&p.a, &p.b] {
		let (used, limit): (i64, i64) = sqlx::query_as(
			&Query::select()
				.columns(["compaction_calls", "compaction_call_limit"].map(Alias::new))
				.from(Alias::new("generation_budgets"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(node.store.pool.driver())
		.await
		.unwrap();
		assert_eq!(
			used, count as i64,
			"{}: {used}/{limit}",
			node.config.node_id
		);
		if count > 0 {
			let amount: i64 = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("reserved_tokens"))
					.from(Alias::new("generation_remote_usage"))
					.and_where(Expr::cust("purpose='compaction'"))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(node.store.pool.driver())
			.await
			.unwrap();
			assert_eq!(
				amount, 401024,
				"every owner must reserve the approved request maximum"
			);
		}
	}
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn remote_compaction_without_explicit_recipient_pauses_without_environment_fallback(
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	seed_remote_history(&p).await;
	p.step().await;
	let run = p.run().await;
	assert_eq!(run.control.as_str(), "PAUSED", "{run:?}");
	assert_eq!(
		serde_json::to_value(run.recovery.semantic_reason).unwrap(),
		"context_budget"
	);
	assert!(p.model.compactions.lock().await.is_empty());
	assert!(p.requests.lock().await.is_empty());
	p.close().await;
}

struct ScopedWorkerCommand {
	command: std::process::Command,
	_settings_directory: tempfile::TempDir,
	_binary: ScopedWorkerBinary,
}
struct ScopedWorkerBinary {
	path: std::path::PathBuf,
	_directory: tempfile::TempDir,
}
#[rstest::fixture]
fn scoped_worker_binary(
	#[from(reinhardt::test::fixtures::temp_dir)] directory: tempfile::TempDir,
) -> ScopedWorkerBinary {
	// Own the exact local executable until the child is reaped; preserve the macOS snapshot.
	#[cfg(target_os = "macos")]
	let path = {
		let snapshot = directory.path().join("aidash");
		std::fs::copy(env!("CARGO_BIN_EXE_aidash"), &snapshot)
			.expect("snapshot the exact scoped worker executable");
		snapshot
	};
	#[cfg(not(target_os = "macos"))]
	let path = std::path::PathBuf::from(env!("CARGO_BIN_EXE_aidash"));
	ScopedWorkerBinary {
		path,
		_directory: directory,
	}
}
#[rstest::fixture]
fn scoped_worker_command(
	#[from(pair_future)] pair: PairFuture,
	#[from(reinhardt::test::fixtures::temp_dir)] directory: tempfile::TempDir,
	scoped_worker_binary: ScopedWorkerBinary,
) -> BoxFuture<'static, ScopedWorkerCommand> {
	async move {
		let state = pair.await;
		let pair = state.lock().await;
		let p = pair.as_ref().unwrap();
		let mut database = reqwest::Url::parse(&p.bu).unwrap();
		database
			.query_pairs_mut()
			.append_pair("options", &format!("-c application_name={}", p.bschema));
		let mut command = std::process::Command::new(&scoped_worker_binary.path);
		command
			// The fixture has already migrated both databases. Starting only the
			// worker keeps crash recovery independent of another migration pass.
			.args(common::native_process_args(&p.b, "runworker"))
			.envs(common::native_process_environment(
				&p.b,
				database.as_str(),
				directory.path(),
				1,
			))
			.env("AIDASH_ACTIVATION_NAMESPACE", &p.bschema)
			.env("AIDASH_ENV", "test")
			.env("RUST_LOG", "aidash_server=info")
			.stdout(std::process::Stdio::inherit())
			.stderr(std::process::Stdio::inherit());
		ScopedWorkerCommand {
			command,
			_settings_directory: directory,
			_binary: scoped_worker_binary,
		}
	}
	.boxed()
}
struct ScopedWorkerProcess {
	child: std::process::Child,
	_command: ScopedWorkerCommand,
}
impl ScopedWorkerProcess {
	// Process primitive used by the initial fixture and by the explicit restart Act.
	fn spawn(mut command: ScopedWorkerCommand) -> Self {
		let child = command.command.spawn().unwrap();
		Self {
			child,
			_command: command,
		}
	}
}
#[rstest::fixture]
async fn running_scoped_pair(
	#[from(pair_future)]
	#[with(true)]
	pair: PairFuture,
	#[from(scoped_worker_command)]
	#[with(pair.clone())]
	#[future(awt)]
	initial: ScopedWorkerCommand,
	#[from(scoped_worker_command)]
	#[with(pair.clone())]
	#[future(awt)]
	restart: ScopedWorkerCommand,
) -> (Pair, ScopedWorkerProcess, ScopedWorkerCommand) {
	let state = pair.await;
	let p = state.lock().await.take().unwrap();
	p.model.hold.store(true, Ordering::Release);
	(p, ScopedWorkerProcess::spawn(initial), restart)
}
impl Drop for ScopedWorkerProcess {
	fn drop(&mut self) {
		let _ = self.child.kill();
		let _ = self.child.wait();
	}
}

#[rstest::rstest]
#[tokio::test]
async fn process_sigkill_preserves_remote_receipt_and_uncertain_origin_charges(
	#[future(awt)] running_scoped_pair: (Pair, ScopedWorkerProcess, ScopedWorkerCommand),
) {
	let (p, mut worker, restart) = running_scoped_pair;
	let arrived = tokio::time::timeout(
		std::time::Duration::from_secs(60),
		p.model.entered.notified(),
	)
	.await;
	assert!(
		arrived.is_ok(),
		"worker={:?}, run={:?}",
		worker.child.try_wait(),
		p.run().await
	);
	drop(worker); // Actual SIGKILL. No worker cleanup or settlement runs.
	let old_attempt: Uuid = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("attempt_id"))
			.from(Alias::new("generation_remote_dispatches"))
			.and_where(Expr::cust(
				"state='DISPATCHED' AND usage->>'purpose'='inference'",
			))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.b.store.pool.driver())
	.await
	.unwrap();
	let before = p.run().await;
	let before_receipts: i64 = {
		let query_bind_1 = p.admission;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("semantic_remote_receipts"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(run_id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(
		before_receipts, 1,
		"receipt must have survived before inference HTTP"
	);
	for node in [&p.a, &p.b] {
		let state: String = {
			let query_bind_1 = old_attempt;
			sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("state"))
					.from(Alias::new("generation_remote_usage"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(attempt_id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(node.store.pool.driver())
			.await
		}
		.unwrap();
		assert_eq!(state, "RESERVED");
	}
	p.model.hold.store(false, Ordering::Release);
	p.model.release.notify_one();
	// Advance only the dead process's lease, retaining the original durable
	// attempt and fencing. Recovery otherwise waits the production lease delay.
	{
		let query_bind_1 = p.admission;
		sqlx::query(
			&Query::update()
				.table(Alias::new("runs"))
				.value_expr(
					Alias::new("lease_until"),
					Expr::cust("CLOCK_TIMESTAMP()-INTERVAL '1 second'"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	// Act: replace the killed worker after preserving its durable receipt and advancing its lease.
	let mut worker = ScopedWorkerProcess::spawn(restart);
	let recovered = tokio::time::timeout(std::time::Duration::from_secs(60), async {
		loop {
			let run = p.run().await;
			if run.phase().as_str() == "COMPLETED" {
				break;
			}
			assert_ne!(run.control.as_str(), "PAUSED", "{run:?}");
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
		}
	})
	.await;
	let state = p.run().await;
	assert!(
		recovered.is_ok(),
		"same remote Run must recover under its original admission: worker={:?}, phase={}, step={}, revision={}, error={:?}, retry={:?}",
		worker.child.try_wait(),
		state.phase().as_str(),
		state.step,
		state.revision,
		state.error,
		state.recovery.retry
	);
	drop(worker);
	let after = p.run().await;
	assert_eq!(after.id, before.id);
	assert_eq!(after.task_id, before.task_id);
	assert_eq!(after.agent_id, before.agent_id);
	assert_eq!(p.requests.lock().await.len(), 2);
	assert_eq!(
		p.semantic.as_ref().unwrap().requests.lock().await.len(),
		5,
		"persisted receipt replay must not embed the same boundary again"
	);
	for node in [&p.a, &p.b] {
		let usages: Vec<(Uuid, String, i64, Option<i64>)> = sqlx::query_as(
			&Query::select()
				.columns(
					["attempt_id", "state", "reserved_tokens", "reported_tokens"].map(Alias::new),
				)
				.from(Alias::new("generation_remote_usage"))
				.and_where(Expr::cust("purpose='inference'"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(node.store.pool.driver())
		.await
		.unwrap();
		assert_eq!(usages.len(), 2);
		let old = usages.iter().find(|usage| usage.0 == old_attempt).unwrap();
		assert_eq!(old.1, "RESERVED");
		assert_eq!(old.3, None);
		let new = usages.iter().find(|usage| usage.0 != old_attempt).unwrap();
		assert_eq!(new.1, "SETTLED");
		assert_eq!(new.3, Some(2));
		let used: i64 = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("used_tokens"))
				.from(Alias::new("generation_budgets"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(node.store.pool.driver())
		.await
		.unwrap();
		assert_eq!(
			used,
			old.2 + 3,
			"uncertain input remains charged at every origin owner"
		);
	}
	let state: String = {
		let query_bind_1 = old_attempt;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("state"))
				.from(Alias::new("generation_remote_dispatches"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(attempt_id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(p.b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(state, "DISPATCHED");
	p.close().await;
}

#[rstest::rstest]
#[case("missing_usage", false)]
#[case("malformed_usage", false)]
#[case("overreported_usage", true)]
#[case("wrong_model", true)]
#[case("wrong_dimensions", true)]
#[case("oversized_response", true)]
#[tokio::test]
async fn generated_remote_embedding_validates_contract_and_retains_uncertain_charge(
	#[case] fault: &str,
	#[case] rejected: bool,
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	let mut response = json!({"model":"home-vector","data":[{"index":0,"embedding":[1.0,0.0,0.0]}],"usage":{"prompt_tokens":1,"total_tokens":1}});
	match fault {
		"missing_usage" => {
			response.as_object_mut().unwrap().remove("usage");
		}
		"malformed_usage" => response["usage"] = json!({"prompt_tokens":-1,"total_tokens":-1}),
		"overreported_usage" => {
			response["usage"] = json!({"prompt_tokens":99999999,"total_tokens":99999999})
		}
		"wrong_model" => response["model"] = json!("unapproved-model"),
		"wrong_dimensions" => response["data"][0]["embedding"] = json!([1.0]),
		"oversized_response" => response["padding"] = json!("x".repeat(1_048_577)),
		_ => unreachable!(),
	}
	*p.semantic.as_ref().unwrap().response.lock().await = Some(response);
	p.step().await;
	p.step().await;
	let run = p.run().await;
	if rejected {
		assert_eq!(run.control.as_str(), "PAUSED", "{fault}: {run:?}");
		assert_eq!(
			serde_json::to_value(run.recovery.semantic_reason).unwrap(),
			"provider_contract",
			"{fault}: {run:?}"
		);
		assert!(p.requests.lock().await.is_empty());
	} else {
		assert_eq!(run.control.as_str(), "ACTIVE", "{fault}: {run:?}");
		assert_eq!(p.requests.lock().await.len(), 1);
	}
	let calls = p.semantic.as_ref().unwrap().requests.lock().await;
	assert_eq!(calls.len(), 5);
	let reserved = calls.last().unwrap()["input"].as_str().unwrap().len() as i64 + 1024;
	drop(calls);
	for node in [&p.a, &p.b] {
		let budget: (i64, i64) = sqlx::query_as(
			&Query::select()
				.columns(["used_tokens", "embedding_calls"].map(Alias::new))
				.from(Alias::new("generation_budgets"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(node.store.pool.driver())
		.await
		.unwrap();
		assert_eq!(
			budget,
			(reserved + if rejected { 0 } else { 2 }, 1),
			"{fault} {}",
			node.config.node_id
		);
	}
	p.close().await;
}

#[rstest::rstest]
#[case("receiver_calls")]
#[case("home_tokens")]
#[case("home_catalog")]
#[case("receiver_expiry")]
#[tokio::test]
async fn generated_remote_prerequisites_stop_before_embedding_dispatch(
	#[case] fault: &str,
	#[future(awt)]
	#[with(true, true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	match fault {
		"receiver_calls" | "home_tokens" => {
			let (node, column) = if fault == "receiver_calls" {
				(&p.b, "embedding_call_limit")
			} else {
				(&p.a, "token_limit")
			};
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_budgets"))
					.value(
						Alias::new(column),
						if fault == "home_tokens" { 1 } else { 0 },
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(node.store.pool.driver())
			.await
			.unwrap();
		}
		"home_catalog" => {
			assert_eq!(request(&p.aa, &p.a.config.api_token, "POST", "/api/authorization/acme/catalog", json!({"entry":{"id":"home-embedding","version":"1.0.0"},"expected_revision":1,"enabled":false})).await.0, 200);
		}
		"receiver_expiry" => {
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_requests"))
					.value_expr(
						Alias::new("expires_at"),
						Expr::cust("CLOCK_TIMESTAMP()-INTERVAL '1 second'"),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(p.b.store.pool.driver())
			.await
			.unwrap();
		}
		_ => unreachable!(),
	}
	for _ in 0..3 {
		p.step().await;
		if p.run().await.control.as_str() == "PAUSED" {
			break;
		}
	}
	let run = p.run().await;
	assert_eq!(run.control.as_str(), "PAUSED", "{fault}: {run:?}");
	assert_ne!(run.phase().as_str(), "FAILED");
	assert!(p.requests.lock().await.is_empty());
	assert_eq!(
		p.semantic.as_ref().unwrap().requests.lock().await.len(),
		4,
		"no query embedding may dispatch"
	);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn remote_memory_mutate_is_not_advertised_and_cannot_write_a_receiver_substitute(
	#[future(awt)]
	#[with(true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	p.model.force_memory_mutate.store(true, Ordering::Release);
	p.step().await;
	p.step().await;
	p.step().await;
	assert_eq!(p.requests.lock().await.len(), 1);
	let run = p.run().await;
	assert_eq!(
		serde_json::to_value(&run.context).unwrap()["history"][0]["result"]["error"],
		"unavailable tool memory_mutate",
		"unsupported remote writes must return an explicit tool error"
	);
	for node in [&p.a, &p.b] {
		let count: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("memory_units"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(node.store.pool.driver())
		.await
		.unwrap();
		assert_eq!(
			count, 0,
			"remote writes cannot use either node's local memory"
		);
	}
	p.close().await;
}

impl Drop for Pair {
	fn drop(&mut self) {
		// Preserve task ownership if a contract assertion fails or is cancelled.
		for server in &self.servers {
			server.abort();
		}
	}
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _, SimpleExpr};

#[rstest::fixture]
fn values_state() -> Arc<Mutex<Vec<Value>>> {
	Arc::new(Mutex::new(Vec::new()))
}
#[rstest::fixture]
fn bool_state() -> Arc<AtomicBool> {
	Arc::new(AtomicBool::new(false))
}
#[rstest::fixture]
fn signal_state() -> Arc<Notify> {
	Arc::new(Notify::new())
}
#[rstest::fixture]
fn count_state() -> Arc<AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}
#[rstest::fixture]
fn reservation_state() -> Arc<Mutex<Option<ReservationCheck>>> {
	Arc::new(Mutex::new(None))
}
#[rstest::fixture]
fn reply_state() -> Arc<Mutex<Option<String>>> {
	Arc::new(Mutex::new(None))
}
#[rstest::fixture]
fn response_state() -> Arc<Mutex<Option<Value>>> {
	Arc::new(Mutex::new(None))
}
#[rstest::fixture]
fn recovery_directory(
	#[from(reinhardt::test::fixtures::temp_dir)] directory: tempfile::TempDir,
) -> Arc<tempfile::TempDir> {
	Arc::new(directory)
}
#[rstest::fixture]
fn model_script(
	#[from(values_state)] requests: Arc<Mutex<Vec<Value>>>,
	#[from(bool_state)] hold: Arc<AtomicBool>,
	#[from(signal_state)] entered: Arc<Notify>,
	#[from(signal_state)] release: Arc<Notify>,
	#[from(reservation_state)] reservations: Arc<Mutex<Option<ReservationCheck>>>,
	#[from(compaction_model_state)] _state: CompactionModelState,
	#[from(bool_state)] force_memory_mutate: Arc<AtomicBool>,
) -> ModelScript {
	let CompactionModelState {
		compactions,
		compaction_status,
		compaction_reservations,
	} = _state;
	ModelScript {
		requests,
		hold,
		entered,
		release,
		reservations,
		compactions,
		compaction_status,
		compaction_reservations,
		force_memory_mutate,
	}
}
#[rstest::fixture]
fn model_router(model_script: ModelScript) -> Arc<Router> {
	let model_app=reinhardt::test::stub::StubRouter::new()
.route("/v1/chat/completions", http::Method::POST, reply({let script=model_script.clone();move |request:reinhardt::Request| {let script=script.clone();let input=request.json::<Value>().unwrap();async move {
        let check=script.reservations.lock().await.clone();
        if let Some(check)=check {check.before_http().await;}
        let mut calls=script.requests.lock().await;
        calls.push(input);
        let count=calls.len(); drop(calls); script.entered.notify_one(); if script.hold.load(Ordering::Acquire) {script.release.notified().await;} let message=if count==1 && script.force_memory_mutate.load(Ordering::Acquire) {json!({"role":"assistant","content":null,"tool_calls":[{"id":"forbidden-write","type":"function","function":{"name":"memory_mutate","arguments":"{\"data\":{\"forbidden\":true}}"}}]})} else if count==1 {json!({"role":"assistant","content":null,"tool_calls":[{"id":"note","type":"function","function":{"name":"workspace_message","arguments":"{\"content\":\"Scoped remote progress\"}"}}]})} else {json!({"role":"assistant","content":"Scoped remote result"})};
        reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
    }}}))
.route("/systemone", http::Method::POST, reply({let script=model_script.clone();move |request:reinhardt::Request| {let script=script.clone();let input=request.json::<Value>().unwrap();async move {
		if let Some(check)=script.compaction_reservations.lock().await.clone() {check.before_http().await;}
		script.compactions.lock().await.push(input.clone());
		match script.compaction_status.load(Ordering::Acquire) {
			503 => reinhardt::Response::new(http::StatusCode::SERVICE_UNAVAILABLE).with_body("private compactor response").with_header("Content-Type", "text/plain; charset=utf-8"),
			401 => reinhardt::Response::new(http::StatusCode::UNAUTHORIZED).with_body("private compactor response").with_header("Content-Type", "text/plain; charset=utf-8"),
			1 => reinhardt::Response::ok().with_json(&json!({"answers":{}})).unwrap(),
			_ => {
				let answers:serde_json::Map<_,_>=input["questions"].as_object().unwrap().keys().map(|name|(name.clone(),json!({"noul":0.0}))).collect();
				reinhardt::Response::ok().with_json(&json!({"answers":answers})).unwrap()
			}
		}
	}}})).into_server_router();
	Arc::new(model_app)
}
#[rstest::fixture]
fn scoped_runtime(
	#[default(false)] native: bool,
	#[default("aidash://execution-test")] node: &str,
	#[from(upstream_fixtures::fixed_listener)] listener: upstream_fixtures::ListenerFuture,
	#[from(recovery_directory)] directory: Arc<tempfile::TempDir>,
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> common::RuntimeFuture {
	let node = node.to_owned();
	async move {
		let mut owner = runtime.await;
		let f = &mut owner.federation;
		f.config.node_id = node.clone();
		f.store.node_id = node.clone();
		f.config.endpoint = format!("http://{}", listener.await.local_addr().unwrap());
		if native {
			f.store = aidash_server::semantic::services::memory_recovery::initialize(
				&f.store,
				directory.path().to_owned(),
			)
			.await
			.unwrap();
		}
		f.registry = aidash_server::registry::Registry::new(f.store.pool.clone(), &node).unwrap();
		owner
	}
	.boxed()
	.shared()
}

#[derive(Clone)]
struct EmbeddingProvider {
	requests: Arc<Mutex<Vec<Value>>>,
	failing: Arc<AtomicBool>,
	reservations: Arc<Mutex<Option<ReservationCheck>>>,
	response: Arc<Mutex<Option<Value>>>,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
}
#[rstest::fixture]
fn embedding_router(
	#[from(values_state)] requests: Arc<Mutex<Vec<Value>>>,
	#[from(bool_state)] failing: Arc<AtomicBool>,
	#[from(reservation_state)] reservations: Arc<Mutex<Option<ReservationCheck>>>,
	#[from(response_state)] response: Arc<Mutex<Option<Value>>>,
) -> Arc<Router> {
	let embedding_app = reinhardt::test::stub::StubRouter::new()
.route("/v1/embeddings", http::Method::POST, reply(move |request:reinhardt::Request| {let requests=requests.clone();let failing=failing.clone();let reservations=reservations.clone();let response=response.clone();let headers=request.headers.clone();let body=request.json::<Value>().unwrap();async move {
		assert!(!headers.contains_key(http::header::AUTHORIZATION), "an unsigned provider must not receive the subject or peer bearer");
        let check=reservations.lock().await.clone();
        if let Some(check)=check {check.before_http().await;}
		requests.lock().await.push(body.clone());
        if failing.load(Ordering::Acquire) {
            return reinhardt::Response::new(http::StatusCode::SERVICE_UNAVAILABLE).with_json(&json!({"error":"fixture outage"})).unwrap();
        }
		if let Some(value) = response.lock().await.clone() { return reinhardt::Response::ok().with_json(&value).unwrap(); }
        reinhardt::Response::ok().with_json(&json!({"model":body["model"],"data":[{"index":0,"embedding":[1.0,0.0,0.0]}],"usage":{"prompt_tokens":1,"total_tokens":1}})).unwrap()
	}})).into_server_router();
	Arc::new(embedding_app)
}
#[rstest::fixture]
fn embedding_provider(
	#[from(values_state)] requests: Arc<Mutex<Vec<Value>>>,
	#[from(bool_state)] failing: Arc<AtomicBool>,
	#[from(reservation_state)] reservations: Arc<Mutex<Option<ReservationCheck>>>,
	#[from(response_state)] response: Arc<Mutex<Option<Value>>>,
	#[from(embedding_router)]
	#[with(requests.clone(),failing.clone(),reservations.clone(),response.clone())]
	_router: Arc<Router>,
	#[from(upstream_fixtures::ready_router)]
	#[with(_router.clone())]
	_ready: upstream_fixtures::RouterFuture,
	#[from(upstream_fixtures::async_upstream)]
	#[with(_ready.clone())]
	server: upstream_fixtures::UpstreamFuture,
) -> BoxFuture<'static, EmbeddingProvider> {
	async move {
		EmbeddingProvider {
			requests,
			failing,
			reservations,
			response,
			server: server.await,
		}
	}
	.boxed()
}

#[derive(Clone)]
struct CompactionModelState {
	compactions: Arc<Mutex<Vec<Value>>>,
	compaction_status: Arc<AtomicUsize>,
	compaction_reservations: Arc<Mutex<Option<ReservationCheck>>>,
}
#[rstest::fixture]
fn compaction_model_state(
	#[from(values_state)] compactions: Arc<Mutex<Vec<Value>>>,
	#[from(count_state)] compaction_status: Arc<AtomicUsize>,
	#[from(reservation_state)] compaction_reservations: Arc<Mutex<Option<ReservationCheck>>>,
) -> CompactionModelState {
	CompactionModelState {
		compactions,
		compaction_status,
		compaction_reservations,
	}
}

#[derive(Clone)]
struct ScopedModel {
	state: ModelScript,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
}
#[rstest::fixture]
fn scoped_model(
	model_script: ModelScript,
	#[from(model_router)]
	#[with(model_script.clone())]
	router: Arc<Router>,
	#[from(upstream_fixtures::ready_router)]
	#[with(router.clone())]
	ready: upstream_fixtures::RouterFuture,
	#[from(upstream_fixtures::async_upstream)]
	#[with(ready.clone())]
	server: upstream_fixtures::UpstreamFuture,
) -> BoxFuture<'static, ScopedModel> {
	async move {
		let _ = (router, ready);
		ScopedModel {
			state: model_script,
			server: server.await,
		}
	}
	.boxed()
}
#[derive(Clone)]
struct ScopedApplication {
	application: common::ApplicationFixture,
	directory: Arc<tempfile::TempDir>,
	listener: upstream_fixtures::ListenerFuture,
}
type ScopedApplicationFuture = futures_util::future::Shared<BoxFuture<'static, ScopedApplication>>;
#[rstest::fixture]
fn scoped_application(
	#[default(false)] _native: bool,
	#[default("aidash://execution-test")] _node: &str,
	#[from(recovery_directory)] directory: Arc<tempfile::TempDir>,
	#[from(upstream_fixtures::fixed_listener)] listener: upstream_fixtures::ListenerFuture,
	#[from(scoped_runtime)]
	#[with(_native,_node,listener.clone(),directory.clone())]
	runtime: common::RuntimeFuture,
	#[from(common::native_application)]
	#[with(Default::default(),aidash_server::sse::Service::new(Default::default()),Arc::new(|r|r),runtime.clone())]
	application: common::ApplicationFuture,
) -> ScopedApplicationFuture {
	async move {
		drop(runtime);
		ScopedApplication {
			application: application.await,
			directory,
			listener,
		}
	}
	.boxed()
	.shared()
}
struct SourceReplyMiddleware(Arc<Mutex<Option<String>>>);
#[async_trait::async_trait]
impl reinhardt::Middleware for SourceReplyMiddleware {
	async fn process(
		&self,
		request: reinhardt::Request,
		next: Arc<dyn reinhardt::Handler>,
	) -> reinhardt::Result<reinhardt::Response> {
		let semantic = request.uri.path().ends_with("/scoped/semantic/query");
		if !semantic && !request.uri.path().ends_with("/scoped/execution/commands") {
			return next.handle(request).await;
		}
		let input: Value = request.json().unwrap();
		let mut wanted = self.0.lock().await;
		let lose = wanted.as_deref().is_some_and(|v| {
			if semantic {
				v == "semantic.query"
			} else {
				input["operation"] == v
			}
		});
		if lose {
			wanted.take();
		}
		drop(wanted);
		let response = next.handle(request).await?;
		if lose && response.status.is_success() {
			Ok(
				reinhardt::Response::new(http::StatusCode::SERVICE_UNAVAILABLE)
					.with_json(&json!({"error":"fixture lost a committed reply"}))
					.unwrap(),
			)
		} else {
			Ok(response)
		}
	}
}
#[rstest::fixture]
fn scoped_router(
	scoped_application: ScopedApplicationFuture,
	#[default(None)] reply: Option<Arc<Mutex<Option<String>>>>,
) -> upstream_fixtures::RouterFuture {
	async move {
		let owner = scoped_application.await;
		// reinhardt-web#6673: apply fault middleware to production routes directly, preserving HEAD.
		let router = aidash_server::routes()
			.into_server()
			.with_di_context(owner.application.application.context.clone());
		Arc::new(if let Some(reply) = reply {
			router.with_middleware(SourceReplyMiddleware(reply))
		} else {
			router
		})
	}
	.boxed()
	.shared()
}
#[rstest::fixture]
fn scoped_listener(
	scoped_application: ScopedApplicationFuture,
) -> upstream_fixtures::ListenerFuture {
	async move { scoped_application.await.listener.await }
		.boxed()
		.shared()
}
struct ScopedPeer {
	application: ScopedApplication,
	server: upstream_fixtures::FixedServerGuard,
}
#[rstest::fixture]
fn scoped_peer(
	#[default(false)] _native: bool,
	#[default("aidash://execution-test")] _node: &str,
	#[default(None)] _reply: Option<Arc<Mutex<Option<String>>>>,
	#[from(scoped_application)]
	#[with(_native, _node)]
	application: ScopedApplicationFuture,
	#[from(scoped_router)]
	#[with(application.clone(),_reply.clone())]
	router: upstream_fixtures::RouterFuture,
	#[from(scoped_listener)]
	#[with(application.clone())]
	listener: upstream_fixtures::ListenerFuture,
	#[from(upstream_fixtures::fixed_upstream)]
	#[with(None,listener.clone(),router.clone())]
	server: BoxFuture<'static, upstream_fixtures::FixedServerGuard>,
) -> BoxFuture<'static, ScopedPeer> {
	async move {
		let _ = (router, listener);
		ScopedPeer {
			application: application.await,
			server: server.await,
		}
	}
	.boxed()
}
struct ScopedInfrastructure {
	model_state: ModelScript,
	model_server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	drop_reply: Arc<Mutex<Option<String>>>,
	source: ScopedPeer,
	receiver: ScopedPeer,
	embedding_provider: BoxFuture<'static, EmbeddingProvider>,
}
#[rstest::fixture]
fn scoped_infrastructure(
	#[default(false)] _native: bool,
	scoped_model: BoxFuture<'static, ScopedModel>,
	#[from(reply_state)] drop_reply: Arc<Mutex<Option<String>>>,
	#[from(scoped_peer)]
	#[with(_native,"aidash://execution-test",Some(drop_reply.clone()))]
	source: BoxFuture<'static, ScopedPeer>,
	#[from(scoped_peer)]
	#[with(_native, "aidash://scoped-receiver")]
	receiver: BoxFuture<'static, ScopedPeer>,
	embedding_provider: BoxFuture<'static, EmbeddingProvider>,
) -> BoxFuture<'static, ScopedInfrastructure> {
	async move {
		let model = scoped_model.await;
		ScopedInfrastructure {
			model_state: model.state,
			model_server: model.server,
			drop_reply,
			source: source.await,
			receiver: receiver.await,
			embedding_provider,
		}
	}
	.boxed()
}

#[rstest::rstest]
#[tokio::test]
async fn remote_human_continuations_survive_retries_and_restart_without_a_shadow_run(
	#[future(awt)] scoped_pair: Pair,
	#[from(signal_state)] source_notify: Arc<Notify>,
	#[from(signal_state)] receiver_notify: Arc<Notify>,
) {
	let mut p = scoped_pair;
	p.step().await;
	let home = Home::new(p.b.clone(), p.run().await);
	let key = format!("{}:binding-human", p.admission);
	*p.drop_reply.lock().await = Some("human_request".into());
	assert!(
		home.human_request("QUESTION", "Proceed with the saved task?", &key)
			.await
			.is_err()
	);
	let saved = home
		.human_request("QUESTION", "Proceed with the saved task?", &key)
		.await
		.unwrap();
	assert_eq!(saved.run_id, p.admission);
	assert_eq!(
		saved.workspace_id,
		p.a.store.task(p.task).await.unwrap().workspace_id
	);
	assert_eq!(
		home.human_request("QUESTION", "Proceed with the saved task?", &key)
			.await
			.unwrap()
			.id,
		saved.id
	);
	assert!(
		home.human_request("QUESTION", "Changed input", &key)
			.await
			.is_err()
	);
	assert!(
		p.a.store.run(p.admission).await.is_err(),
		"Home must not create a shadow execution Run"
	);
	// The model guard lives in providers; both peer transports must stop before rebinding.
	for server in p.servers.drain(..) {
		server.shutdown().await;
	}
	reconnect(&mut p.a, source_notify).await;
	reconnect(&mut p.b, receiver_notify).await;
	// Act: rebuild peers over their durable journals after stopping the transports.
	p.aa = common::application(p.a.clone()).await;
	p.ba = common::application(p.b.clone()).await;
	for (f, application) in [(&p.a, p.aa.clone()), (&p.b, p.ba.clone())] {
		let router = aidash_server::routes()
			.into_server()
			.with_di_context(application.context.clone())
			.with_middleware(SourceReplyMiddleware(p.drop_reply.clone()));
		let endpoint = reqwest::Url::parse(&f.config.endpoint).unwrap();
		let listener = tokio::net::TcpListener::bind(("127.0.0.1", endpoint.port().unwrap()))
			.await
			.unwrap();
		// Act: restart the production transport at its original address.
		p.servers.push(upstream_fixtures::FixedServerGuard::spawn(
			Arc::new(listener),
			Arc::new(router),
			Some(application.context.clone()),
		));
	}
	let home = Home::new(p.b.clone(), p.run().await);
	assert_eq!(
		home.human_request_by_id(saved.id).await.unwrap().id,
		saved.id
	);
	let projection = request(
		&p.aa,
		&p.token,
		"GET",
		&format!("/api/tasks/{}/remote-executions", p.task),
		Value::Null,
	)
	.await;
	assert_eq!(projection.0, 200, "{}", projection.1);
	assert_eq!(projection.1[0]["human_requests"][0]["id"], json!(saved.id));
	let events: Vec<Value> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("data"))
			.from(Alias::new("events"))
			.and_where(Expr::col(Alias::new("kind")).eq("task.remote_human_requested"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(p.a.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(
		events.len(),
		1,
		"idempotent retries must not duplicate notifications"
	);
	assert_eq!(events[0]["task_id"], json!(p.task));
	assert_eq!(events[0]["request_id"], json!(saved.id));
	let answer_path = format!(
		"/api/tasks/{}/remote-grants/{}/human-requests/answer",
		p.task, p.grant
	);
	let (status, answered) = request(
		&p.aa,
		&p.token,
		"POST",
		&answer_path,
		json!({"id":saved.id,"response":{"answer":"Continue"}}),
	)
	.await;
	assert_eq!(status, 200, "{answered}");
	assert_eq!(
		home.human_request_by_id(saved.id).await.unwrap().response,
		Some(json!({"answer":"Continue"}))
	);
	let approval = home
		.human_request(
			"APPROVAL_REQUIRED",
			"Approve this exact saved operation?",
			"binding-approval",
		)
		.await
		.unwrap();
	let response = json!({"approved":true});
	let input = json!({"id":approval.id,"response":response});
	let first = request(&p.aa, &p.token, "POST", &answer_path, input.clone()).await;
	assert_eq!(first.0, 200, "{}", first.1);
	assert_eq!(
		request(&p.aa, &p.token, "POST", &answer_path, input).await,
		first
	);
	assert_eq!(
		home.human_request_by_id(approval.id)
			.await
			.unwrap()
			.response,
		Some(response)
	);
	let denial = home
		.human_request(
			"APPROVAL_REQUIRED",
			"Deny the other saved operation?",
			"binding-denial",
		)
		.await
		.unwrap();
	let denied = request(
		&p.aa,
		&p.token,
		"POST",
		&answer_path,
		json!({"id":denial.id,"response":{"approved":false}}),
	)
	.await;
	assert_eq!(denied.0, 200, "{}", denied.1);
	let unanswered = home
		.human_request(
			"APPROVAL_REQUIRED",
			"Expire an unanswered operation",
			"binding-unanswered",
		)
		.await
		.unwrap();
	// Advance only the Home journal's request deadlines. The answers above
	// were durably accepted in time; later polling must retain both decisions.
	let mut journal: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("human_requests"))
			.from(Alias::new("authorization_remote_execution"))
			.and_where(Expr::col(Alias::new("grant_id")).eq(Expr::value(p.grant)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(p.a.store.pool.driver())
	.await
	.unwrap();
	for record in journal.as_array_mut().unwrap() {
		record["request"]["created_at"] = json!(chrono::Utc::now() - chrono::Duration::minutes(16));
	}
	sqlx::query(
		&Query::update()
			.table(Alias::new("authorization_remote_execution"))
			.value(Alias::new("human_requests"), journal)
			.and_where(Expr::col(Alias::new("grant_id")).eq(Expr::value(p.grant)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(p.a.store.pool.driver())
	.await
	.unwrap();
	for (id, expected, actor) in [
		(
			approval.id,
			json!({"approved":true}),
			first.1["answered_by"].clone(),
		),
		(
			denial.id,
			json!({"approved":false}),
			denied.1["answered_by"].clone(),
		),
	] {
		for _ in 0..2 {
			let retained = home.human_request_by_id(id).await.unwrap();
			assert_eq!(retained.response, Some(expected.clone()));
			assert_eq!(json!(retained.answered_by), actor);
		}
	}
	let expired = home.human_request_by_id(unanswered.id).await.unwrap();
	assert_eq!(
		expired.response,
		Some(json!({"approved":false,"expired":true}))
	);
	assert_eq!(expired.answered_by.as_deref(), Some("system"));
	let (status, wrong) = request(
		&p.aa,
		&p.token,
		"POST",
		&answer_path,
		json!({"id":Uuid::new_v4(),"response":{"answer":"Wrong request"}}),
	)
	.await;
	assert_eq!(status, 403, "{wrong}");
	let (status, revoked) = request(
		&p.aa,
		&p.token,
		"POST",
		&format!("/api/tasks/{}/remote-grants/{}/revoke", p.task, p.grant),
		json!({}),
	)
	.await;
	assert_eq!(status, 200, "{revoked}");
	let (status, denied) = request(
		&p.aa,
		&p.token,
		"POST",
		&answer_path,
		json!({"id":saved.id,"response":{"answer":"Continue"}}),
	)
	.await;
	assert_eq!(status, 403, "{denied}");
	assert!(home.human_request_by_id(saved.id).await.is_err());
	assert!(
		home.human_request("QUESTION", "After revocation", "revoked")
			.await
			.is_err()
	);
	p.close().await;
}
