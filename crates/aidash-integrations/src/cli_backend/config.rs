//! Strict literal env files. Values are never evaluated, expanded or logged.
use super::{Failure, Result};
use std::{
	collections::BTreeMap,
	fs::OpenOptions,
	io::Write,
	path::{Path, PathBuf},
};

pub const KEYS: &[&str] = &[
	"AIDASH_CLI_PROVIDER",
	"AIDASH_CLI_AUTH_MODE",
	"AIDASH_CLI_EXECUTABLE",
	"AIDASH_CLI_MODEL",
	"AIDASH_CLI_PORT",
	"AIDASH_CLI_TIMEOUT_SECONDS",
	"AIDASH_CLI_MAX_OUTPUT_TOKENS",
	"AIDASH_CLI_WORK_DIR",
	"AIDASH_CLI_CONTEXT_WINDOW",
	"CLAUDE_CONFIG_DIR",
	"CLAUDE_CODE_OAUTH_TOKEN",
	"CODEX_HOME",
	"CODEX_ACCESS_TOKEN",
	"CODEX_API_KEY",
];
const SECRETS: &[&str] = &[
	"CODEX_ACCESS_TOKEN",
	"CODEX_API_KEY",
	"CLAUDE_CODE_OAUTH_TOKEN",
];
pub type Values = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
	Claude,
	Codex,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
	Native,
	AccessToken,
	AccessTokenLogin,
	ApiKey,
	ApiKeyLogin,
}

// Deliberately no Debug: settings contain credentials.
pub struct Config {
	pub provider: Provider,
	pub auth: Auth,
	pub executable: PathBuf,
	pub model: String,
	pub port: u16,
	pub timeout_seconds: u64,
	pub max_output_tokens: u32,
	pub context_window: usize,
	pub work_dir: PathBuf,
	values: Values,
}

pub fn parse_env(text: &str) -> Result<Values> {
	if text.len() > 65536 {
		return Err(Failure("env_file_too_large"));
	}
	let mut out = Values::new();
	for line in text.lines() {
		let line = line.trim();
		if line.is_empty() || line.starts_with('#') {
			continue;
		}
		let (key, raw) = line
			.split_once('=')
			.ok_or(Failure("invalid_env_assignment"))?;
		let key = key.trim();
		if !KEYS.contains(&key) || out.contains_key(key) {
			return Err(Failure("unknown_or_duplicate_env_key"));
		}
		let raw = raw.trim();
		let value = if raw.starts_with(['\'', '"']) {
			let quote = raw.as_bytes()[0] as char;
			let end = raw[1..]
				.find(quote)
				.map(|n| n + 1)
				.ok_or(Failure("unclosed_env_quote"))?;
			let tail = raw[end + 1..].trim();
			if !tail.is_empty() && !tail.starts_with('#') {
				return Err(Failure("invalid_env_quote_tail"));
			}
			&raw[1..end]
		} else {
			raw.split_once(" #")
				.map_or(raw, |(value, _)| value.trim_end())
		};
		if value.contains(['$', '`', '\\']) || value.chars().any(char::is_control) {
			return Err(Failure("env_expansion_or_escape_not_supported"));
		}
		out.insert(key.into(), value.into());
	}
	Ok(out)
}

pub fn load(path: &Path, ambient: &Values, overrides: &Values) -> Result<Config> {
	let text = std::fs::read_to_string(path).map_err(|_| Failure("env_file_unreadable"))?;
	let mut values = parse_env(&text)?;
	for (key, value) in ambient {
		if key.starts_with("AIDASH_CLI_") && !KEYS.contains(&key.as_str()) {
			return Err(Failure("unknown_cli_environment_key"));
		}
		if KEYS.contains(&key.as_str()) {
			values.insert(key.clone(), value.clone());
		}
	}
	for (key, value) in overrides {
		if !KEYS.contains(&key.as_str()) || SECRETS.contains(&key.as_str()) {
			return Err(Failure("invalid_cli_override"));
		}
		values.insert(key.clone(), value.clone());
	}
	Config::from_values(values)
}

fn required(values: &Values, key: &str) -> Result<String> {
	let value = values
		.get(key)
		.filter(|v| !v.trim().is_empty())
		.ok_or(Failure("required_setting_empty"))?;
	if value != value.trim()
		|| value.contains(['<', '>', '$', '`', '\n', '\r'])
		|| ["REPLACE", "YOUR_", "CHANGEME", "__MODEL"]
			.iter()
			.any(|s| value.to_uppercase().contains(s))
	{
		return Err(Failure("placeholder_or_invalid_setting"));
	}
	Ok(value.clone())
}

impl Config {
	pub fn from_values(values: Values) -> Result<Self> {
		if values.keys().any(|key| !KEYS.contains(&key.as_str())) {
			return Err(Failure("unknown_env_key"));
		}
		let provider = match required(&values, "AIDASH_CLI_PROVIDER")?.as_str() {
			"claude" => Provider::Claude,
			"codex" => Provider::Codex,
			_ => return Err(Failure("invalid_provider")),
		};
		let auth = match required(&values, "AIDASH_CLI_AUTH_MODE")?.as_str() {
			"native" => Auth::Native,
			"access-token" => Auth::AccessToken,
			"access-token-login" => Auth::AccessTokenLogin,
			"api-key" => Auth::ApiKey,
			"api-key-login" => Auth::ApiKeyLogin,
			_ => return Err(Failure("invalid_auth_mode")),
		};
		if provider == Provider::Claude && auth != Auth::Native {
			return Err(Failure("invalid_provider_auth_combination"));
		}
		if provider == Provider::Claude
			&& values
				.get("CLAUDE_CODE_OAUTH_TOKEN")
				.is_some_and(|v| !v.is_empty())
		{
			// This profile verifies a saved personal login, not an environment
			// token's account. Never silently test a different saved account.
			return Err(Failure("claude_oauth_token_backend_unsupported"));
		}
		let selected = match auth {
			Auth::AccessToken => Some("CODEX_ACCESS_TOKEN"),
			Auth::ApiKey => Some("CODEX_API_KEY"),
			_ => None,
		};
		for key in SECRETS {
			if Some(*key) == selected {
				required(&values, key)?;
			} else if values.get(*key).is_some_and(|v| !v.is_empty()) {
				return Err(Failure("conflicting_auth_environment"));
			}
		}
		let foreign = if provider == Provider::Claude {
			"CODEX_HOME"
		} else {
			"CLAUDE_CONFIG_DIR"
		};
		if values.get(foreign).is_some_and(|v| !v.is_empty()) {
			return Err(Failure("foreign_provider_setting"));
		}
		let model = required(&values, "AIDASH_CLI_MODEL")?;
		if model.len() > 160
			|| !model
				.bytes()
				.all(|c| c.is_ascii_alphanumeric() || b"-._/".contains(&c))
			|| ["default", "latest", "sonnet", "opus", "haiku", "fable"].contains(&model.as_str())
		{
			return Err(Failure("explicit_model_id_required"));
		}
		let number = |key: &str, default: u64, min: u64, max: u64| -> Result<u64> {
			let n = match values.get(key) {
				Some(v) => v
					.parse::<u64>()
					.map_err(|_| Failure("invalid_numeric_setting"))?,
				None => default,
			};
			if !(min..=max).contains(&n) {
				return Err(Failure("numeric_setting_out_of_range"));
			}
			Ok(n)
		};
		let executable = values
			.get("AIDASH_CLI_EXECUTABLE")
			.cloned()
			.unwrap_or_else(|| {
				if provider == Provider::Claude {
					"claude"
				} else {
					"codex"
				}
				.into()
			});
		if executable.is_empty() || executable.contains(['$', '`', '\n', '\r', '~']) {
			return Err(Failure("invalid_executable"));
		}
		let executable = if executable.contains('/') && !Path::new(&executable).is_absolute() {
			std::env::current_dir()
				.map_err(|_| Failure("working_directory_unavailable"))?
				.join(executable)
		} else {
			PathBuf::from(executable)
		};
		for key in ["AIDASH_CLI_WORK_DIR", "CODEX_HOME", "CLAUDE_CONFIG_DIR"] {
			if let Some(path) = values.get(key)
				&& (path.is_empty()
					|| path.contains(['$', '`', '~'])
					|| key != "AIDASH_CLI_WORK_DIR" && !Path::new(path).is_absolute())
			{
				return Err(Failure("invalid_path_setting"));
			}
		}
		let max_output_tokens = number("AIDASH_CLI_MAX_OUTPUT_TOKENS", 4096, 1, 32768)? as u32;
		let context_window = number("AIDASH_CLI_CONTEXT_WINDOW", 0, 2048, 2_000_000)? as usize;
		if context_window <= max_output_tokens as usize {
			return Err(Failure("context_window_too_small"));
		}
		Ok(Self {
			provider,
			auth,
			executable,
			model,
			port: number("AIDASH_CLI_PORT", 18190, 1024, 65535)? as u16,
			timeout_seconds: number("AIDASH_CLI_TIMEOUT_SECONDS", 120, 1, 900)?,
			max_output_tokens,
			context_window,
			work_dir: values
				.get("AIDASH_CLI_WORK_DIR")
				.map(PathBuf::from)
				.unwrap_or_else(|| PathBuf::from("private/cli-backends/work")),
			values,
		})
	}

	pub fn child_env(&self, ambient: &Values) -> Values {
		let mut env = Values::new();
		// No proxy, SDK, shell-startup, provider, instrumentation or agent variables.
		for key in [
			"HOME",
			"PATH",
			"USER",
			"LOGNAME",
			"LANG",
			"TMPDIR",
			"XDG_RUNTIME_DIR",
			"DBUS_SESSION_BUS_ADDRESS",
		] {
			if let Some(value) = ambient.get(key) {
				env.insert(key.into(), value.clone());
			}
		}
		let keys: &[&str] = match self.provider {
			Provider::Claude => &["CLAUDE_CONFIG_DIR"],
			Provider::Codex => &["CODEX_HOME"],
		};
		for key in keys {
			if let Some(value) = self.values.get(*key) {
				env.insert((*key).into(), value.clone());
			}
		}
		let secret = match self.auth {
			Auth::AccessToken => Some("CODEX_ACCESS_TOKEN"),
			Auth::ApiKey => Some("CODEX_API_KEY"),
			_ => None,
		};
		if let Some(key) = secret {
			env.insert(key.into(), self.values[key].clone());
		}
		env
	}

	pub fn diagnostic(&self) -> serde_json::Value {
		serde_json::json!({"provider":format!("{:?}",self.provider),"auth_mode":format!("{:?}",self.auth),
			"model":self.model,"origin":format!("http://127.0.0.1:{}",self.port),
			"timeout_seconds":self.timeout_seconds,"max_output_tokens":self.max_output_tokens,
			"context_window":self.context_window,"secret_required":matches!(self.auth,Auth::AccessToken|Auth::ApiKey),
			"restart_required":true})
	}
}

/// Returns false for an existing destination; never follows an existing symlink.
pub fn initialize(template: &Path, destination: &Path) -> Result<bool> {
	let text = std::fs::read_to_string(template).map_err(|_| Failure("template_unreadable"))?;
	parse_env(&text)?;
	let parent = destination.parent().ok_or(Failure("invalid_destination"))?;
	std::fs::create_dir_all(parent).map_err(|_| Failure("env_directory_failed"))?;
	let mut options = OpenOptions::new();
	options.write(true).create_new(true);
	#[cfg(unix)]
	{
		use std::os::unix::fs::OpenOptionsExt;
		options.mode(0o600);
	}
	let mut file = match options.open(destination) {
		Ok(file) => file,
		Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
		Err(_) => return Err(Failure("env_create_failed")),
	};
	file.write_all(text.as_bytes())
		.map_err(|_| Failure("env_write_failed"))?;
	Ok(true)
}
