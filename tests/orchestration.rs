#[allow(dead_code)]
mod common;
use aidash::{api, store::Store};
use migration::MigratorTrait;
use serde_json::Value;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn simultaneous_migration_startup_preserves_one_complete_schema() {
	let (f, url, schema) = common::setup().await;
	let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(f.store.pool.clone());
	migration::Migrator::down(&db, None).await.unwrap();
	let (a, b, c) = tokio::join!(
		Store::migrate(&f.store.pool),
		Store::migrate(&f.store.pool),
		Store::migrate(&f.store.pool)
	);
	a.unwrap();
	b.unwrap();
	c.unwrap();
	assert_eq!(
		migration::Migrator::get_applied_migrations(&db)
			.await
			.unwrap()
			.len(),
		migration::Migrator::migrations().len()
	);
	f.store
		.create_workspace("Replica startup", "All migrations applied once")
		.await
		.unwrap();
	common::cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn probes_report_draining_without_restarting_for_dependency_failure() {
	let (f, url, schema) = common::setup().await;
	let app = api::router(f.clone());
	let (_, subject, _) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	assert_eq!(
		common::request(&app, &subject, "GET", "/api/deployment", Value::Null)
			.await
			.0,
		403
	);
	assert_eq!(
		common::request(
			&app,
			&f.config.api_token,
			"GET",
			"/api/deployment",
			Value::Null
		)
		.await
		.1["enabled"],
		false
	);
	let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
	let address = listener.local_addr().unwrap();
	drop(listener);
	let store = f.store.isolated_pool().await.unwrap();
	let (stop, receiver) = tokio::sync::watch::channel(false);
	let probe = tokio::spawn(aidash::lifecycle::probes(address, store.clone(), receiver));
	let client = reqwest::Client::new();
	let endpoint = format!("http://{address}");
	for _ in 0..100 {
		if client.get(format!("{endpoint}/ready")).send().await.is_ok() {
			break;
		}
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
	}
	assert_eq!(
		client
			.get(format!("{endpoint}/ready"))
			.send()
			.await
			.unwrap()
			.status(),
		200
	);
	stop.send_replace(true);
	assert_eq!(
		client
			.get(format!("{endpoint}/ready"))
			.send()
			.await
			.unwrap()
			.status(),
		503
	);
	stop.send_replace(false);
	store.control_pool.close().await;
	assert_eq!(
		client
			.get(format!("{endpoint}/ready"))
			.send()
			.await
			.unwrap()
			.status(),
		503
	);
	assert_eq!(
		client
			.get(format!("{endpoint}/live"))
			.send()
			.await
			.unwrap()
			.status(),
		200
	);
	probe.abort();
	let _ = probe.await;
	store.pool.close().await;
	common::cleanup(f, &url, &schema).await;
}
