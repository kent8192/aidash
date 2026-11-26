//! External service admission from a Scratch Space against PostgreSQL (#738).
#[path = "../../tests/support/settings.rs"]
pub(crate) mod settings;
#[path = "../../tests/support/statements.rs"]
pub(crate) mod statements;
#[path = "../../tests/support/tenant_roots.rs"]
pub(crate) mod tenant_roots;
use crate::apps::execution::test_database::{DatabaseFixture, database};

use crate::apps::execution::capabilities::models::egress::{RequestRecord, RunRevocation};
use crate::apps::execution::repositories::egress::ServiceSecrets;
use crate::authorization::Authorization;
use aidash_application::egress::{
	Admission, ExternalAdmission, ScopeSettings, ServiceRefusal, TunnelLimits,
};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::egress::{
	AllowedHosts, AnomalyRule, DailyBudget, SERVICE_APPROVAL_LIMITED, SERVICE_METHOD_FORBIDDEN,
	SERVICE_NOT_APPROVED, SERVICE_NOT_AVAILABLE, ScopeId,
};
use aidash_domain::registry::EntityRef;
use chrono::Utc;
use reinhardt::db::orm::Model;
use secrecy::ExposeSecret;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

const DIGEST: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn tenant_secrets(
	f: &crate::federation::Federation,
	encrypted: Arc<crate::apps::identity::repositories::secret_store::PostgresStore>,
	calls: Option<Arc<dyn aidash_application::tenant_secrets::SecretCalls>>,
) -> Arc<aidash_application::tenant_secrets::Service> {
	Arc::new(aidash_application::tenant_secrets::Service {
		repository: Arc::new(
			crate::apps::identity::repositories::tenant_secrets::NativeRepository {
				pool: f.store.control_pool.clone(),
			},
		),
		store: encrypted.clone(),
		reader: Some(encrypted),
		calls,
		fingerprint_key: "tenant-secret-fingerprint-fixture-32-bytes".into(),
		max_per_tenant: 64,
	})
}

async fn register(
	f: &crate::federation::Federation,
	id: &str,
	version: &str,
	base_url: &str,
	secret: Option<&str>,
) {
	let mut config = json!({"base_url": base_url, "methods": ["GET"]});
	if let Some(secret) = secret {
		config["secret"] = json!(secret);
	}
	register_config(f, id, version, config).await;
}

async fn register_config(
	f: &crate::federation::Federation,
	id: &str,
	version: &str,
	config: serde_json::Value,
) {
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	f.registry
		.register(
			&tenant,
			serde_json::from_value(json!({
				"id": id, "version": version, "kind": "external_service",
				"name": {"en": id}, "description": {"en": "Fixture service"},
				"config": config,
			}))
			.unwrap(),
		)
		.await
		.unwrap();
}

pub(crate) async fn set_approval(
	f: &crate::federation::Federation,
	node: &aidash_domain::nodes::NodeName,
	id: &str,
	version: &str,
	enabled: bool,
) {
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	let current = authorization
		.catalog("acme", node)
		.await
		.unwrap()
		.into_iter()
		.find(|binding| binding.entry_id == id && binding.entry_version == version);
	authorization
		.set_catalog(
			"acme",
			node,
			&EntityRef {
				id: id.to_owned(),
				version: version.to_owned(),
			},
			current.map_or(0, |binding| binding.revision),
			enabled,
			"deployment_administrator",
		)
		.await
		.unwrap();
}

async fn insert_space(f: &crate::federation::Federation, services: &[&str]) -> Uuid {
	let now = Utc::now();
	let settings = aidash_domain::scratch::ScratchSettings {
		external_services: services.iter().map(|s| (*s).to_owned()).collect(),
		..Default::default()
	};
	let space = aidash_domain::scratch::ScratchSpace {
		is_default: false,
		id: Uuid::new_v4(),
		node: tenant_roots::fixture_node_name("acme"),
		name: "services".to_owned(),
		creator: "alice".to_owned(),
		state: aidash_domain::scratch::ScratchState::Active,
		revision: 0,
		wipe_generation: 0,
		wipe_completed_generation: 0,
		settings,
		members: Default::default(),
		created_at: now,
		last_used_at: now,
	};
	let mut tx = f
		.store
		.pool
		.begin(&TenancyScope::tenant("acme").unwrap())
		.await
		.unwrap();
	crate::apps::workspaces::repositories::scratch::insert(&mut *tx, "acme", &space)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	space.id
}

fn scope(space: Uuid, run: Option<Uuid>) -> ScopeSettings {
	ScopeSettings {
		run_id: run,
		id: ScopeId {
			tenant: "acme".to_owned(),
			node: tenant_roots::fixture_node_name("acme").to_string(),
			scratch_space: space,
		},
		allowed_hosts: AllowedHosts {
			hosts: Vec::new(),
			allow_all: false,
			home_ceiling: None,
		},
		budget: DailyBudget {
			sent_bytes: 1 << 30,
			received_bytes: 1 << 30,
		},
		anomaly: AnomalyRule::default(),
		operator_allows_all: false,
		tunnel_limits: TunnelLimits::default(),
	}
}

fn admitted(outcome: ExternalAdmission) -> Admission {
	match outcome {
		ExternalAdmission::Admitted(admission) => *admission,
		ExternalAdmission::Refused(refusal) => panic!("refused: {refusal:?}"),
	}
}

fn refused(outcome: ExternalAdmission) -> ServiceRefusal {
	match outcome {
		ExternalAdmission::Refused(refusal) => refusal,
		ExternalAdmission::Admitted(admission) => {
			panic!("admitted {}", admission.definition.base_url)
		}
	}
}

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn lowered_egress_limits_are_reflected_in_settings_and_task_instructions(
	#[future] database: DatabaseFixture,
) {
	use crate::apps::workspaces::{repositories::scratch::usable_lines, services::scratch};
	use aidash_domain::egress::Limits;

	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let mut f = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	let token = tenant_roots::tenant_token(&f.store, "acme").await;
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	let node = tenant_roots::fixture_node_name("acme");
	register(&f, "images", "1.0.0", "https://v1.example.test/", None).await;
	register_config(
		&f,
		"images",
		"2.0.0",
		json!({
			"base_url": "https://v2.example.test/",
			"methods": ["GET"],
			"limits": { "received_bytes": 4096 },
		}),
	)
	.await;
	register(&f, "maps", "1.0.0", "https://maps.example.test/", None).await;
	for (id, version) in [("images", "1.0.0"), ("images", "2.0.0"), ("maps", "1.0.0")] {
		set_approval(&f, &node, id, version, true).await;
	}
	let space = insert_space(&f, &["images", "maps"]).await;
	let scope = scope(space, Some(Uuid::new_v4()));
	assert_eq!(
		admitted(
			ServiceSecrets::new(f.store.clone())
				.admit(&scope, "images", "GET", "", DIGEST)
				.await
				.unwrap()
		)
		.definition
		.base_url,
		"https://v2.example.test/",
	);
	Arc::make_mut(&mut f.store.capabilities.0).egress = Some(
		serde_json::from_value(json!({
			"listen": "127.0.0.1:0",
			"limits": Limits { received_bytes: 4095, ..Limits::default() },
		}))
		.unwrap(),
	);
	assert!(matches!(
		ServiceSecrets::new(f.store.clone()).admit(&scope, "images", "GET", "", DIGEST).await,
		Err(crate::Error::Invalid(reason)) if reason == "External service limits exceed Deployment ceilings"
	));

	let actor = authorization.authenticate(&token).await.unwrap();
	let view = scratch::execute(&f, actor, scratch::Operation::Read(space))
		.await
		.unwrap();
	let mut tx = f
		.store
		.pool
		.begin(&TenancyScope::tenant("acme").unwrap())
		.await
		.unwrap();
	let lines = usable_lines(&f.store, &mut tx, "acme", space)
		.await
		.unwrap();
	tx.rollback().await.unwrap();
	assert_eq!(
		(
			&view["external_service_status"],
			lines
				.iter()
				.map(|line| line.id.as_str())
				.collect::<Vec<_>>()
		),
		(
			&json!([
				{"id": "images", "usable": false, "reason": "limits_exceeded"},
				{"id": "maps", "usable": true},
			]),
			vec!["maps"]
		),
	);
}

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn a_scratch_space_uses_the_newest_version_approved_for_everyone_on_its_node(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let mut f = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_scope(&f.store, "acme").await;
	let encrypted = Arc::new(
		crate::apps::identity::repositories::secret_store::PostgresStore::new(
			f.store.control_pool.clone(),
			"11".repeat(32).into(),
			vec![],
		)
		.await
		.unwrap(),
	);
	let secrets = tenant_secrets(&f, encrypted.clone(), None);
	secrets
		.put(
			"acme",
			"image-key",
			aidash_application::tenant_secrets::Put {
				expected_revision: 0,
				value: Some("scratch-space-secret-canary".into()),
				hosts: Some(vec!["secure.example.test".into()]),
			},
			"alice",
		)
		.await
		.unwrap();
	f.store.tenant_secrets = Some(secrets);
	f.registry = f.registry.clone().with_tenant_secrets(true);
	register(&f, "images", "1.0.0", "https://v1.example.test/", None).await;
	register(&f, "images", "2.0.0", "https://v2.example.test/", None).await;
	register(&f, "images", "3.0.0", "https://v3.example.test/", None).await;
	register(
		&f,
		"secured-images",
		"1.0.0",
		"https://secure.example.test/",
		Some("image-key"),
	)
	.await;
	register(
		&f,
		"limited",
		"1.0.0",
		"https://limited.example.test/",
		None,
	)
	.await;
	register(
		&f,
		"pending",
		"1.0.0",
		"https://pending.example.test/",
		None,
	)
	.await;
	let node = tenant_roots::fixture_node_name("acme");
	for (id, version) in [
		("images", "1.0.0"),
		("images", "2.0.0"),
		("secured-images", "1.0.0"),
		("limited", "1.0.0"),
	] {
		set_approval(&f, &node, id, version, true).await;
	}
	// Approved only for one subject: code in a shared Pod cannot be held to it.
	sqlx::query(
		"UPDATE authorization_catalog SET restriction = '{\"subjects\":[\"alice\"],\"groups\":[]}'::jsonb \
		 WHERE tenant = 'acme' AND entry_id = 'limited'",
	)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	let space = insert_space(&f, &["images", "secured-images", "limited", "pending"]).await;
	let run = Uuid::new_v4();
	let services = ServiceSecrets::new(f.store.clone());
	let scope = scope(space, Some(run));

	// 3.0.0 is registered but not approved; 2.0.0 is the newest approved.
	let images = admitted(
		services
			.admit(&scope, "images", "GET", "maps/tile", DIGEST)
			.await
			.unwrap(),
	);
	assert_eq!(images.definition.base_url, "https://v2.example.test/");
	assert!(images.credential.is_none() && images.route.is_none());
	images.meter.check_active().await.unwrap();

	let secured = admitted(
		services
			.admit(&scope, "secured-images", "GET", "photo", DIGEST)
			.await
			.unwrap(),
	);
	assert_eq!(
		secured.credential.as_ref().unwrap().expose_secret(),
		"scratch-space-secret-canary"
	);
	let record = RequestRecord::objects()
		.filter(RequestRecord::field_id().eq(secured.request))
		.all_with_executor(
			&mut *f
				.store
				.pool
				.begin(&TenancyScope::tenant("acme").unwrap())
				.await
				.unwrap(),
		)
		.await
		.unwrap()
		.pop()
		.unwrap();
	assert_eq!(record.run_id, run);
	assert_eq!(record.via, "pod");
	assert!(record.secret_id.is_some());
	assert_eq!(record.secret_host.as_deref(), Some("secure.example.test"));

	let available = vec!["images".to_owned(), "secured-images".to_owned()];
	assert_eq!(
		refused(
			services
				.admit(&scope, "limited", "GET", "", DIGEST)
				.await
				.unwrap()
		),
		ServiceRefusal {
			code: SERVICE_APPROVAL_LIMITED,
			available: available.clone(),
		}
	);
	assert_eq!(
		refused(
			services
				.admit(&scope, "pending", "GET", "", DIGEST)
				.await
				.unwrap()
		),
		ServiceRefusal {
			code: SERVICE_NOT_APPROVED,
			available: available.clone(),
		}
	);
	assert_eq!(
		refused(
			services
				.admit(&scope, "elsewhere", "GET", "", DIGEST)
				.await
				.unwrap()
		),
		ServiceRefusal {
			code: SERVICE_NOT_AVAILABLE,
			available: available.clone(),
		}
	);
	assert_eq!(
		refused(
			services
				.admit(&scope, "images", "POST", "", DIGEST)
				.await
				.unwrap()
		),
		ServiceRefusal {
			code: SERVICE_METHOD_FORBIDDEN,
			available: Vec::new(),
		}
	);
	// A Run's starting instructions list the usable services only, with the
	// address its code calls (#738).
	let mut tx = f
		.store
		.pool
		.begin(&TenancyScope::tenant("acme").unwrap())
		.await
		.unwrap();
	let lines = crate::apps::workspaces::repositories::scratch::usable_lines(
		&f.store, &mut tx, "acme", space,
	)
	.await
	.unwrap();
	tx.rollback().await.unwrap();
	assert_eq!(
		lines
			.iter()
			.map(|line| (line.id.as_str(), line.url.as_str()))
			.collect::<Vec<_>>(),
		vec![
			("images", "http://services.aidash.internal/images/"),
			(
				"secured-images",
				"http://services.aidash.internal/secured-images/"
			),
		]
	);
	assert_eq!(lines[0].methods, vec!["GET".to_owned()]);
	// Preparation has no Run and gets no External service.
	assert!(matches!(
		services
			.admit(&self::scope(space, None), "images", "GET", "", DIGEST)
			.await,
		Err(crate::Error::Forbidden)
	));

	// Withdrawing the admitted version's approval stops the request in
	// progress; the next request falls back to the older approved version.
	set_approval(&f, &node, "images", "2.0.0", false).await;
	assert!(images.meter.check_active().await.is_err());
	let older = admitted(
		services
			.admit(&scope, "images", "GET", "maps/tile", DIGEST)
			.await
			.unwrap(),
	);
	assert_eq!(older.definition.base_url, "https://v1.example.test/");

	// Taking the service off the Scratch Space stops it too.
	sqlx::query(
		"UPDATE scratch_space_settings SET settings = jsonb_set(settings, '{external_services}', '[\"images\"]'::jsonb) WHERE space_id = $1",
	)
	.bind(space)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert!(secured.meter.check_active().await.is_err());
	older.meter.check_active().await.unwrap();

	// A revoked Run is refused, and so is its request in progress.
	let mut tx = f
		.store
		.pool
		.begin(&TenancyScope::tenant("acme").unwrap())
		.await
		.unwrap();
	RunRevocation::objects()
		.insert_with_executor(
			&mut *tx,
			&RunRevocation::build()
				.run_id(run)
				.tenant("acme".to_owned())
				.revoked_at(Utc::now())
				.reason("fixture".to_owned())
				.stopped_at(None)
				.finish(),
		)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	assert!(older.meter.check_active().await.is_err());
	assert!(matches!(
		services.admit(&scope, "images", "GET", "", DIGEST).await,
		Err(crate::Error::Forbidden)
	));

	// A brokered request names the Scratch Space as the secret's user and
	// reuses its route for the same version.
	let calls = Arc::new(RecordedCalls::default());
	f.store.tenant_secrets = Some(tenant_secrets(&f, encrypted, Some(calls.clone())));
	sqlx::query(
		"UPDATE scratch_space_settings SET settings = jsonb_set(settings, '{external_services}', '[\"secured-images\"]'::jsonb) WHERE space_id = $1",
	)
	.bind(space)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	let brokered = ServiceSecrets::new(f.store.clone());
	let other_run = self::scope(space, Some(Uuid::new_v4()));
	for _ in 0..2 {
		let admission = admitted(
			brokered
				.admit(&other_run, "secured-images", "GET", "photo", DIGEST)
				.await
				.unwrap(),
		);
		assert!(admission.credential.is_none());
		assert!(admission.route.is_some());
	}
	let grants = calls.0.lock().unwrap();
	assert_eq!(grants.len(), 1);
	assert_eq!(grants[0].destination.origin, "https://secure.example.test");
	assert!(matches!(
		&grants[0].subject,
		aidash_capability::tenant_secret::SecretSubject::ScratchSpace { tenant, space: id, .. }
			if tenant == "acme" && *id == space
	));
}

#[derive(Default)]
struct RecordedCalls(std::sync::Mutex<Vec<aidash_application::tenant_secrets::SecretCallGrant>>);
#[async_trait::async_trait]
impl aidash_application::tenant_secrets::SecretCalls for RecordedCalls {
	async fn route(
		&self,
		grant: aidash_application::tenant_secrets::SecretCallGrant,
	) -> aidash_application::Result<aidash_application::tenant_secrets::SecretRoute> {
		self.0.lock().unwrap().push(grant);
		Ok(aidash_application::tenant_secrets::SecretRoute {
			base: "https://broker.example/v1/forward".parse().unwrap(),
			token: "fixture-route-token".into(),
		})
	}
}
