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
	let ids: [Uuid; 4] = std::array::from_fn(|_| Uuid::now_v7());
	let resource = "https://mcp.example/mcp";
	let prompts = [resource, resource, "https://other.example/mcp", resource];
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
				"QUESTION".into_value(),
				json!({"resource":prompts[index]}).to_string().into_value(),
				format!("oauth-{id}").into_value(),
				chrono::Utc::now().into_value(),
				response.into_value(),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await.unwrap();
	}
	tx.commit().await.unwrap();
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
		"QUESTION",
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
	answer_pending(
		&f.store.control_pool,
		&f.store.pool,
		&owner,
		"QUESTION",
		resource,
	)
	.await
	.unwrap();
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
				2 => None,
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
		resource: resource.into(),
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

/// These routes never need an outbound OAuth request: pending resources have
/// no token, and a Subject Credential must fail before authorization discovery.
struct OfflineOAuth;
#[async_trait::async_trait]
impl aidash_application::ports::oauth::OAuthDiscovery for OfflineOAuth {
	async fn protected_resource(
		&self,
		_: &str,
	) -> aidash_application::Result<Option<aidash_application::ports::oauth::ProtectedResource>> {
		Err(aidash_application::Error::External("offline".into()))
	}
	async fn authorization_server(
		&self,
		_: &str,
	) -> aidash_application::Result<aidash_application::ports::oauth::AuthorizationServer> {
		Err(aidash_application::Error::External("offline".into()))
	}
	async fn register(
		&self,
		_: &aidash_application::ports::oauth::AuthorizationServer,
		_: &reqwest::Url,
		_: &str,
	) -> aidash_application::Result<aidash_application::ports::oauth::ClientCredentials> {
		Err(aidash_application::Error::External("offline".into()))
	}
}
#[async_trait::async_trait]
impl aidash_application::ports::oauth::OAuthTokens for OfflineOAuth {
	async fn exchange(
		&self,
		_: &aidash_application::ports::oauth::AuthorizationServer,
		_: &aidash_application::ports::oauth::ClientCredentials,
		_: &str,
		_: &secrecy::SecretString,
		_: &reqwest::Url,
		_: &str,
	) -> aidash_application::Result<aidash_application::ports::oauth::Tokens> {
		Err(aidash_application::Error::External("offline".into()))
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
		discovery: Arc::new(OfflineOAuth),
		tokens: Arc::new(OfflineOAuth),
		policy: Arc::new(
			aidash_server::apps::identity::repositories::oauth_connections::UnconfiguredPolicy,
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
	let app = common::application(f.clone()).await;
	let base = "/api/tenants/acme/connections";
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
