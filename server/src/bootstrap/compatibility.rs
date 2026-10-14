//! Native migration and HTTP launch paths used by the compatibility binary.
use super::{RuntimeTasks, initialize};
use crate::apps::execution::services::lifecycle::{ProbeServer, ProcessLifecycle};
use crate::{
	Error, Result,
	config::{Config, settings::ProjectSettings},
};
use reinhardt::commands::CommandContext;
use reinhardt::server::{HttpServer, ShutdownCoordinator, server::shutdown::shutdown_signal};
use reinhardt::{InjectionContext, SingletonScope};
use std::{sync::Arc, time::Duration};

pub async fn migrate(settings: &ProjectSettings) -> Result<()> {
	let config = Config::from_settings(settings)?;
	let mut context = CommandContext::default();
	context.set_option("database".into(), config.database_url);
	context.set_option(
		"migrations-dir".into(),
		settings
			.core
			.base_dir
			.join("migrations")
			.to_string_lossy()
			.into_owned(),
	);
	super::migrations::run(&context).await
}

/// Refuse to start against a database whose native history is not fully applied.
pub async fn require_migrated(settings: &ProjectSettings) -> Result<()> {
	let config = Config::from_settings(settings)?;
	super::migrations::require_current(
		&config.database_url,
		&settings.core.base_dir.join("migrations"),
	)
	.await
}

pub async fn serve(settings: ProjectSettings, workers: bool) -> Result<()> {
	// The server's login cannot change the schema; `migrate` runs first as its owner.
	require_migrated(&settings).await?;
	let config = Config::from_settings(&settings)?;
	let address: std::net::SocketAddr = std::env::var("AIDASH_LISTEN")
		.unwrap_or_else(|_| "127.0.0.1:8080".into())
		.parse()
		.map_err(|_| Error::Invalid("invalid AIDASH_LISTEN".into()))?;
	let connection = crate::database::runtime::connect(&config.database_url, 16).await?;
	let context = Arc::new(InjectionContext::builder(SingletonScope::new()).build());
	let coordinator = ShutdownCoordinator::new(Duration::from_secs(20));
	let mut stopped = coordinator.subscribe();
	ProcessLifecycle::register(&context, coordinator.clone());
	let federation = initialize(&context, &settings, connection).await?;
	let event_streams =
		(*reinhardt::Depends::<crate::sse::Service>::resolve_from_registry(&context, true)
			.await
			.map_err(|error| Error::External(error.to_string()))?)
		.clone();
	let probes = match settings.node.probe_listen {
		Some(address) => {
			Some(ProbeServer::start(address, context.clone(), coordinator.clone()).await?)
		}
		None => None,
	};
	let listener = super::listener::bind(address, std::env::var_os("AIDASH_LISTEN_FD")).await?;
	let tasks = RuntimeTasks::start(
		federation,
		if workers {
			settings.node.worker_count
		} else {
			0
		},
		coordinator.clone(),
		Some(event_streams),
	)
	.await?;
	let server = HttpServer::new(crate::routes().into_server()).with_di_context(context);
	let serving = async {
		server
			.listen_on_with_shutdown(listener, coordinator.clone())
			.await
			.map_err(|error| Error::External(format!("HTTP listener: {error}")))
	};
	tokio::pin!(serving);
	let finished = tokio::select! {
		result = &mut serving => Some(result),
		_ = shutdown_signal() => None,
		_ = stopped.recv() => None,
	};
	coordinator.shutdown();
	let close_http = async {
		match finished {
			Some(result) => result,
			None => tokio::time::timeout(Duration::from_secs(20), serving)
				.await
				.map_err(|_| Error::External("HTTP drain deadline reached".into()))?,
		}
	};
	let close_probes = async {
		match probes {
			Some(probes) => probes.shutdown().await,
			None => Ok(()),
		}
	};
	let (runtime, http, probes) = tokio::join!(tasks.shutdown(), close_http, close_probes);
	runtime?;
	http?;
	probes
}
