//! Command composition for the isolated personal CLI listener.
#[path = "cli_backend/examples.rs"]
mod examples;
#[path = "cli_backend/http.rs"]
mod http;
use aidash_integrations::cli_backend::{
	Failure, Result,
	config::{self, Values},
	provider::Backend,
};
use clap::{Parser, Subcommand, ValueEnum};
use std::{path::PathBuf, sync::Arc};

#[derive(Parser)]
#[command(
	name = "cli-backend",
	about = "Personal loopback CLI inference. No implicit env file or live calls."
)]
struct Args {
	#[arg(long, global = true)]
	env_file: Option<PathBuf>,
	#[arg(long, global = true)]
	model: Option<String>,
	#[arg(long, global = true)]
	port: Option<u16>,
	#[arg(long, global = true)]
	executable: Option<PathBuf>,
	#[arg(long, global = true)]
	provider: Option<String>,
	#[arg(long, global = true)]
	auth_mode: Option<String>,
	#[arg(long, global = true)]
	timeout_seconds: Option<u64>,
	#[arg(long, global = true)]
	max_output_tokens: Option<u32>,
	#[arg(long, global = true)]
	context_window: Option<usize>,
	#[arg(long, global = true)]
	work_dir: Option<PathBuf>,
	#[command(subcommand)]
	command: Command,
}
#[derive(Clone, ValueEnum)]
enum Profile {
	Claude,
	CodexNative,
	CodexAccessToken,
	CodexApiKey,
}
impl Profile {
	fn name(&self) -> &'static str {
		match self {
			Self::Claude => "claude",
			Self::CodexNative => "codex-native",
			Self::CodexAccessToken => "codex-access-token",
			Self::CodexApiKey => "codex-api-key",
		}
	}
}
#[derive(Subcommand)]
enum Command {
	/// Create a private 0600 env; existing files are preserved.
	Init {
		#[arg(value_enum)]
		profile: Profile,
		#[arg(long)]
		destination: Option<PathBuf>,
	},
	/// Validate configuration without starting a CLI or calling a model.
	Validate,
	/// Check CLI flags, version, isolation and saved login; no inference.
	Doctor,
	/// Listen on 127.0.0.1. Stop with SIGINT or SIGTERM.
	Serve,
	/// Query the running backend without inference.
	Health,
	/// One short real model call, only when --live is present.
	Smoke {
		#[arg(long)]
		live: bool,
	},
	/// Render schema-validated Registry definitions. Does not register/approve.
	Examples {
		#[arg(long)]
		output_dir: PathBuf,
		#[arg(long)]
		model_entry: String,
		#[arg(long)]
		agent_entry: String,
		#[arg(long, default_value = "1.0.0")]
		version: String,
	},
}

pub async fn main() -> Result<()> {
	let args = Args::parse();
	if let Command::Init {
		profile,
		destination,
	} = &args.command
	{
		let template = PathBuf::from(format!(
			"examples/cli-backends/{}/.env.example",
			profile.name()
		));
		let destination = destination.clone().unwrap_or_else(|| {
			PathBuf::from(format!("private/cli-backends/{}/.env", profile.name()))
		});
		let created = config::initialize(&template, &destination)?;
		println!(
			"{}; set AIDASH_CLI_MODEL and AIDASH_CLI_CONTEXT_WINDOW; token modes also require their selected secret",
			if created {
				"created private env"
			} else {
				"existing env preserved"
			}
		);
		return Ok(());
	}
	if matches!(args.command, Command::Smoke { live: false }) {
		return Err(Failure("smoke_requires_explicit_live"));
	}
	let ambient: Values = std::env::vars().collect();
	let mut overrides = Values::new();
	for (key, value) in [
		("AIDASH_CLI_MODEL", args.model),
		("AIDASH_CLI_PORT", args.port.map(|n| n.to_string())),
		(
			"AIDASH_CLI_EXECUTABLE",
			args.executable.map(|p| p.to_string_lossy().into_owned()),
		),
		("AIDASH_CLI_PROVIDER", args.provider),
		("AIDASH_CLI_AUTH_MODE", args.auth_mode),
		(
			"AIDASH_CLI_TIMEOUT_SECONDS",
			args.timeout_seconds.map(|n| n.to_string()),
		),
		(
			"AIDASH_CLI_MAX_OUTPUT_TOKENS",
			args.max_output_tokens.map(|n| n.to_string()),
		),
		(
			"AIDASH_CLI_CONTEXT_WINDOW",
			args.context_window.map(|n| n.to_string()),
		),
		(
			"AIDASH_CLI_WORK_DIR",
			args.work_dir.map(|p| p.to_string_lossy().into_owned()),
		),
	] {
		if let Some(value) = value {
			overrides.insert(key.into(), value);
		}
	}
	let config = config::load(
		&args.env_file.ok_or(Failure("explicit_env_file_required"))?,
		&ambient,
		&overrides,
	)?;
	match args.command {
		Command::Validate => println!("{}", config.diagnostic()),
		Command::Doctor => {
			Backend::checked(config, &ambient).await?;
			println!("CLI profile and saved login verified; no inference performed");
		}
		Command::Serve => {
			let backend = Arc::new(Backend::checked(config, &ambient).await?);
			let listener =
				tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, backend.config.port))
					.await
					.map_err(|_| Failure("listen_failed"))?;
			println!("{}", backend.config.diagnostic());
			http::serve(listener, backend, shutdown()).await?;
		}
		Command::Health | Command::Smoke { .. } => {
			let client = reqwest::Client::builder()
				.no_proxy()
				.redirect(reqwest::redirect::Policy::none())
				.timeout(std::time::Duration::from_secs(config.timeout_seconds + 65))
				.build()
				.map_err(|_| Failure("health_client_failed"))?;
			let origin = format!("http://127.0.0.1:{}", config.port);
			let response = if matches!(args.command,Command::Health) {
				client.get(format!("{origin}/health")).send().await
			} else {
				client.post(format!("{origin}/v1/chat/completions")).json(&serde_json::json!({"model":config.model,"messages":[{"role":"user","content":"Reply with the single word OK, without any tool calls."}],"max_tokens":config.max_output_tokens.min(1024),"stream":false})).send().await
			}.map_err(|_|Failure("backend_unreachable"))?;
			if !response.status().is_success() {
				return Err(Failure("backend_check_failed"));
			}
			let value: serde_json::Value = response
				.json()
				.await
				.map_err(|_| Failure("invalid_backend_response"))?;
			if matches!(args.command, Command::Smoke { .. }) {
				let parsed = aidash_integrations::inference::parse_openai(
					&serde_json::to_vec(&value).map_err(|_| Failure("invalid_backend_response"))?,
				)
				.map_err(|_| Failure("invalid_smoke_response"))?;
				if parsed.text.trim() != "OK" || !parsed.tool_calls.is_empty() {
					return Err(Failure("unexpected_smoke_response"));
				}
				println!("real CLI model response verified; this is not an Aidash Run");
			} else {
				println!("backend health OK; no inference performed");
			}
		}
		Command::Examples {
			output_dir,
			model_entry,
			agent_entry,
			version,
		} => examples::render(&config, &output_dir, &model_entry, &agent_entry, &version)?,
		Command::Init { .. } => unreachable!(),
	}
	Ok(())
}

async fn shutdown() {
	#[cfg(unix)]
	{
		if let Ok(mut term) =
			tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
		{
			tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=term.recv()=>{} }
			return;
		}
	}
	let _ = tokio::signal::ctrl_c().await;
}
