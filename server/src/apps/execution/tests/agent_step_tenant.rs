//! Which Tenant a step's inference names when it reads the Run's Provider
//! Credential pin (#521).
use super::step_tenant;
use crate::{
	apps::{
		execution::test_database::{DatabaseFixture, database},
		identity::{
			models::provider_credentials::RunProviderCredential,
			repositories::{
				provider_credentials::{AdmittedAccess, NativeRepository},
				secret_store::PostgresStore,
			},
		},
	},
	store::Store,
};
use aidash_application::{
	provider_access::{Context, ProviderAccess, Source},
	provider_credentials::{
		KeyValidator, Service, Validation,
		testing::{KeyMaterialWritePort as _, NoExchange},
	},
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use aidash_domain::{nodes::DeploymentId, provider_credentials::Provider};
use reinhardt::db::orm::Model;
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use uuid::Uuid;

struct AcceptKeys;
#[async_trait::async_trait]
impl KeyValidator for AcceptKeys {
	async fn validate(
		&self,
		_: Provider,
		_: &SecretString,
	) -> aidash_application::Result<Validation> {
		Ok(Validation { warnings: vec![] })
	}
}

#[rstest::rstest]
#[case::tenant(Some(TenancyScope::tenant("acme").unwrap()), None)]
#[case::other_tenant(Some(TenancyScope::tenant("other").unwrap()), Some("forbidden"))]
#[case::no_authority(
	None,
	Some("identifiers require 1..256 bytes without whitespace or wildcards")
)]
#[case::no_tenant(
	Some(TenancyScope::NoTenant),
	Some("identifiers require 1..256 bytes without whitespace or wildcards")
)]
#[case::deployment(
	Some(TenancyScope::Deployment(DeploymentScopeReason::ActivationClaim)),
	Some("identifiers require 1..256 bytes without whitespace or wildcards")
)]
#[tokio::test]
async fn a_steps_scope_controls_access_to_its_persisted_provider_credential_pin(
	#[future] database: DatabaseFixture,
	#[case] scope: Option<TenancyScope>,
	#[case] refused: Option<&str>,
) {
	// Arrange: a real Tenant credential and Run pin in Reinhardt's PostgreSQL
	// fixture. The Secret Store also persists and reads the key in this Tenant.
	let database = database.await;
	let tenant = database.tenant("acme").await;
	database.tenant("other").await;
	let mut store = Store::from_pool(
		database.connection.clone().into_postgres().unwrap(),
		DeploymentId::parse("aidash://step-tenant-test").unwrap(),
	)
	.await
	.unwrap();
	let secrets = Arc::new(
		PostgresStore::new(store.control_pool.clone(), "12".repeat(32).into(), vec![])
			.await
			.unwrap(),
	);
	let service = Arc::new(Service {
		repository: Arc::new(NativeRepository {
			pool: store.control_pool.clone(),
			node: store.deployment.to_string(),
		}),
		store: secrets.clone(),
		validator: Arc::new(AcceptKeys),
		exchange: Arc::new(NoExchange),
		fingerprint_key: "step-tenant-fingerprint-key-of-32-bytes".into(),
		max_per_tenant: 5,
	});
	let credential = service
		.create(
			"acme",
			Uuid::now_v7(),
			Provider::Openrouter,
			"step-tenant-provider-key".into(),
			"deployment_administrator",
		)
		.await
		.unwrap()
		.provider_credential;
	store.provider_credentials = Some(service);
	store.provider_key_material_reader = Some(secrets);
	let run = Uuid::now_v7();
	// Current policy comes from the persisted Run's Tenant, not a synthetic pin.
	let document: aidash_domain::policy::PolicyBundle = serde_json::from_value(
		serde_json::json!({"tenant":"acme","subjects":{"fixture":{"kind":"user"}},"policies":[]}),
	)
	.unwrap();
	let bundle = crate::apps::identity::models::AuthorizationBundle::build()
		.tenant("acme")
		.revision(1)
		.document(reinhardt::db::orm::Json(serde_json::json!(document)))
		.updated_at(chrono::Utc::now())
		.finish();
	let mut seed = database.begin_in(&tenant).await;
	crate::apps::identity::models::AuthorizationBundle::objects()
		.insert_with_executor(seed.as_mut(), &bundle)
		.await
		.unwrap();
	seed.commit().await.unwrap();
	let workspace = store
		.create_workspace(&tenant, "fixture", "Step policy", "Tenant scope")
		.await
		.unwrap();
	let task = store
		.create_task(
			&tenant,
			workspace.id,
			&aidash_domain::NewTask {
				title: "Step inference".into(),
				description: "Tenant authority fixture".into(),
				requirements: serde_json::json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"fixture",
			None,
		)
		.await
		.unwrap();
	let node = aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme");
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder};
	let (sql, values) = Query::insert()
		.into_table(Alias::new("runs"))
		.columns(
			[
				"id",
				"task_id",
				"workspace_id",
				"home_node",
				"node",
				"agent_id",
				"agent_version",
				"context",
				"tenant",
			]
			.map(Alias::new),
		)
		.from_subquery(
			Query::select()
				.expr(Expr::value(run))
				.expr(Expr::value(task.id))
				.expr(Expr::value(workspace.id))
				.expr(Expr::value(node.to_string()))
				.expr(Expr::value(node.node.as_str()))
				.expr(Expr::value("fixture-agent"))
				.expr(Expr::value("1.0.0"))
				.expr(Expr::value(serde_json::json!(
					crate::context::Context::default()
				)))
				.expr(Expr::value("acme"))
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	let pin = RunProviderCredential::build()
		.id(format!("{run}:openrouter"))
		.run_id(run)
		.tenant("acme")
		.provider("openrouter")
		.provider_credential_id(credential.id)
		.finish();
	let mut tx = database.begin_in(&tenant).await;
	tx.execute(&sql, reinhardt::db::orm::execution::convert_values(values))
		.await
		.unwrap();
	RunProviderCredential::objects()
		.insert_with_executor(tx.as_mut(), &pin)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	let access = AdmittedAccess { store };
	let context = Context {
		data_protection: Default::default(),
		tenant: step_tenant(scope.as_ref()),
		run: Some(run),
		observation: None,
		maintenance: None,
		provider_credential_id: None,
		inference: None,
	};

	// Act: use the same admitted lookup as worker inference, including its ORM
	// query under row-level security, rather than checking only the Tenant name.
	let result = access
		.resolve(
			&context,
			Provider::Openrouter.base_url(),
			&Source::Tenant {
				provider: "openrouter".into(),
			},
		)
		.await;

	// Assert: only the pin's Tenant can read the admitted key. No mutable
	// Provider binding is needed to resolve an already admitted Run.
	if let Some(reason) = refused {
		assert_eq!(result.unwrap_err().to_string(), reason);
	} else {
		let resolved = result.unwrap();
		assert_eq!(resolved.endpoint, Provider::Openrouter.base_url());
		assert_eq!(resolved.bearer.expose_secret(), "step-tenant-provider-key");
	}
	let keyless = access
		.resolve(&context, Provider::Openrouter.base_url(), &Source::Keyless)
		.await;
	if context.tenant == "other" {
		assert_eq!(keyless.unwrap_err().to_string(), "forbidden");
	} else {
		let keyless = keyless.unwrap();
		assert!(keyless.bearer.expose_secret().is_empty());
		assert!(keyless.keyless.is_some());
	}
}
