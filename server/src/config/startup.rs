//! Register the application's lifecycle with Reinhardt's native runserver.
use super::settings::{ProjectSettings, get_settings};
use crate::{
	Result,
	apps::execution::services::lifecycle::{ProbeServer, ProcessLifecycle},
	bootstrap::{self, RuntimeTasks},
};
use async_trait::async_trait;
use reinhardt::commands::{RunserverContext, RunserverHook};
use reinhardt::conf::settings::profile::Profile;
use reinhardt::db::backends::DatabaseConnection;
use reinhardt::macros::hook;
use reinhardt::server::server::shutdown::shutdown_signal;
use std::error::Error;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

// One runserver invocation owns one supervisor. manage drains it before the
// Tokio runtime exits; command discovery and migration commands never start it.
static APPLICATION: Mutex<Option<RunningApplication>> = Mutex::const_new(None);

struct RunningApplication {
	tasks: Option<RuntimeTasks>,
	probes: Option<ProbeServer>,
	signal: JoinHandle<()>,
}

impl Drop for RunningApplication {
	fn drop(&mut self) {
		self.signal.abort();
	}
}

pub fn load_settings() -> Result<ProjectSettings> {
	let settings = get_settings()
		.and_then(|pending| pending.resolve())
		.map_err(|error| crate::Error::Invalid(error.to_string()))?
		.into_parts()
		.0;
	let profile = std::env::var("REINHARDT_ENV").unwrap_or_else(|_| "local".into());
	settings
		.validate(&Profile::parse(&profile))
		.map_err(|error| crate::Error::Invalid(error.to_string()))?;
	Ok(settings)
}

#[hook(on = runserver)]
struct AidashRuntime;

#[async_trait]
impl RunserverHook for AidashRuntime {
	async fn on_server_start(
		&self,
		context: &RunserverContext,
	) -> std::result::Result<(), Box<dyn Error + Send + Sync>> {
		let settings = load_settings()?;
		let config = crate::config::Config::from_settings(&settings)?;
		// The application keeps admission/control capacity separate from native
		// command connections; no migration runs implicitly during startup.
		let connection =
			DatabaseConnection::connect_postgres_with_pool_size(&config.database_url, Some(16))
				.await?;
		let federation = bootstrap::initialize(&context.di_context, &settings, connection).await?;
		ProcessLifecycle::register(&context.di_context, context.shutdown_coordinator.clone());
		let probes = if let Some(address) = settings.node.probe_listen {
			Some(
				ProbeServer::start(
					address,
					context.di_context.clone(),
					context.shutdown_coordinator.clone(),
				)
				.await?,
			)
		} else {
			None
		};
		let tasks = if settings.node.background_enabled {
			Some(
				RuntimeTasks::start(
					federation,
					settings.node.worker_count,
					context.shutdown_coordinator.clone(),
					Some(
						(*reinhardt::Depends::<crate::sse::Service>::resolve_from_registry(
							&context.di_context,
							true,
						)
						.await
						.map_err(|error| crate::Error::External(error.to_string()))?)
						.clone(),
					),
				)
				.await?,
			)
		} else {
			None
		};
		let coordinator = context.shutdown_coordinator.clone();
		let signal = tokio::spawn(async move {
			shutdown_signal().await;
			coordinator.shutdown();
		});
		*APPLICATION.lock().await = Some(RunningApplication {
			tasks,
			probes,
			signal,
		});
		Ok(())
	}
}

pub async fn shutdown() -> Result<()> {
	if let Some(mut application) = APPLICATION.lock().await.take() {
		let drained = if let Some(tasks) = application.tasks.take() {
			tasks.shutdown().await
		} else {
			Ok(())
		};
		let closed = if let Some(probes) = application.probes.take() {
			probes.shutdown().await
		} else {
			Ok(())
		};
		drained?;
		closed?;
	}
	Ok(())
}
