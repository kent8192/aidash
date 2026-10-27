//! PostgreSQL proof for Connection owner scopes and resource-bound human answers.
#![allow(clippy::disallowed_methods)] // Disposable fixture access only.
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_application::{
	oauth_connections::{Owner, Repository},
	tenancy::TEST_FIXTURE,
};
use aidash_server::apps::identity::repositories::oauth_connections::{
	NativeRepository, answer_pending,
};
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, IntoValue, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::{Value, json};
use uuid::Uuid;

#[rstest::rstest]
#[tokio::test]
async fn oauth_connections_answer_only_unanswered_requests_of_the_resource_and_chain_root(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, _) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status,created)=common::request(&app,&token,"POST","/api/conversations",json!({"title":"Connect MCP","goal":"Use the connected server","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"})).await;
	assert_eq!(status, 200, "{created}");
	let scope = aidash_application::tenancy::TenancyScope::tenant("acme").unwrap();
	let run = f.store.runs(&scope).await.unwrap().pop().unwrap();
	let ids: [Uuid; 6] = std::array::from_fn(|_| Uuid::now_v7());
	let resource = "https://mcp.example/mcp";
	let prompts = [
		resource,
		resource,
		"https://other.example/mcp",
		resource,
		resource,
		resource,
	];
	let mut tx = f
		.store
		.pool
		.begin(&scope)
		.await
		.unwrap()
		.into_executor()
		.await
		.unwrap();
	for (index, id) in ids.iter().enumerate() {
		let response = if index == 3 {
			Some(json!({"connected":false}))
		} else {
			None
		};
		let (sql, values) = Query::insert()
			.into_table(Alias::new("human_requests"))
			.columns(
				[
					"id",
					"tenant",
					"workspace_id",
					"run_id",
					"kind",
					"prompt",
					"request_key",
					"created_at",
					"response",
				]
				.map(Alias::new),
			)
			.values_panic([
				(*id).into_value(),
				"acme".into_value(),
				run.workspace_id.into_value(),
				run.id.into_value(),
				(if index == 4 {
					"QUESTION"
				} else {
					aidash_domain::oauth_connections::CONNECTION_REQUEST_KIND
				})
				.into_value(),
				json!({"resource":prompts[index]}).to_string().into_value(),
				format!("oauth-{id}").into_value(),
				chrono::Utc::now().into_value(),
				response.into_value(),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await.unwrap();
	}
	// One live wait has elapsed; even a successful OAuth callback must close it false.
	let mut waiting_run = run.clone();
	waiting_run.state =
		aidash_domain::RunState::Waiting(Box::new(aidash_domain::WaitingState::Human {
			request_id: ids[5],
			answer_by: Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
			resume: aidash_domain::ResumeState::Thinking(Default::default()),
		}));
	let (sql, values) = Query::update()
		.table(Alias::new("runs"))
		.value(Alias::new("phase"), "WAITING")
		.value(Alias::new("pending"), waiting_run.stored_pending().unwrap())
		.and_where(reinhardt::query::ExprTrait::eq(
			Expr::col("id"),
			Expr::value(run.id),
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await.unwrap();
	tx.commit().await.unwrap();
	let (status, _) = common::request(
		&app,
		&token,
		"POST",
		&format!("/api/human-requests/{}/answer", ids[0]),
		json!({"connected":true}),
	)
	.await;
	assert_eq!(
		status, 400,
		"ordinary answers cannot impersonate an OAuth callback"
	);
	let owner = Owner {
		tenant: "acme".into(),
		subject: "alice".into(),
	};
	answer_pending(
		&f.store.control_pool,
		&f.store.pool,
		&Owner {
			tenant: "acme".into(),
			subject: "bob".into(),
		},
		resource,
	)
	.await
	.unwrap();
	let before: Option<Value> = aidash_server::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("response"))
			.from(Alias::new("human_requests"))
			.and_where(reinhardt::query::ExprTrait::eq(
				Expr::col("id"),
				Expr::value(ids[0]),
			))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(f.store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	assert!(before.is_none());
	// Exercise complete through the real repository and encrypted Store: its
	// committed consent, rather than a normal human answer, releases these waits.
	let repository = std::sync::Arc::new(NativeRepository {
		pool: f.store.control_pool.clone(),
		execution: f.store.pool.clone(),
	});
	let store = std::sync::Arc::new(
		aidash_server::apps::identity::repositories::secret_store::PostgresStore::new(
			f.store.control_pool.clone(),
			"11".repeat(32).into(),
			vec![],
		)
		.await
		.unwrap(),
	);
	let service=aidash_application::oauth_connections::Service {repository,store:store.clone(),reader:store,discovery:std::sync::Arc::new(FixtureOAuth),tokens:std::sync::Arc::new(FixtureOAuth),policy:std::sync::Arc::new(aidash_server::apps::identity::repositories::connection_policy::NativeConnectionPolicy {pool:f.store.control_pool.clone()})};
	let began = service
		.begin(
			&owner,
			resource,
			&["read".into()],
			None,
			&mut NoTool,
			&"https://aidash.example/auth/connections/callback"
				.parse()
				.unwrap(),
			"/dashboard",
		)
		.await
		.unwrap();
	assert_eq!(
		service
			.complete(began.pending, Some("accepted"), None)
			.await
			.unwrap(),
		aidash_application::oauth_connections::Outcome::Connected
	);
	let mut tx = f
		.store
		.pool
		.begin(&scope)
		.await
		.unwrap()
		.into_executor()
		.await
		.unwrap();
	for (index, id) in ids.iter().enumerate() {
		let value: Option<Value> = aidash_server::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("response"))
				.from(Alias::new("human_requests"))
				.and_where(reinhardt::query::ExprTrait::eq(
					Expr::col("id"),
					Expr::value(*id),
				))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(tx.as_mut())
		.await
		.unwrap();
		assert_eq!(
			value,
			match index {
				0 | 1 => Some(json!({"connected":true})),
				2 | 4 => None,
				_ => Some(json!({"connected":false})),
			}
		);
	}
	tx.commit().await.unwrap();
	// The real owner scope also preserves resource uniqueness and does not expose another subject's rows.
	let repository = NativeRepository {
		pool: f.store.control_pool.clone(),
		execution: f.store.pool.clone(),
	};
	let now = chrono::Utc::now();
	let id = Uuid::now_v7();
	let row = aidash_domain::oauth_connections::OAuthConnection {
		id,
		tenant: "acme".into(),
		subject: "alice".into(),
		resource: "https://unique.example/mcp".into(),
		issuer: "https://auth.example".into(),
		client: aidash_domain::oauth_connections::ClientRef::Dynamic {
			client_id: "client".into(),
		},
		scopes: vec!["read".into()],
		secret_resource: format!("aidash-store/oauth-connections/{id}"),
		pinned_version: String::new(),
		state: aidash_domain::oauth_connections::ConnectionState::Pending,
		access_expires_at: None,
		refresh_expires_at: None,
		created_at: now,
		updated_at: now,
		revision: 1,
	};
	let mut owned = repository.begin(&owner).await.unwrap();
	owned.save(&row).await.unwrap();
	owned.commit().await.unwrap();
	let mut other = repository
		.begin(&Owner {
			tenant: "acme".into(),
			subject: "bob".into(),
		})
		.await
		.unwrap();
	assert!(other.list().await.unwrap().is_empty());
	assert!(other.delete(id).await.is_err());
	other.commit().await.unwrap();
	let mut owned = repository.begin(&owner).await.unwrap();
	let mut duplicate = row.clone();
	duplicate.id = Uuid::now_v7();
	assert!(owned.save(&duplicate).await.is_err());
	drop(owned);
	common::cleanup(f, &url, &schema).await;
}

/// The OAuth side is scripted; persistence and human-request delivery are real.
/// Subject Credential routes must still refuse before starting authorization.
struct FixtureOAuth;
struct NoTool;
#[async_trait::async_trait]
impl aidash_application::ports::registry::DefinitionLookup for NoTool {
	async fn definition(
		&mut self,
		_id: &str,
		_version: &str,
	) -> aidash_application::Result<aidash_domain::registry::Entry> {
		Err(aidash_application::Error::Forbidden)
	}
}
#[async_trait::async_trait]
impl aidash_application::ports::oauth::OAuthDiscovery for FixtureOAuth {
	async fn protected_resource(
		&self,
		resource: &str,
	) -> aidash_application::Result<Option<aidash_application::ports::oauth::ProtectedResource>> {
		Ok(Some(aidash_application::ports::oauth::ProtectedResource {
			resource: resource.into(),
			authorization_servers: vec!["https://auth.example".into()],
			scopes_supported: Some(vec!["read".into(), "write".into()]),
		}))
	}
	async fn authorization_server(
		&self,
		_: &str,
	) -> aidash_application::Result<aidash_application::ports::oauth::AuthorizationServer> {
		Ok(aidash_application::ports::oauth::AuthorizationServer {
			issuer: "https://auth.example".into(),
			authorization_endpoint: "https://auth.example/authorize".parse().unwrap(),
			token_endpoint: "https://auth.example/token".parse().unwrap(),
			registration_endpoint: Some("https://auth.example/register".parse().unwrap()),
			revocation_endpoint: None,
			scopes_supported: Some(vec!["read".into(), "write".into()]),
			code_challenge_methods_supported: vec!["S256".into()],
		})
	}
	async fn register(
		&self,
		_: &aidash_application::ports::oauth::AuthorizationServer,
		_: &reqwest::Url,
		_: &str,
	) -> aidash_application::Result<aidash_application::ports::oauth::ClientCredentials> {
		Ok(aidash_application::ports::oauth::ClientCredentials {
			client_id: "fixture-client".into(),
			client_secret: None,
		})
	}
}
#[async_trait::async_trait]
impl aidash_application::ports::oauth::OAuthTokens for FixtureOAuth {
	async fn exchange(
		&self,
		_: &aidash_application::ports::oauth::AuthorizationServer,
		_: &aidash_application::ports::oauth::ClientCredentials,
		_: &str,
		_: &secrecy::SecretString,
		_: &reqwest::Url,
		_: &str,
	) -> aidash_application::Result<aidash_application::ports::oauth::Tokens> {
		Ok(aidash_application::ports::oauth::Tokens {
			access_token: "fixture-access".into(),
			refresh_token: None,
			expires_in: Some(3600),
			scope: Some("read".into()),
		})
	}
	async fn refresh(
		&self,
		_: &aidash_application::ports::oauth::AuthorizationServer,
		_: &aidash_application::ports::oauth::ClientCredentials,
		_: &secrecy::SecretString,
		_: &str,
	) -> Result<
		aidash_application::ports::oauth::Tokens,
		aidash_application::plan_connections::RefreshFailure,
	> {
		Err(
			aidash_application::plan_connections::RefreshFailure::Unavailable(
				aidash_application::Error::External("offline".into()),
			),
		)
	}
	async fn revoke(
		&self,
		_: &aidash_application::ports::oauth::AuthorizationServer,
		_: &aidash_application::ports::oauth::ClientCredentials,
		_: &secrecy::SecretString,
	) -> aidash_application::Result<()> {
		Err(aidash_application::Error::External("offline".into()))
	}
}
#[rstest::rstest]
#[tokio::test]
async fn oauth_connections_subject_credentials_manage_only_their_persons_connections(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	use aidash_application::provider_credentials::Store as _;
	use std::sync::Arc;
	let (mut f, url, schema) = fixture.runtime.parts();
	let (mut policy, alice, _) =
		common::bootstrap(&f, &fixture.application, "http://127.0.0.1:9").await;
	policy["subjects"]["bob"] = json!({"kind":"user"});
	policy["subjects"]["bot"] = json!({"kind":"service"});
	policy["subjects"]["charlie"] = json!({"kind":"user"});
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"bob-cannot-update-ceilings", "effect":"deny", "subjects":{"ids":["bob"]},
		"actions":["connection.scopes.update"], "resources":{"kinds":["connection_scopes"]}
	}));
	policy["policies"].as_array_mut().unwrap().extend([
        json!({"id":"alice-registry-only", "effect":"deny", "subjects":{"ids":["alice"]}, "actions":["secret.use"], "resources":{"kinds":["secret"]}}),
        json!({"id":"bob-secret-only", "effect":"deny", "subjects":{"ids":["bob"]}, "actions":["registry.create"], "resources":{"kinds":["registry"]}}),
        json!({"id":"charlie-cannot-probe", "effect":"deny", "subjects":{"ids":["charlie"]}, "actions":["registry.create", "secret.use"], "resources":{"kinds":["registry", "secret"]}}),
    ]);
	let authorization = aidash_server::authorization::Authorization {
		pool: f.store.control_pool.clone(),
	};
	authorization
		.replace(
			"acme",
			1,
			serde_json::from_value(policy).unwrap(),
			"deployment_administrator",
		)
		.await
		.unwrap();
	let bob = authorization
		.issue_credential("acme", "bob", 3600, "deployment_administrator")
		.await
		.unwrap()
		.token;
	let charlie = authorization
		.issue_credential("acme", "charlie", 3600, "deployment_administrator")
		.await
		.unwrap()
		.token;
	let bot = authorization
		.issue_credential("acme", "bot", 3600, "deployment_administrator")
		.await
		.unwrap()
		.token;
	let store = Arc::new(
		aidash_server::apps::identity::repositories::secret_store::PostgresStore::new(
			f.store.control_pool.clone(),
			"11".repeat(32).into(),
			vec![],
		)
		.await
		.unwrap(),
	);
	let repository = Arc::new(NativeRepository {
		pool: f.store.control_pool.clone(),
		execution: f.store.pool.clone(),
	});
	f.store.oauth_connections = Some(Arc::new(aidash_application::oauth_connections::Service {
		repository: repository.clone(),
		store: store.clone(),
		reader: store.clone(),
		discovery: Arc::new(FixtureOAuth),
		tokens: Arc::new(FixtureOAuth),
		policy: Arc::new(
			aidash_server::apps::identity::repositories::connection_policy::NativeConnectionPolicy {
                pool: f.store.control_pool.clone(),
            },
		),
	}));
	let now = chrono::Utc::now();
	let ids: [Uuid; 2] = std::array::from_fn(|_| Uuid::now_v7());
	for (id, subject) in ids.iter().zip(["alice", "bob"]) {
		store.create_connection("acme", *id).await.unwrap();
		let row = aidash_domain::oauth_connections::OAuthConnection {
			id: *id,
			tenant: "acme".into(),
			subject: subject.into(),
			resource: "https://mcp.example/mcp".into(),
			issuer: "https://auth.example".into(),
			client: aidash_domain::oauth_connections::ClientRef::Dynamic {
				client_id: "client".into(),
			},
			scopes: vec!["read".into()],
			secret_resource: store.connection_resource(*id),
			pinned_version: String::new(),
			state: aidash_domain::oauth_connections::ConnectionState::Pending,
			access_expires_at: None,
			refresh_expires_at: None,
			created_at: now,
			updated_at: now,
			revision: 1,
		};
		let mut owned = repository
			.begin(&Owner {
				tenant: "acme".into(),
				subject: subject.into(),
			})
			.await
			.unwrap();
		owned.save(&row).await.unwrap();
		owned.commit().await.unwrap();
	}
	let ceilings = aidash_application::oauth_clients::Service {
		repository: Arc::new(
			aidash_server::apps::identity::repositories::oauth_clients::NativeRepository {
				pool: f.store.control_pool.clone(),
			},
		),
		store: None,
	};
	ceilings
		.put_ceiling("acme", "https://mcp.example/mcp", vec!["read".into()], 0)
		.await
		.unwrap();
	let app = common::application(f.clone()).await;
	let base = "/api/tenants/acme/connections";
	let (status, _) = common::request(
		&app,
		&charlie,
		"GET",
		&format!("{base}/probe?resource=https%3A%2F%2Fmcp.example%2Fmcp"),
		Value::Null,
	)
	.await;
	assert_eq!(
		status, 403,
		"a member without registry.create or secret.use cannot probe"
	);
	let (status, probe) = common::request(
		&app,
		&alice,
		"GET",
		&format!("{base}/probe?resource=https%3A%2F%2Fmcp.example%2Fmcp"),
		Value::Null,
	)
	.await;
	assert_eq!(
		status, 200,
		"registry.create alone permits probing: {probe}"
	);
	// The picker needs Tenant bounds even when its person cannot edit them.
	let (status, probe) = common::request(
		&app,
		&bob,
		"GET",
		&format!("{base}/probe?resource=https%3A%2F%2FMCP.example%2Fmcp%2F"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{probe}");
	assert_eq!(
		probe,
		json!({"oauth":true, "issuer":"https://auth.example", "scopes_supported":["read","write"], "scope_ceiling":["read"]})
	);
	use base64::Engine as _;
	let encoded =
		base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("https://mcp.example/mcp");
	let (status, _) = common::request(
		&app,
		&bob,
		"GET",
		&format!("/api/tenants/acme/connection-scope-ceilings/{encoded}"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 403);
	let (status, probe) = common::request(
		&app,
		&bob,
		"GET",
		&format!("{base}/probe?resource=https%3A%2F%2Fother.example%2Fmcp"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{probe}");
	assert!(probe.get("scope_ceiling").is_none());
	let (status, listed) = common::request(&app, &alice, "GET", base, Value::Null).await;
	assert_eq!(status, 200, "{listed}");
	assert_eq!(listed.as_array().unwrap().len(), 1);
	assert_eq!(listed[0]["id"], json!(ids[0]));
	let (status, _) = common::request(
		&app,
		&alice,
		"DELETE",
		&format!("{base}/{}", ids[1]),
		Value::Null,
	)
	.await;
	assert_eq!(status, 404);
	let (status, _) = common::request(
		&app,
		&alice,
		"POST",
		&format!("{base}/authorizations"),
		json!({"resource":"https://mcp.example/mcp"}),
	)
	.await;
	assert_eq!(status, 403);
	let (status, deleted) = common::request(
		&app,
		&alice,
		"DELETE",
		&format!("{base}/{}", ids[0]),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{deleted}");
	let (status, remaining) = common::request(&app, &alice, "GET", base, Value::Null).await;
	assert_eq!(status, 200);
	assert_eq!(remaining, json!([]));
	let (status, listed) = common::request(&app, &bob, "GET", base, Value::Null).await;
	assert_eq!(status, 200, "{listed}");
	assert_eq!(listed[0]["id"], json!(ids[1]));
	let (status, _) = common::request(&app, &bot, "GET", base, Value::Null).await;
	assert_eq!(status, 403);
	common::cleanup(f, &url, &schema).await;
}

mod mcp_connection_owner;

#[rstest::rstest]
#[tokio::test]
async fn oauth_connections_refusal_checkpoint_is_lease_fenced_and_preserves_other_attempts(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, _) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status,created)=common::request(&app,&token,"POST","/api/conversations",json!({"title":"Connect MCP","goal":"Use the connected server","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"})).await;
	assert_eq!(status, 200, "{created}");
	let scope = aidash_application::tenancy::TenancyScope::tenant("acme").unwrap();
	let worker = Uuid::now_v7();
	let mut run = f.store.lease_run(worker, 600).await.unwrap().unwrap();
	run.state = aidash_domain::RunState::ToolCall(Box::default());
	for key in ["refused", "other-started", "already-completed"] {
		f.store
			.invocation_start(&scope, &run, worker, key, "crm", &json!({}), false)
			.await
			.unwrap();
	}
	f.store
		.invocation_finish(
			&scope,
			&run,
			worker,
			"already-completed",
			&json!({"ok":true}),
		)
		.await
		.unwrap();
	let cause = aidash_domain::agent_plugin::ConnectionCause::ConnectionRequired {
		resource: "https://mcp.example".into(),
	};
	run.state.tool_mut().unwrap().connection =
		Some(aidash_domain::run_state::ConnectionCallState {
			cause: Some(cause.clone()),
			refused: Some(json!({"error":"connection_required","resource":"https://mcp.example"})),
			..Default::default()
		});
	aidash_application::connection_wait::record_refusal(&mut run, "crm", cause.clone(), vec![]);
	assert!(
		f.store
			.connection_refused(&scope, &run, Uuid::now_v7(), "refused")
			.await
			.is_err()
	);
	let unchanged = f.store.run(&scope, run.id).await.unwrap();
	assert!(
		unchanged.state.tool().unwrap().connection.is_none(),
		"a stale worker cannot persist the typed fact"
	);
	f.store
		.connection_refused(&scope, &run, worker, "refused")
		.await
		.unwrap();
	f.store
		.connection_refused(&scope, &run, worker, "already-completed")
		.await
		.unwrap();
	let recovered = f.store.run(&scope, run.id).await.unwrap();
	assert_eq!(
		recovered
			.state
			.tool()
			.unwrap()
			.connection
			.as_ref()
			.unwrap()
			.cause
			.as_ref(),
		Some(&cause)
	);
	assert_eq!(recovered.recovery.connection_refusals[0].cause, cause);
	for (key, expected) in [
		("refused", None),
		("other-started", Some("STARTED")),
		("already-completed", Some("COMPLETED")),
	] {
		let query = Query::select()
			.column(Alias::new("status"))
			.from(Alias::new("invocations"))
			.and_where(reinhardt::query::ExprTrait::eq(
				Expr::col("run_id"),
				Expr::value(run.id),
			))
			.and_where(reinhardt::query::ExprTrait::eq(
				Expr::col("idempotency_key"),
				Expr::value(key),
			))
			.to_string(PostgresQueryBuilder);
		let status: Option<String> = aidash_server::database::native::query_scalar(&query)
			.scalar_optional(f.store.pool.in_scope(&scope))
			.await
			.unwrap();
		assert_eq!(status.as_deref(), expected, "{key}");
	}
	common::cleanup(f, &url, &schema).await;
}
