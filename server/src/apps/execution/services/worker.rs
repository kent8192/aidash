//! Native management command for a worker process with optional probe and metrics listeners.
use super::lifecycle::{ProbeServer, ProcessLifecycle};
use crate::bootstrap::{self, RuntimeTasks};
use crate::config::{Config, startup::load_settings};
use async_trait::async_trait;
use reinhardt::commands::{
	BaseCommand, CommandArgument, CommandContext, CommandError, CommandResult,
};
use reinhardt::commands::{CapabilityCommand, CapabilityContext, CapabilityRequirement};
use reinhardt::db::backends::DatabaseConnection;
use reinhardt::server::{ShutdownCoordinator, server::shutdown::shutdown_signal};
use reinhardt::{InjectionContext, SingletonScope};
use std::{net::SocketAddr, sync::Arc, time::Duration};

pub struct RunWorker;

#[async_trait]
impl CapabilityCommand for RunWorker {
	fn cli(&self) -> clap::Command {
		clap::Command::new("runworker")
			.bin_name("manage runworker")
			.about(BaseCommand::description(self).to_owned())
			.arg(clap::Arg::new("probe_address").required(false))
	}

	fn requirements(&self, _: &clap::ArgMatches) -> Vec<CapabilityRequirement> {
		// Argument validation precedes the existing worker's full settings load.
		Vec::new()
	}

	async fn execute(
		&self,
		matches: &clap::ArgMatches,
		_: &CapabilityContext,
	) -> CommandResult<()> {
		let mut context = CommandContext::default();
		if let Some(address) = matches.get_one::<String>("probe_address") {
			context.args.push(address.clone());
		}
		BaseCommand::execute(self, &context).await
	}
}

#[async_trait]
impl BaseCommand for RunWorker {
	fn name(&self) -> &str {
		"runworker"
	}
	fn description(&self) -> &str {
		"Run background workers; expose /live and /ready only when configured"
	}
	fn arguments(&self) -> Vec<CommandArgument> {
		vec![CommandArgument::optional(
			"probe_address",
			"Probe listener address (default: node.probe_listen, otherwise disabled)",
		)]
	}
	async fn execute(&self, context: &CommandContext) -> CommandResult<()> {
		if context.args == ["--help"] || context.args == ["-h"] {
			println!(
				"Usage: manage runworker [probe_address]\n{}",
				self.description()
			);
			return Ok(());
		}
		if context.args.len() > 1 {
			return Err(CommandError::InvalidArguments(
				"runworker accepts one optional probe address".into(),
			));
		}
		let address = context
			.args
			.first()
			.map(|value| value.parse::<SocketAddr>())
			.transpose()
			.map_err(|_| {
				CommandError::InvalidArguments(
					"probe address must be an IP address and port".into(),
				)
			})?;
		run(address)
			.await
			.map_err(|error| CommandError::ExecutionError(error.to_string()))
	}
}

pub async fn run(address: Option<SocketAddr>) -> crate::Result<()> {
	let settings = load_settings()?;
	let config = Config::from_settings(&settings)?;
	let address = address.or(settings.node.probe_listen);
	let connection =
		DatabaseConnection::connect_postgres_with_pool_size(&config.database_url, Some(16)).await?;
	let context = Arc::new(InjectionContext::builder(SingletonScope::new()).build());
	let shutdown = ShutdownCoordinator::new(Duration::from_secs(30));
	let mut stopped = shutdown.subscribe();
	ProcessLifecycle::register(&context, shutdown.clone());
	let federation = bootstrap::initialize(&context, &settings, connection).await?;
	let probes = match address {
		Some(address) => Some(ProbeServer::start(address, context, shutdown.clone()).await?),
		None => None,
	};
	let tasks = RuntimeTasks::start(
		federation,
		settings.node.worker_count,
		shutdown.clone(),
		None,
	)
	.await?;
	tokio::select! {
		_ = shutdown_signal() => {},
		_ = stopped.recv() => {},
	}
	shutdown.shutdown();
	let drained = tasks.shutdown().await;
	let closed = match probes {
		Some(probes) => probes.shutdown().await,
		None => Ok(()),
	};
	drained?;
	closed
}
