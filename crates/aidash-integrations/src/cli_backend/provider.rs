//! Audited CLI profiles. Unsupported isolation fails before inference.
use super::{
	Failure, Result,
	config::{Config, Provider, Values},
	protocol::{self, Request},
};
use aidash_runtime::cli_child::exchange;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub const CLAUDE_VERSION: &str = "2.1.302 (Claude Code)";
pub const CODEX_BLOCKER: &str = "codex_native_tools_and_discovery_isolation_unverified";

pub struct Backend {
	pub config: Config,
	env: Values,
}

impl Backend {
	/// Deterministic process fixtures cannot establish official CLI isolation.
	#[cfg(any(test, feature = "test-support"))]
	pub fn fixture(config: Config, ambient: &Values) -> Self {
		let env = config.child_env(ambient);
		Self { config, env }
	}

	#[cfg(any(test, feature = "test-support"))]
	pub async fn fixture_infer(&self, request: &Request) -> Result<Value> {
		self.exchange(request).await
	}

	pub async fn checked(config: Config, ambient: &Values) -> Result<Self> {
		let env = config.child_env(ambient);
		let backend = Self { config, env };
		backend.check().await?;
		Ok(backend)
	}

	async fn probe(&self, args: &[&str], cwd: &Path) -> Result<Vec<u8>> {
		let output = exchange(
			&self.config.executable,
			&args.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
			&self.env,
			cwd,
			b"",
			Duration::from_secs(15),
			128 * 1024,
		)
		.await?;
		if !output.success {
			return Err(Failure("cli_probe_failed"));
		}
		Ok(output.stdout)
	}

	pub async fn check(&self) -> Result<()> {
		std::fs::create_dir_all(&self.config.work_dir)
			.map_err(|_| Failure("work_directory_failed"))?;
		let cwd = tempfile::Builder::new()
			.prefix("probe-")
			.tempdir_in(&self.config.work_dir)
			.map_err(|_| Failure("work_directory_failed"))?;
		let version = self.probe(&["--version"], cwd.path()).await?;
		if self.config.provider == Provider::Codex {
			let help = self.probe(&["exec", "--help"], cwd.path()).await?;
			let login = self.probe(&["login", "--help"], cwd.path()).await?;
			if !String::from_utf8_lossy(&version).starts_with("codex-cli ")
				|| !String::from_utf8_lossy(&help).contains("--output-schema")
				|| matches!(
					self.config.auth,
					super::config::Auth::AccessToken | super::config::Auth::AccessTokenLogin
				) && !String::from_utf8_lossy(&login).contains("--with-access-token")
			{
				return Err(Failure("unsupported_cli_version"));
			}
			// exec JSONL and app-server's generated 0.165.0 schema provide no
			// audited deny-all for native tools and all automatic discovery.
			// Do not substitute a read-only sandbox or a post-hoc event check.
			return Err(Failure(CODEX_BLOCKER));
		}
		if String::from_utf8_lossy(&version).trim() != CLAUDE_VERSION {
			return Err(Failure("unsupported_cli_version"));
		}
		let help = self.probe(&["--help"], cwd.path()).await?;
		for flag in [
			"--safe-mode",
			"--restricted",
			"--tools",
			"--strict-mcp-config",
			"--setting-sources",
			"--json-schema",
			"--permission-prompts",
			"--disable-slash-commands",
			"--input-format",
		] {
			if !String::from_utf8_lossy(&help).contains(flag) {
				return Err(Failure("unsupported_cli_capability"));
			}
		}
		unmanaged_linux()?;
		// Only personal subscriptions: server-managed organization hooks survive
		// safe mode and cannot be suppressed by unprivileged --settings.
		let status = self
			.probe(&["--safe-mode", "auth", "status"], cwd.path())
			.await?;
		let status: Value =
			serde_json::from_slice(&status).map_err(|_| Failure("invalid_auth_status"))?;
		if status["loggedIn"] != true || status["authMethod"] != "claude.ai" {
			return Err(Failure("claude_saved_login_required"));
		}
		if !matches!(status["subscriptionType"].as_str(), Some("pro" | "max")) {
			return Err(Failure("managed_or_unknown_account_unsupported"));
		}
		Ok(())
	}

	pub async fn infer(&self, request: &Request) -> Result<Value> {
		// Refresh the check before every request, without reading auth files.
		self.check().await?;
		self.exchange(request).await
	}

	async fn exchange(&self, request: &Request) -> Result<Value> {
		if self.config.provider != Provider::Claude {
			return Err(Failure(CODEX_BLOCKER));
		}
		let cwd = tempfile::Builder::new()
			.prefix("call-")
			.tempdir_in(&self.config.work_dir)
			.map_err(|_| Failure("work_directory_failed"))?;
		let mut env = self.env.clone();
		for key in [
			"DISABLE_UPDATES",
			"DISABLE_AUTO_COMPACT",
			"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
			"CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL",
			"CLAUDE_CODE_DISABLE_REFUSAL_FALLBACK",
		] {
			env.insert(key.into(), "1".into());
		}
		env.insert("ENABLE_CLAUDEAI_MCP_SERVERS".into(), "false".into());
		env.insert(
			"CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(),
			request.max_tokens.to_string(),
		);
		let settings = json!({"disableAllHooks":true,"disableClaudeAiConnectors":true,"enableArtifact":false,"disableBundledSkills":true,"autoMemoryEnabled":false});
		let args: Vec<String> = [
			"--print",
			"--safe-mode",
			"--restricted",
			"--tools",
			"",
			"--disallowedTools",
			"*",
			"--strict-mcp-config",
			"--mcp-config",
			"{\"mcpServers\":{}}",
			"--setting-sources",
			"",
			"--settings",
			&settings.to_string(),
			"--disable-slash-commands",
			"--permission-mode",
			"dontAsk",
			"--permission-prompts",
			"none",
			"--no-session-persistence",
			"--no-chrome",
			"--model",
			&self.config.model,
			"--input-format",
			"stream-json",
			"--output-format",
			"stream-json",
			"--verbose",
			"--json-schema",
			&protocol::output_schema().to_string(),
		]
		.iter()
		.map(|s| (*s).into())
		.collect();
		let mut input = serde_json::to_vec(&json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":String::from_utf8_lossy(&request.prompt())}]}})).map_err(|_| Failure("input_serialization"))?;
		input.push(b'\n');
		let output = exchange(
			&self.config.executable,
			&args,
			&env,
			cwd.path(),
			&input,
			Duration::from_secs(self.config.timeout_seconds),
			protocol::MAX_OUTPUT,
		)
		.await?;
		if !output.success {
			return Err(Failure("cli_auth_or_inference_failed"));
		}
		protocol::completion(
			self.config.provider,
			&output.stdout,
			request,
			&self.config.model,
		)
	}
}

fn unmanaged_linux() -> Result<()> {
	if !cfg!(target_os = "linux") {
		return Err(Failure("only_unmanaged_linux_verified_profile"));
	}
	// Managed hooks/helpers must never run before Aidash approval. The first
	// release excludes managed machines and WSL instead of overriding policy.
	for path in ["/etc/claude-code", "/proc/sys/fs/binfmt_misc/WSLInterop"] {
		match std::fs::symlink_metadata(path) {
			Ok(_) => return Err(Failure("managed_or_wsl_host_unsupported")),
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
			Err(_) => return Err(Failure("managed_policy_check_failed")),
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests;
