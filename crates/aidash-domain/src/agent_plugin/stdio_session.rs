//! How a stdio MCP server of an Agent Plugin starts in the Execution Pod of its Run's Scratch
//! Space, and which `${PLUGIN_DATA}` content may replace the saved version (ADR 0073 and its
//! 2026-11-14 amendment). Pure: the application writes the files and talks to the Pod.
use super::{Command, Cwd, Executable, PluginFile, PluginName, StdioServer, expand};
use crate::registry::EntityRef;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Where the Scratch Space is mounted in the Pod.
pub const SCRATCH_ROOT: &str = "/scratch";
/// The plugin trees a Run's stdio servers start from, by copy digest.
pub const PLUGINS_DIR: &str = ".aidash/plugins";
/// The live `${PLUGIN_DATA}` areas, by Run and plugin.
pub const DATA_DIR: &str = ".aidash/plugin-data";
/// The cap on all of one plugin's saved data.
pub const MAX_PLUGIN_DATA_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_PLUGIN_DATA_FILES: u32 = 4096;
const MAX_DATA_PATH_BYTES: usize = 1024;

/// A command that downloads packages, while no lockfile was pinned when the plugin was attached.
/// Installing whatever is latest at session start is what ADR 0073 rejects.
pub const DEPENDENCIES_NOT_PINNED: &str = "dependencies_not_pinned";
pub const DATA_TOO_LARGE: &str = "plugin_data_too_large";
pub const DATA_TOO_MANY_FILES: &str = "plugin_data_too_many_files";
pub const DATA_INVALID_PATH: &str = "plugin_data_invalid_path";

/// Starts argv in the working directory given as `$1`. The directory is passed as an argument,
/// never spliced into the script, so no plugin text is parsed by the shell.
const IN_DIRECTORY: &str = r#"mkdir -p "$1" && cd "$1" || exit 127; shift; exec "$@""#;

/// The `mcp_stdio` group's operation. A stdio call needs `tool.invoke` on it, as a bound shell
/// call needs it on the shell's definition.
pub fn mcp_stdio_operation() -> EntityRef {
	EntityRef {
		id: "aidash.mcp_stdio".into(),
		version: "1.0.0".into(),
	}
}

/// The `mcp_stdio` group's bundle, whose Catalog Approval lets stdio servers run. It is read at
/// every session start and every call.
pub fn mcp_stdio_bundle() -> EntityRef {
	EntityRef {
		id: "aidash.bundle.mcp_stdio".into(),
		version: "1.0.0".into(),
	}
}

/// What the Runner's `mcp/start` needs from one stdio server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
	/// Wrapped so the process starts in the server's working directory.
	pub command: Vec<String>,
	pub env: BTreeMap<String, String>,
	/// Relative to the Scratch Space root.
	pub plugin_root: String,
	/// Relative to the Scratch Space root.
	pub plugin_data: String,
	/// The plugin file the server runs, which must be executable.
	pub executable: Option<String>,
}

/// The tree of a plugin copy in one Run, relative to the Scratch Space root. Runs share the
/// Execution Pod, so each has its own copy: preparing one never removes another's.
pub fn plugin_root(digest: &str, run: Uuid) -> String {
	let name: String = digest
		.chars()
		.map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
		.collect();
	format!("{}/{name}", run_plugins(run))
}

/// Every plugin tree of a Run, removed when its sessions close.
pub fn run_plugins(run: Uuid) -> String {
	format!("{PLUGINS_DIR}/{run}")
}

/// One plugin's live data area in a Run, relative to the Scratch Space root. Every stdio server
/// of the plugin in the Run shares it.
pub fn plugin_data(run: Uuid, plugin: &PluginName) -> String {
	format!("{DATA_DIR}/{run}/{}", plugin.as_str())
}

/// A path relative to the Scratch Space root, as processes in the Pod see it.
pub fn in_pod(relative: &str) -> String {
	format!("{SCRATCH_ROOT}/{relative}")
}

/// How `server` of the plugin copy `digest` starts for `run`. Refuses a package fetcher.
pub fn launch(
	digest: &str,
	plugin: &PluginName,
	run: Uuid,
	server: &StdioServer,
) -> Result<Launch, &'static str> {
	let root = plugin_root(digest, run);
	let data = plugin_data(run, plugin);
	let (pod_root, pod_data) = (in_pod(&root), in_pod(&data));
	let (program, executable) = match &server.command {
		Command::Bare(Executable::Node) => ("node".to_owned(), None),
		Command::Bare(Executable::Python3) => ("python3".to_owned(), None),
		Command::Bare(Executable::Npm | Executable::Npx | Executable::Uv | Executable::Uvx) => {
			return Err(DEPENDENCIES_NOT_PINNED);
		}
		Command::PluginRelative(path) => (format!("{pod_root}/{path}"), Some(path.clone())),
	};
	let directory = match &server.cwd {
		Cwd::PluginRoot => pod_root.clone(),
		Cwd::Plugin(path) => format!("{pod_root}/{path}"),
		Cwd::Data(path) if path.is_empty() => pod_data.clone(),
		Cwd::Data(path) => format!("{pod_data}/{path}"),
	};
	let mut command = vec![
		"/bin/sh".to_owned(),
		"-c".to_owned(),
		IN_DIRECTORY.to_owned(),
		"aidash-mcp".to_owned(),
		directory,
		program,
	];
	command.extend(
		server
			.args
			.iter()
			.map(|arg| expand(arg, &pod_root, &pod_data)),
	);
	let env = server
		.env
		.iter()
		.map(|(name, value)| (name.clone(), expand(value, &pod_root, &pod_data)))
		.collect();
	Ok(Launch {
		command,
		env,
		plugin_root: root,
		plugin_data: data,
		executable,
	})
}

/// Whether a plugin's live data, as listed from the Pod, may replace its saved version: within
/// the cap, completely listed, and every path a plain relative path. A refusal keeps the
/// previous version.
pub fn check_data<'a>(
	files: impl IntoIterator<Item = (&'a str, u64)>,
	truncated: bool,
) -> Result<(), &'static str> {
	if truncated {
		return Err(DATA_TOO_MANY_FILES);
	}
	let (mut count, mut total) = (0u32, 0u64);
	for (path, size) in files {
		count += 1;
		total = total.saturating_add(size);
		if count > MAX_PLUGIN_DATA_FILES {
			return Err(DATA_TOO_MANY_FILES);
		}
		if total > MAX_PLUGIN_DATA_BYTES {
			return Err(DATA_TOO_LARGE);
		}
		if !data_path(path) {
			return Err(DATA_INVALID_PATH);
		}
	}
	Ok(())
}

fn data_path(path: &str) -> bool {
	!path.is_empty()
		&& path.len() <= MAX_DATA_PATH_BYTES
		&& !path.contains('\\')
		&& !path.chars().any(char::is_control)
		&& path
			.split('/')
			.all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

/// Packs checked plugin data into one value to save: per file a big-endian `u32` path length,
/// the path, a big-endian `u64` length and the bytes.
pub fn pack_data(files: &[PluginFile]) -> Vec<u8> {
	let mut packed = Vec::new();
	for file in files {
		packed.extend_from_slice(&(file.path.len() as u32).to_be_bytes());
		packed.extend_from_slice(file.path.as_bytes());
		packed.extend_from_slice(&(file.bytes.len() as u64).to_be_bytes());
		packed.extend_from_slice(&file.bytes);
	}
	packed
}

/// The files [`pack_data`] packed, if `packed` is well formed and they pass [`check_data`].
pub fn unpack_data(packed: &[u8]) -> Option<Vec<PluginFile>> {
	let mut files = Vec::new();
	let mut rest = packed;
	while !rest.is_empty() {
		let (length, tail) = rest.split_at_checked(4)?;
		let length = u32::from_be_bytes(length.try_into().ok()?) as usize;
		let (path, tail) = tail.split_at_checked(length)?;
		let (size, tail) = tail.split_at_checked(8)?;
		let size = usize::try_from(u64::from_be_bytes(size.try_into().ok()?)).ok()?;
		let (bytes, tail) = tail.split_at_checked(size)?;
		files.push(PluginFile {
			path: String::from_utf8(path.to_vec()).ok()?,
			bytes: bytes.to_vec(),
		});
		rest = tail;
	}
	check_data(
		files
			.iter()
			.map(|file| (file.path.as_str(), file.bytes.len() as u64)),
		false,
	)
	.ok()?;
	Some(files)
}
