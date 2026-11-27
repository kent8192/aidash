use super::*;
use crate::agent_plugin::stdio_session::{
	DATA_INCOMPLETE, DATA_INVALID_PATH, DATA_TOO_LARGE, DEPENDENCIES_NOT_PINNED,
	MAX_PLUGIN_DATA_BYTES, check_data, launch, pack_data, plugin_data, plugin_root, unpack_data,
};
use rstest::rstest;
use std::collections::BTreeMap;
use uuid::Uuid;

const DIGEST: &str = "sha256:abc";

fn run() -> Uuid {
	Uuid::from_u128(7)
}

fn server(command: Command, args: &[&str], cwd: Cwd) -> StdioServer {
	StdioServer {
		command,
		args: args.iter().map(|arg| (*arg).to_owned()).collect(),
		env: BTreeMap::from([("CACHE".to_owned(), "${PLUGIN_DATA}/cache".to_owned())]),
		cwd,
	}
}

#[test]
fn a_plugin_file_starts_in_its_directory_with_placeholders_expanded_in_the_pod() {
	// Arrange
	let plugin = plugin_name("acme-tools");
	let config = server(
		Command::PluginRelative("bin/index.js".into()),
		&["--root", "${PLUGIN_ROOT}", "--data=${PLUGIN_DATA}"],
		Cwd::Data("work".into()),
	);

	// Act
	let launch = launch(DIGEST, &plugin, run(), &config).unwrap();

	// Assert
	let root = format!("/scratch/.aidash/plugins/{}/sha256-abc", run());
	let data = format!("/scratch/.aidash/plugin-data/{}/acme-tools", run());
	assert_eq!(
		launch.plugin_root,
		format!(".aidash/plugins/{}/sha256-abc", run())
	);
	assert_eq!(launch.plugin_data, plugin_data(run(), &plugin));
	assert_eq!(launch.executable.as_deref(), Some("bin/index.js"));
	assert_eq!(&launch.command[..2], ["/bin/sh", "-c"]);
	assert_eq!(
		&launch.command[3..],
		[
			"aidash-mcp".to_owned(),
			format!("{data}/work"),
			format!("{root}/bin/index.js"),
			"--root".to_owned(),
			root.clone(),
			format!("--data={data}"),
		]
	);
	assert_eq!(launch.env["CACHE"], format!("{data}/cache"));
}

#[rstest]
#[case::node(Executable::Node, Ok("node"))]
#[case::python(Executable::Python3, Ok("python3"))]
#[case::npx(Executable::Npx, Err(DEPENDENCIES_NOT_PINNED))]
#[case::npm(Executable::Npm, Err(DEPENDENCIES_NOT_PINNED))]
#[case::uvx(Executable::Uvx, Err(DEPENDENCIES_NOT_PINNED))]
#[case::uv(Executable::Uv, Err(DEPENDENCIES_NOT_PINNED))]
fn bare_commands_run_from_the_image_and_package_fetchers_wait_for_a_lockfile(
	#[case] executable: Executable,
	#[case] expected: Result<&str, &str>,
) {
	let config = server(Command::Bare(executable), &[], Cwd::PluginRoot);

	let launched = launch(DIGEST, &plugin_name("acme-tools"), run(), &config);

	match expected {
		Ok(program) => {
			let launched = launched.unwrap();
			assert_eq!(
				launched.command[4],
				format!("/scratch/.aidash/plugins/{}/sha256-abc", run())
			);
			assert_eq!(launched.command[5], program);
			assert_eq!(launched.executable, None);
		}
		Err(cause) => assert_eq!(launched.unwrap_err(), cause),
	}
}

#[test]
fn a_digest_names_a_single_directory() {
	let run = Uuid::from_u128(1);
	assert_eq!(
		plugin_root("sha256:a/b..c", run),
		format!(".aidash/plugins/{run}/sha256-a-b--c")
	);
}

#[rstest]
#[case::empty(vec![], false, Ok(()))]
#[case::fits(vec![("notes/a.txt", 10), ("b", MAX_PLUGIN_DATA_BYTES - 10)], false, Ok(()))]
#[case::over_the_cap(vec![("a", MAX_PLUGIN_DATA_BYTES), ("b", 1)], false, Err(DATA_TOO_LARGE))]
#[case::truncated_listing(vec![("a", 1)], true, Err(DATA_INCOMPLETE))]
#[case::absolute(vec![("/etc/passwd", 1)], false, Err(DATA_INVALID_PATH))]
#[case::climbing(vec![("a/../../b", 1)], false, Err(DATA_INVALID_PATH))]
#[case::dot(vec![("./a", 1)], false, Err(DATA_INVALID_PATH))]
fn saved_data_must_fit_the_cap_and_hold_plain_relative_paths(
	#[case] files: Vec<(&str, u64)>,
	#[case] truncated: bool,
	#[case] expected: Result<(), &str>,
) {
	assert_eq!(check_data(files, truncated), expected);
}

#[test]
fn saved_data_can_hold_more_than_four_thousand_empty_files() {
	let names: Vec<String> = (0..=4096).map(|n| n.to_string()).collect();

	let checked = check_data(names.iter().map(|name| (name.as_str(), 0)), false);

	assert_eq!(checked, Ok(()));
}

#[test]
fn packed_data_unpacks_to_the_same_files_and_refuses_damage() {
	let files = vec![
		PluginFile {
			path: "cache/state.json".into(),
			bytes: b"{}".to_vec(),
		},
		PluginFile {
			path: "empty".into(),
			bytes: Vec::new(),
		},
	];

	let packed = pack_data(&files);

	assert_eq!(unpack_data(&packed), Some(files));
	assert_eq!(unpack_data(&packed[..packed.len() - 1]), None);
	assert_eq!(
		unpack_data(&pack_data(&[PluginFile {
			path: "../escape".into(),
			bytes: vec![1],
		}])),
		None
	);
}
