//! Use the shipped settings and migrations from an isolated deployment directory.
use std::{fs, path::Path};
use tokio::process::Command;

pub fn deployment_command(database_url: &str, directory: &Path) -> Command {
	let server = Path::new(env!("CARGO_MANIFEST_DIR"));
	let settings = directory.join("settings");
	fs::create_dir_all(&settings).unwrap();
	let mut defaults: toml::Value =
		toml::from_str(&fs::read_to_string(server.join("settings/base.example.toml")).unwrap())
			.unwrap();
	defaults["core"].as_table_mut().unwrap().insert(
		"base_dir".into(),
		toml::Value::String(directory.to_string_lossy().into_owned()),
	);
	fs::write(
		settings.join("base.toml"),
		toml::to_string(&defaults).unwrap(),
	)
	.unwrap();
	for app in fs::read_dir(server.join("migrations")).unwrap() {
		let app = app.unwrap();
		if !app.file_type().unwrap().is_dir() {
			continue;
		}
		let target = directory.join("migrations").join(app.file_name());
		fs::create_dir_all(&target).unwrap();
		for migration in fs::read_dir(app.path()).unwrap() {
			let migration = migration.unwrap();
			if migration.path().extension().is_some_and(|ext| ext == "rs") {
				fs::copy(migration.path(), target.join(migration.file_name())).unwrap();
			} else if migration.file_name() == "sql" && migration.path().is_dir() {
				let sql_target = target.join("sql");
				copy_assets(&migration.path(), &sql_target);
			}
		}
	}
	let mut command = Command::new(env!("CARGO_BIN_EXE_manage"));
	command
		.env_clear()
		.env("PATH", std::env::var_os("PATH").unwrap_or_default())
		.env("REINHARDT_SETTINGS_DIR", settings)
		.env("REINHARDT_ENV", "container")
		.env(
			"REINHARDT_SECRET_KEY",
			"deployment-fixture-key-with-no-production-use",
		)
		.env("AIDASH_BASE_DIR", directory)
		.env("DATABASE_URL", database_url)
		.env("AIDASH_NODE_ID", "aidash://manage-fixture")
		.env("AIDASH_ENDPOINT", "http://127.0.0.1:0")
		.env("AIDASH_API_TOKEN", "native-command-test-operator")
		.env("NATS_URL", "nats://127.0.0.1:0")
		.env("AIDASH_BACKGROUND_ENABLED", "false")
		.env("AIDASH_WORKER_COUNT", "1")
		.env("RUST_BACKTRACE", "0")
		.current_dir(directory)
		.kill_on_drop(true);
	command
}

fn copy_assets(source: &Path, target: &Path) {
	fs::create_dir_all(target).unwrap();
	for asset in fs::read_dir(source).unwrap() {
		let asset = asset.unwrap();
		let destination = target.join(asset.file_name());
		if asset.path().is_dir() {
			copy_assets(&asset.path(), &destination);
		} else {
			fs::copy(asset.path(), destination).unwrap();
		}
	}
}
