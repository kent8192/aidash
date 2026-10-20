//! The `demo-data` command (#432, ADR 0079, ADR 0080) against PostgreSQL. The
//! real `manage` binary runs with the migration owner's login, as the
//! migration Job runs it, so these tests also prove that the command works
//! under `FORCE ROW LEVEL SECURITY` with that login.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;
use aidash_server::apps::identity::models::{
	AuthorizationBundle, AuthorizationCatalog, AuthorizationCatalogHistory, DemoDataLoad,
	DemoTenant, Tenant,
};
use aidash_server::registry::Search;
use native_database::{DatabaseFixture, database};
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const NODE: &str = "aidash://demo-data-test";

/// The structure the repository checks in, which every Environment loads.
fn structure() -> Value {
	let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../deploy/demo-data/structure.json");
	serde_json::from_slice(&std::fs::read(path).expect("deploy/demo-data/structure.json exists"))
		.expect("the checked-in structure is JSON")
}

/// Settings for the command; the guard is off unless `enabled`.
fn settings(directory: &Path, enabled: bool) {
	std::fs::write(
		directory.join("base.toml"),
		format!(
			r#"[core]
secret_key = "demo-data-fixture-secret-key-not-for-production-at-least-50-bytes"
[node]
node_id = {NODE:?}
endpoint = "http://127.0.0.1:8080"
api_token = "demo-data-fixture-token-at-least-16"
[demo_data]
enabled = {enabled}
"#
		),
	)
	.unwrap();
}

async fn command(f: &DatabaseFixture, directory: &Path, structure: &Path) -> std::process::Output {
	tokio::process::Command::new(env!("CARGO_BIN_EXE_manage"))
		.arg("demo-data")
		.arg("--from")
		.arg(structure)
		// The migration Job's login: it owns every table.
		.env("DATABASE_URL", &f.owner_url)
		.env("REINHARDT_SETTINGS_DIR", directory)
		.env("REINHARDT_ENV", "local")
		.env_remove("AIDASH_DEMO_DATA_ENABLED")
		.stdin(std::process::Stdio::null())
		.kill_on_drop(true)
		.output()
		.await
		.expect("spawn manage demo-data")
}

fn stdout(output: &std::process::Output) -> String {
	String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
	String::from_utf8_lossy(&output.stderr).into_owned()
}

fn write(directory: &Path, name: &str, document: &Value) -> PathBuf {
	let path = directory.join(name);
	std::fs::write(&path, serde_json::to_vec_pretty(document).unwrap()).unwrap();
	path
}

/// Everything the command writes, read back in the test-fixture scope.
#[derive(Debug, PartialEq)]
struct Rows {
	/// Structure key to Tenant name.
	tenants: BTreeMap<String, String>,
	/// Tenant name to its policy revision and subjects.
	policies: BTreeMap<String, (i64, Value)>,
	/// Tenant name to the approved `id@version` entries.
	approved: BTreeMap<String, Vec<String>>,
	history: usize,
	loads: usize,
	/// The number of Tenants of any kind.
	roots: usize,
}

async fn rows(f: &DatabaseFixture) -> Rows {
	f.fixture(async |tx| {
		let tenants = DemoTenant::objects()
			.all()
			.all_with_db(tx)
			.await?
			.into_iter()
			.map(|named| (named.key, named.tenant))
			.collect();
		let policies = AuthorizationBundle::objects()
			.all()
			.all_with_db(tx)
			.await?
			.into_iter()
			.map(|bundle| {
				(
					bundle.tenant,
					(bundle.revision, bundle.document.0["subjects"].clone()),
				)
			})
			.collect();
		let mut approved: BTreeMap<String, Vec<String>> = BTreeMap::new();
		for binding in AuthorizationCatalog::objects()
			.all()
			.all_with_db(tx)
			.await?
		{
			assert!(binding.enabled);
			approved
				.entry(binding.tenant)
				.or_default()
				.push(format!("{}@{}", binding.entry_key, binding.entry_version));
		}
		approved.values_mut().for_each(|entries| entries.sort());
		Ok(Rows {
			tenants,
			policies,
			approved,
			history: AuthorizationCatalogHistory::objects()
				.count_with_conn(tx)
				.await? as usize,
			loads: DemoDataLoad::objects().count_with_conn(tx).await? as usize,
			roots: Tenant::objects().count_with_conn(tx).await? as usize,
		})
	})
	.await
}

#[rstest]
#[tokio::test]
async fn demo_data_loads_once_and_adds_only_what_is_missing(#[future] database: DatabaseFixture) {
	// Arrange: a migrated database, as the migration Job leaves it.
	let f = database.await;
	let directory = tempfile::tempdir().unwrap();
	let inputs = tempfile::tempdir().unwrap();
	let original = write(inputs.path(), "structure.json", &structure());

	// Act / Assert: the guard is off, so nothing is written.
	settings(directory.path(), false);
	let refused = command(&f, directory.path(), &original).await;
	assert!(!refused.status.success());
	assert!(
		stderr(&refused).contains("demo-data is off"),
		"{}",
		stderr(&refused)
	);
	let empty = rows(&f).await;
	assert_eq!(
		(
			empty.roots,
			empty.loads,
			empty.tenants.len(),
			empty.policies.len()
		),
		(0, 0, 0, 0)
	);

	// Act: the first load with the guard on.
	settings(directory.path(), true);
	let first = command(&f, directory.path(), &original).await;
	// Assert: every step ran.
	assert!(first.status.success(), "{}", stderr(&first));
	assert!(
		stdout(&first).contains("Demo data loaded"),
		"{}",
		stdout(&first)
	);
	let loaded = rows(&f).await;
	assert_eq!(loaded.loads, 1);
	assert_eq!(loaded.roots, 2);
	assert_eq!(
		loaded.tenants.keys().cloned().collect::<Vec<_>>(),
		["acme", "globex"]
	);
	let acme = &loaded.tenants["acme"];
	let globex = &loaded.tenants["globex"];
	let subjects = |rows: &Rows, tenant: &str| -> Vec<String> {
		rows.policies[tenant]
			.1
			.as_object()
			.unwrap()
			.keys()
			.cloned()
			.collect()
	};
	for fictional in ["morgan", "dana", "sam"] {
		assert!(subjects(&loaded, acme).contains(&fictional.to_owned()));
	}
	assert_eq!(
		loaded.policies[acme].1["dana"]["groups"],
		json!(["members"])
	);
	assert_eq!(
		loaded.policies[acme].1["morgan"]["groups"],
		json!(["administrators"])
	);
	for fictional in ["riley", "jordan"] {
		assert!(subjects(&loaded, globex).contains(&fictional.to_owned()));
	}
	// Approving an agent version made the agent a subject of the policy.
	let agent = aidash_server::domain::qualified_agent(NODE, "research-assistant", "1.0.0");
	assert!(loaded.policies[acme].1.get(&agent).is_some());
	assert_eq!(
		loaded.approved[acme],
		[
			"demo-gpt-4o-mini@1.0.0",
			"research-assistant@1.0.0",
			"summarizer@1.0.0",
			"weather-lookup@1.0.0"
		]
	);
	assert_eq!(
		loaded.approved[globex],
		["demo-gpt-4o-mini@1.0.0", "support-triage@1.0.0"]
	);
	assert_eq!(loaded.history, 6);
	// The Registry holds the entries, with the tool bound to this Node.
	let registry =
		aidash_server::apps::registry::repositories::Registry::new(f.pool().await, NODE).unwrap();
	let tenant = aidash_domain::tenancy::TenantName::new(acme.clone()).unwrap();
	let tool = registry
		.get(&tenant, "weather-lookup", "1.0.0")
		.await
		.unwrap();
	assert_eq!(tool.config["registry_node"], NODE);
	let listed = registry.list(&tenant, &Search::default()).await.unwrap();
	assert!(listed.iter().any(|entry| entry.id == "research-assistant"));

	// Act: the same inputs again.
	let again = command(&f, directory.path(), &original).await;
	// Assert: nothing changed, down to the policy revisions.
	assert!(again.status.success(), "{}", stderr(&again));
	assert!(
		stdout(&again).contains("Demo data unchanged"),
		"{}",
		stdout(&again)
	);
	assert_eq!(rows(&f).await, loaded);

	// Act: a changed structure adds a subject and tries to regroup another.
	let mut changed = structure();
	let subjects = changed["tenants"][0]["subjects"].as_array_mut().unwrap();
	subjects.push(json!({"id": "casey", "groups": ["members"]}));
	subjects[1]["groups"] = json!(["administrators"]);
	let changed = write(inputs.path(), "changed.json", &changed);
	let second = command(&f, directory.path(), &changed).await;
	// Assert: the new subject is added, the existing one is left as it was, and
	// everything else is untouched.
	assert!(second.status.success(), "{}", stderr(&second));
	assert!(
		stdout(&second).contains("Demo data loaded"),
		"{}",
		stdout(&second)
	);
	let extended = rows(&f).await;
	assert_eq!(extended.tenants, loaded.tenants);
	assert_eq!(extended.roots, 2);
	assert_eq!(extended.loads, 2);
	assert_eq!(extended.approved, loaded.approved);
	assert_eq!(extended.history, loaded.history);
	assert_eq!(extended.policies[acme].0, loaded.policies[acme].0 + 1);
	assert_eq!(
		extended.policies[acme].1["casey"]["groups"],
		json!(["members"])
	);
	assert_eq!(
		extended.policies[acme].1["dana"],
		loaded.policies[acme].1["dana"]
	);
	assert_eq!(extended.policies[globex], loaded.policies[globex]);

	// Act: an entry whose content changed under the same id and version.
	let mut edited = structure();
	edited["tenants"][0]["registry"][3]["config"]["instructions"] = json!("Say less.");
	let edited = write(inputs.path(), "edited.json", &edited);
	let third = command(&f, directory.path(), &edited).await;
	// Assert: the load fails loudly, names the entry and records nothing.
	assert!(!third.status.success());
	assert!(
		stderr(&third).contains("summarizer@1.0.0"),
		"{}",
		stderr(&third)
	);
	assert_eq!(rows(&f).await, extended);
}
