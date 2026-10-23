use super::*;
use aidash_application::plan_connections::Owner;
use aidash_domain::plan_connections::{PlanConnection, State};
use aidash_server::apps::identity::repositories::secret_store::PostgresStore;

fn owner() -> Owner {
	Owner {
		tenant: "alpha".into(),
		subject: "alice".into(),
	}
}

async fn service_with_key(
	fixture: &Fixture,
	current: &str,
	retired: Vec<SecretString>,
) -> (Service, Arc<PostgresStore>) {
	let store = Arc::new(
		PostgresStore::new(
			fixture.f.store.control_pool.clone(),
			current.to_owned().into(),
			retired,
		)
		.await
		.unwrap(),
	);
	(
		Service {
			repository: Arc::new(NativeRepository {
				pool: fixture.f.store.control_pool.clone(),
				node: fixture.f.store.deployment.to_string(),
			}),
			store: store.clone(),
			reader: store.clone(),
			authorization: Arc::new(fixture.openai.clone()),
		},
		store,
	)
}

async fn interrupt_refresh(fixture: &Fixture, after_commit: bool) -> PlanConnection {
	let (mut service, store) = service_with_key(fixture, &"41".repeat(32), vec![]).await;
	let row = connect_durable(fixture, &service).await;
	let interrupted = Arc::new(InterruptedStore {
		inner: store,
		after_commit,
		after_destroy: false,
		reached: Default::default(),
	});
	service.store = interrupted.clone();
	let pin = row.pin();
	let task = tokio::spawn(async move { service.bearer(pin).await });
	tokio::time::timeout(
		std::time::Duration::from_secs(15),
		interrupted.reached.notified(),
	)
	.await
	.unwrap();
	task.abort();
	assert!(task.await.unwrap_err().is_cancelled());
	row
}

/// Deleting encrypted evidence and rolling back the metadata deletion must
/// not make a retried disconnect claim the unknown upstream grant was revoked.
#[rstest::rstest]
#[case::with_pin(false)]
#[case::without_pin(true)]
#[tokio::test]
async fn uncertainty_survives_interrupted_terminal_cleanup(
	#[case] expire: bool,
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(plan_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	map(&fixture, "browser-a", "alice").await;
	let row = interrupt_refresh(&fixture, false).await;
	let (mut service, store) = service_with_key(&fixture, &"41".repeat(32), vec![]).await;
	assert!(matches!(
		service.bearer(row.pin()).await,
		Err(Error::Plan(Failure::NeedsReconnection))
	));
	let row = connect_durable(&fixture, &service).await;
	if expire {
		let mut scope = service.repository.begin(&owner()).await.unwrap();
		let mut expired = scope.owned(Provider::Chatgpt).await.unwrap().unwrap();
		expired.refresh_expires_at = chrono::Utc::now();
		scope
			.save(&expired, "fixture.expired", "fixture")
			.await
			.unwrap();
		scope.commit().await.unwrap();
		assert!(matches!(
			service.bearer(row.pin()).await,
			Err(Error::Plan(Failure::NeedsReconnection))
		));
	}

	let status = service.status(&owner()).await.unwrap().unwrap();
	let interrupted = Arc::new(InterruptedStore {
		inner: store.clone(),
		after_commit: false,
		after_destroy: true,
		reached: Default::default(),
	});
	service.store = interrupted.clone();
	let task = tokio::spawn(async move {
		service
			.disconnect(&owner(), status.revision, "fixture")
			.await
	});
	tokio::time::timeout(
		std::time::Duration::from_secs(15),
		interrupted.reached.notified(),
	)
	.await
	.unwrap();
	task.abort();
	assert!(task.await.unwrap_err().is_cancelled());
	let (service, store) = service_with_key(&fixture, &"41".repeat(32), vec![]).await;
	let status = service.status(&owner()).await.unwrap().unwrap();
	assert!(
		!service
			.disconnect(&owner(), status.revision, "fixture")
			.await
			.unwrap()
			.revoked
	);
	assert!(
		store
			.versions(&row.tenant, &row.secret_resource)
			.await
			.unwrap()
			.is_empty()
	);
	fixture.cleanup().await;
}

/// Offline key recovery must preserve uncertainty for an old unpinned
/// replacement even when the current connection pin uses a surviving key.
#[rstest::rstest]
#[tokio::test]
async fn key_recovery_preserves_unpinned_grant_uncertainty(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(plan_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	map(&fixture, "browser-a", "alice").await;
	interrupt_refresh(&fixture, true).await;
	let current = "42".repeat(32);
	let (service, _) = service_with_key(&fixture, &current, vec!["41".repeat(32).into()]).await;
	let row = connect_durable(&fixture, &service).await;
	let directory = tempfile::tempdir().unwrap();
	std::fs::write(directory.path().join("master"), &current).unwrap();
	std::fs::write(
		directory.path().join("fingerprint"),
		"plan-recovery-fingerprint-at-least-32-bytes",
	)
	.unwrap();
	std::fs::write(
		directory.path().join("base.toml"),
		format!(
			r#"[core]
secret_key = "plan-recovery-test-secret-at-least-50-characters-long-159"
[deployment]
id = "{}"
endpoint = "http://127.0.0.1:8080"
api_token = "plan-recovery-api-token-159"
[provider_credentials]
fingerprint_key = {{ file = {:?} }}
[provider_credentials.store]
kind = "postgres"
master_key = {{ file = {:?} }}
"#,
			fixture.f.store.deployment,
			directory.path().join("fingerprint"),
			directory.path().join("master")
		),
	)
	.unwrap();
	for execute in [false, true] {
		let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_manage"));
		command.arg("secret-store-recovery");
		if execute {
			command.arg("--execute");
		}
		command
			.env("DATABASE_URL", &fixture.f.config.database_url)
			.env("REINHARDT_SETTINGS_DIR", directory.path())
			.env("REINHARDT_ENV", "local")
			.kill_on_drop(true);
		let output = tokio::time::timeout(std::time::Duration::from_secs(60), command.output())
			.await
			.unwrap()
			.unwrap();
		assert!(
			output.status.success(),
			"{}",
			String::from_utf8_lossy(&output.stderr)
		);
		let counts: Value = serde_json::from_slice(&output.stdout).unwrap();
		assert_eq!(counts["affected_plan_connections"], 1);
		assert!(!String::from_utf8_lossy(&output.stdout).contains("plan-refresh"));
	}
	let (service, store) = service_with_key(&fixture, &current, vec![]).await;
	let status = service.status(&owner()).await.unwrap().unwrap();
	assert_eq!(status.state, State::Active);
	assert!(
		!service
			.disconnect(&owner(), status.revision, "fixture")
			.await
			.unwrap()
			.revoked
	);
	assert!(
		store
			.versions(&row.tenant, &row.secret_resource)
			.await
			.unwrap()
			.is_empty()
	);
	fixture.cleanup().await;
}
