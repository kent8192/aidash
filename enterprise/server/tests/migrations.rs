// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! The Enterprise migrations extend Community's, and the models of both
//! workspaces agree with their migration state.
use std::fs;
use std::path::Path;
use std::process::Command;

fn copy(from: &Path, to: &Path) {
	fs::create_dir_all(to).unwrap();
	for entry in fs::read_dir(from).unwrap() {
		let entry = entry.unwrap();
		let target = to.join(entry.file_name());
		if entry.file_type().unwrap().is_dir() {
			copy(&entry.path(), &target);
		} else {
			fs::copy(entry.path(), target).unwrap();
		}
	}
}

#[test]
fn makemigrations_finds_no_change_in_either_workspace() {
	let build = &aidash_enterprise_server::BUILD;
	let community = Path::new(aidash_server::bootstrap::build::Build::COMMUNITY.manifest_dir);
	let project = tempfile::tempdir().unwrap();
	for root in build.migration_roots(community) {
		copy(&root, &project.path().join("migrations"));
	}
	assert!(
		project.path().join("migrations/gcip").is_dir(),
		"the Enterprise root is staged beside Community's apps"
	);
	fs::create_dir_all(project.path().join("src/bin")).unwrap();
	fs::copy(
		Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin/manage.rs"),
		project.path().join("src/bin/manage.rs"),
	)
	.unwrap();
	let settings = project.path().join("settings");
	fs::create_dir_all(&settings).unwrap();
	fs::write(
		settings.join("base.toml"),
		"[deployment]\noffering = \"cloud\"\n",
	)
	.unwrap();
	let output = Command::new(env!("CARGO_BIN_EXE_manage"))
		.args([
			"makemigrations",
			"--state-source",
			"files",
			"--dry-run",
			"--check",
		])
		.current_dir(project.path())
		.env_clear()
		.env("PATH", std::env::var_os("PATH").unwrap_or_default())
		.env("REINHARDT_ENV", "local")
		.env("REINHARDT_SETTINGS_DIR", &settings)
		.output()
		.unwrap();
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(output.status.success(), "{stdout}\n{stderr}");
	assert!(stdout.contains("No changes detected"), "{stdout}\n{stderr}");
}
