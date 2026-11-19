//! Router and Watcher scripts of a Duty (#1011, ADR 0142): deterministic
//! Python or shell a Duty runs without a model, so cheap filtering never wakes
//! one. A Router decides what an event does; a Watcher polls an outside
//! service on its own interval and reports events.
//!
//! A script needs no approval of its own. It runs as a Run of the Duty's Agent
//! for its Requester, so Egress, External services, cancel and purge apply as
//! for any Run, but the harness makes the Run's tool calls itself (see
//! [`plan`]) through the bound `shell` or `code_interpreter` tool, which the
//! Catalog must approve for the Duty's Node.
use super::{MAX_INTERVAL_SECONDS, NoTrigger, TITLE_MAX};
use crate::{Error, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// The largest script source, in bytes.
pub const MAX_SOURCE_BYTES: usize = 64 * 1024;
/// The shortest gap between two runs of a Watcher. A run is a Pod command and
/// a Task, and Budgets count only model spend, so a floor keeps a typo from
/// running one every second.
pub const MIN_WATCHER_SECONDS: i64 = 60;
/// The most events one Watcher run reports; the rest are dropped with a
/// recorded reason.
pub const MAX_WATCHER_EVENTS: usize = 32;
/// The longest description or input a Router hands on, in characters.
pub const MAX_ROUTED_TEXT: usize = 16_000;
/// The ids of the calls a script Run makes start with this, so its pending
/// state tells a script response from a model's.
pub const CALL_PREFIX: &str = "duty-script:";
/// The most output the harness reads from a script, in bytes: the Runner's
/// own output cap.
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// Printed before the script's stdout, so whatever ran before it (a Python
/// session notice, the file writes) is never taken for its output.
const OUTPUT_MARK: &str = "\u{1e}aidash-duty-script-output\n";
/// Printed after the stdout of a script that exited unsuccessfully, followed
/// by its exit code and the tail of its stderr.
const ERROR_MARK: &str = "\u{1e}aidash-duty-script-error ";
/// Base64 characters written per call; a multiple of 4, so each chunk decodes
/// on its own.
const CHUNK: usize = 32 * 1024;
/// Commands are merged into one call while they fit this, below the Runner's
/// 64 KiB command limit.
const MERGE_LIMIT: usize = 60 * 1024;
/// The stderr a failed script shows, in bytes.
const STDERR_TAIL: usize = 2_000;

/// The language a script is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Language {
	Python,
	Shell,
}

impl Language {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Python => "python",
			Self::Shell => "shell",
		}
	}

	pub fn parse(value: &str) -> Result<Self> {
		match value {
			"python" => Ok(Self::Python),
			"shell" => Ok(Self::Shell),
			other => Err(Error::Invalid(format!("unknown script language {other}"))),
		}
	}

	/// The core operation that runs it, bound to the Duty's Agent.
	pub fn operation(self) -> &'static str {
		match self {
			Self::Python => "code_interpreter",
			Self::Shell => "shell",
		}
	}

	/// The core operation that reads a running call's output.
	pub fn poll_operation(self) -> &'static str {
		match self {
			Self::Python => "python_poll",
			Self::Shell => "shell_poll",
		}
	}
}

/// A Router's or Watcher's code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Script {
	pub language: Language,
	/// At most 64 KiB, without NUL.
	pub source: String,
}

impl Script {
	pub fn validate(&self) -> Result<()> {
		if self.source.trim().is_empty() {
			return Err(Error::Invalid("a Duty script needs source code".into()));
		}
		if self.source.len() > MAX_SOURCE_BYTES || self.source.contains('\0') {
			return Err(Error::Invalid(format!(
				"a Duty script may have at most {MAX_SOURCE_BYTES} bytes and no NUL"
			)));
		}
		Ok(())
	}
}

/// A script that polls an outside service on its own interval, independent of
/// the Duty's time trigger. Its stdout is a JSON array of events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Watcher {
	pub script: Script,
	/// Seconds between two runs: one minute to one year.
	pub every_seconds: i64,
}

impl Watcher {
	pub fn validate(&self) -> Result<()> {
		self.script.validate()?;
		if !(MIN_WATCHER_SECONDS..=MAX_INTERVAL_SECONDS).contains(&self.every_seconds) {
			return Err(Error::Invalid(format!(
				"a Watcher runs every {MIN_WATCHER_SECONDS} seconds to one year"
			)));
		}
		Ok(())
	}
}

/// A new Router, or `{"kind": "none"}` to remove it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ScriptChoice {
	Set(Script),
	Remove(NoTrigger),
}

/// A new Watcher, or `{"kind": "none"}` to remove it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum WatcherChoice {
	Set(Watcher),
	Remove(NoTrigger),
}

/// Which script of a Duty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
	Router,
	Watcher,
}

impl Role {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Router => "router",
			Self::Watcher => "watcher",
		}
	}

	pub fn parse(value: &str) -> Result<Self> {
		match value {
			"router" => Ok(Self::Router),
			"watcher" => Ok(Self::Watcher),
			other => Err(Error::Invalid(format!("unknown Duty script role {other}"))),
		}
	}
}

/// One saved version of a Duty's script: who saved it and when (#1011).
/// Only shown, never read back: its flattened [`Script`] refuses unknown
/// fields, which `flatten` cannot deserialize.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Version {
	pub id: Uuid,
	pub duty_id: Uuid,
	pub role: Role,
	#[serde(flatten)]
	pub script: Script,
	/// The Watcher's interval when this version was saved.
	pub every_seconds: Option<i64>,
	pub author: String,
	pub created_at: chrono::DateTime<chrono::Utc>,
}

/// What one script Run executes: the frozen script and what it reads on stdin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invocation {
	pub language: Language,
	pub source: String,
	pub stdin: String,
}

/// An open Task of the Duty, as a Router sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenTask {
	pub task_id: Uuid,
	pub title: String,
}

/// What a Router reads on stdin.
pub fn router_stdin(
	event: &Value,
	trigger: (Uuid, &str),
	open_tasks: &[OpenTask],
) -> Result<String> {
	Ok(serde_json::to_string(&serde_json::json!({
		"event": event,
		"trigger": {"id": trigger.0, "kind": trigger.1},
		"open_tasks": open_tasks,
	}))?)
}

/// What a Router decided about one event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum RouterAction {
	/// Nothing more happens. (Struct variants, so `deny_unknown_fields`
	/// refuses extra fields beside the tag.)
	Drop {},
	/// The script did the work itself.
	Act {},
	/// Start a model Task in the Duty's Thread. The Task keeps the Duty's
	/// own title; the Router's title and description reach the Agent only
	/// as data, beside the event they were computed from.
	Task {
		title: String,
		#[serde(default)]
		description: String,
	},
	/// Hand the input to an open Task of the Duty and wake it.
	Deliver { task_id: Uuid, input: String },
}

impl RouterAction {
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::Drop {} => "drop",
			Self::Act {} => "act",
			Self::Task { .. } => "task",
			Self::Deliver { .. } => "deliver",
		}
	}
}

/// Cuts `text` to at most `limit` characters for a recorded reason.
fn short(text: &str, limit: usize) -> String {
	let text = text.trim();
	match text.char_indices().nth(limit) {
		Some((end, _)) => format!("{}…", &text[..end]),
		None => text.to_owned(),
	}
}

/// Reads a Router's stdout: exactly one JSON object naming its action.
/// Anything else is refused with a short reason the event records.
pub fn parse_router(stdout: &str) -> std::result::Result<RouterAction, String> {
	let text = stdout.trim();
	if text.is_empty() {
		return Err("the Router printed nothing; it must print one JSON object".into());
	}
	let action: RouterAction = serde_json::from_str(text).map_err(|error| {
		format!(
			"the Router's output is not a valid action: {}",
			short(&error.to_string(), 200)
		)
	})?;
	match &action {
		RouterAction::Drop {} | RouterAction::Act {} => {}
		RouterAction::Task { title, description } => {
			let title = title.trim();
			if title.is_empty()
				|| title.chars().count() > TITLE_MAX
				|| title.chars().any(char::is_control)
			{
				return Err(format!(
					"a Router's Task title needs 1 to {TITLE_MAX} characters without control characters"
				));
			}
			if description.chars().count() > MAX_ROUTED_TEXT || description.contains('\0') {
				return Err(format!(
					"a Router's Task description may have at most {MAX_ROUTED_TEXT} characters and no NUL"
				));
			}
		}
		RouterAction::Deliver { input, .. } => {
			if input.trim().is_empty()
				|| input.chars().count() > MAX_ROUTED_TEXT
				|| input.contains('\0')
			{
				return Err(format!(
					"a Router's input needs 1 to {MAX_ROUTED_TEXT} characters without NUL"
				));
			}
		}
	}
	Ok(action)
}

/// The events one Watcher run reported, and why any were dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatcherOutput {
	pub events: Vec<Value>,
	/// Why entries were dropped, if any were.
	pub dropped: Option<String>,
}

/// Reads a Watcher's stdout: a JSON array of event objects. Nothing printed
/// means no events. At most [`MAX_WATCHER_EVENTS`] are kept, each at most
/// [`super::events::MAX_EVENT_BYTES`] serialized.
pub fn parse_watcher(stdout: &str) -> std::result::Result<WatcherOutput, String> {
	let text = stdout.trim();
	if text.is_empty() {
		return Ok(WatcherOutput {
			events: vec![],
			dropped: None,
		});
	}
	let entries: Vec<Value> = serde_json::from_str(text).map_err(|error| {
		format!(
			"the Watcher's output is not a JSON array: {}",
			short(&error.to_string(), 200)
		)
	})?;
	let (mut past_limit, mut oversized, mut not_objects) = (0_usize, 0_usize, 0_usize);
	let mut events = Vec::new();
	for entry in entries {
		if !entry.is_object() {
			not_objects += 1;
		} else if serde_json::to_string(&entry).map_or(usize::MAX, |text| text.len())
			> super::events::MAX_EVENT_BYTES
		{
			oversized += 1;
		} else if events.len() >= MAX_WATCHER_EVENTS {
			past_limit += 1;
		} else {
			events.push(entry);
		}
	}
	let mut reasons = Vec::new();
	if past_limit > 0 {
		reasons.push(format!(
			"{past_limit} past the {MAX_WATCHER_EVENTS}-event limit"
		));
	}
	if oversized > 0 {
		reasons.push(format!(
			"{oversized} over {} bytes",
			super::events::MAX_EVENT_BYTES
		));
	}
	if not_objects > 0 {
		reasons.push(format!("{not_objects} not JSON objects"));
	}
	Ok(WatcherOutput {
		events,
		dropped: (!reasons.is_empty())
			.then(|| format!("Watcher entries dropped: {}", reasons.join(", "))),
	})
}

/// The idempotency key of a script Run's `index`th call, stable across retries.
pub fn call_key(run: Uuid, index: usize) -> Uuid {
	use sha2::{Digest, Sha256};
	let digest = Sha256::digest(format!("aidash-duty-script/v1:{run}:{index}"));
	let mut bytes = [0_u8; 16];
	bytes.copy_from_slice(&digest[..16]);
	uuid::Builder::from_random_bytes(bytes).into_uuid()
}

/// The Scratch Space directory a script Run writes its files in.
pub fn directory(task: Uuid) -> String {
	format!(".aidash/duty-scripts/{task}")
}

/// The commands a script Run makes, in order, through the tool that runs its
/// language. The Runner takes at most 64 KiB per command, and the source alone
/// may be that long, so the script and its stdin are first written into the
/// Scratch Space as base64 chunks; the last command runs the script with that
/// stdin, prints [`OUTPUT_MARK`] before its stdout and removes the files.
/// Commands are merged while they fit one call.
pub fn plan(invocation: &Invocation, directory: &str) -> Vec<String> {
	let language = invocation.language;
	let mut commands = vec![prepare(language, directory)];
	for (name, content) in [
		("script", invocation.source.as_bytes()),
		("stdin", invocation.stdin.as_bytes()),
	] {
		let encoded = STANDARD.encode(content);
		let chunks: Vec<&str> = if encoded.is_empty() {
			vec![""]
		} else {
			encoded
				.as_bytes()
				.chunks(CHUNK)
				.map(|chunk| std::str::from_utf8(chunk).expect("base64 is ASCII"))
				.collect()
		};
		for (index, chunk) in chunks.into_iter().enumerate() {
			commands.push(write(language, directory, name, chunk, index == 0));
		}
	}
	commands.push(run(language, directory));
	let mut merged: Vec<String> = Vec::new();
	for command in commands {
		match merged.last_mut() {
			Some(last) if last.len() + 1 + command.len() <= MERGE_LIMIT => {
				last.push('\n');
				last.push_str(&command);
			}
			_ => merged.push(command),
		}
	}
	merged
}

fn prepare(language: Language, directory: &str) -> String {
	match language {
		Language::Shell => format!("set -e\nmkdir -p '{directory}'"),
		Language::Python => format!("import os\nos.makedirs(\"{directory}\", exist_ok=True)"),
	}
}

fn write(language: Language, directory: &str, name: &str, chunk: &str, first: bool) -> String {
	match language {
		Language::Shell => format!(
			"printf '%s' '{chunk}' | base64 -d {} '{directory}/{name}'",
			if first { ">" } else { ">>" }
		),
		Language::Python => format!(
			"import base64\nwith open(\"{directory}/{name}\", \"{}\") as _aidash_file:\n    _aidash_file.write(base64.b64decode(\"{chunk}\"))",
			if first { "wb" } else { "ab" }
		),
	}
}

fn run(language: Language, directory: &str) -> String {
	match language {
		Language::Shell => format!(
			"set +e\n\
			 _aidash_dir='{directory}'\n\
			 printf '\\036aidash-duty-script-output\\n'\n\
			 if command -v bash >/dev/null 2>&1; then bash \"$_aidash_dir/script\" < \"$_aidash_dir/stdin\" 2> \"$_aidash_dir/stderr\"; \
			 else sh \"$_aidash_dir/script\" < \"$_aidash_dir/stdin\" 2> \"$_aidash_dir/stderr\"; fi\n\
			 _aidash_code=$?\n\
			 if [ \"$_aidash_code\" -ne 0 ]; then printf '\\n\\036aidash-duty-script-error %s\\n' \"$_aidash_code\"; \
			 tail -c {STDERR_TAIL} \"$_aidash_dir/stderr\"; fi\n\
			 rm -rf \"$_aidash_dir\"\n\
			 exit \"$_aidash_code\""
		),
		Language::Python => format!(
			"import shutil, subprocess, sys\n\
			 _aidash_dir = \"{directory}\"\n\
			 try:\n\
			 \x20   with open(_aidash_dir + \"/stdin\", \"rb\") as _aidash_in, open(_aidash_dir + \"/stderr\", \"wb\") as _aidash_err:\n\
			 \x20       _aidash_run = subprocess.run([sys.executable, \"-I\", _aidash_dir + \"/script\"], stdin=_aidash_in, stdout=subprocess.PIPE, stderr=_aidash_err)\n\
			 \x20   sys.stdout.write(\"\\x1eaidash-duty-script-output\\n\")\n\
			 \x20   sys.stdout.write(_aidash_run.stdout.decode(\"utf-8\", \"replace\"))\n\
			 \x20   if _aidash_run.returncode != 0:\n\
			 \x20       with open(_aidash_dir + \"/stderr\", \"rb\") as _aidash_err:\n\
			 \x20           _aidash_tail = _aidash_err.read()[-{STDERR_TAIL}:].decode(\"utf-8\", \"replace\")\n\
			 \x20       sys.stdout.write(\"\\n\\x1eaidash-duty-script-error %d\\n%s\" % (_aidash_run.returncode, _aidash_tail))\n\
			 finally:\n\
			 \x20   shutil.rmtree(_aidash_dir, ignore_errors=True)"
		),
	}
}

/// The script's own stdout in the last call's output, or why it failed: it
/// exited unsuccessfully (its exit code and stderr tail), or it never ran.
pub fn script_output(output: &str) -> std::result::Result<String, String> {
	let Some(start) = output.find(OUTPUT_MARK) else {
		return Err(format!("the script did not run: {}", short(output, 500)));
	};
	let body = &output[start + OUTPUT_MARK.len()..];
	match body.rfind(ERROR_MARK) {
		Some(end) => {
			let detail = body[end + ERROR_MARK.len()..].trim();
			let (code, stderr) = detail.split_once('\n').unwrap_or((detail, ""));
			let stderr = stderr.trim();
			Err(if stderr.is_empty() {
				format!("the script exited with code {}", code.trim())
			} else {
				format!(
					"the script exited with code {}: {}",
					code.trim(),
					short(stderr, 500)
				)
			})
		}
		None => Ok(body.to_owned()),
	}
}

#[cfg(test)]
mod tests;
