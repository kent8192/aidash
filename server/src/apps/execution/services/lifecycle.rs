//! Process readiness and a dedicated probe listener, independent of admission.
use crate::{Error, Result, federation::Federation};
use reinhardt::db::backends::{DatabaseConnection, dialect::PostgresBackend};
use reinhardt::di::{KeyedFactoryOutput, SelfKey};
use reinhardt::server::{HttpServer, ShutdownCoordinator};
use reinhardt::{Depends, InjectionContext, injectable};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{net::TcpListener, task::JoinHandle};

#[derive(Clone)]
pub struct ProcessLifecycle {
	pub coordinator: ShutdownCoordinator,
}

impl ProcessLifecycle {
	pub fn register(context: &InjectionContext, coordinator: ShutdownCoordinator) {
		// Depends resolves the provider output key, rather than the unwrapped type.
		context.set_singleton(KeyedFactoryOutput::<SelfKey<Self>, Self>::new(Self {
			coordinator,
		}));
	}
}

#[injectable(scope = "singleton")]
pub async fn provide_lifecycle() -> ProcessLifecycle {
	ProcessLifecycle {
		coordinator: ShutdownCoordinator::new(Duration::from_secs(30)),
	}
}

#[derive(Clone)]
pub struct ProcessHealth {
	connection: DatabaseConnection,
	lifecycle: ProcessLifecycle,
}

#[injectable(scope = "singleton")]
pub async fn provide_health(
	#[inject] runtime: Federation,
	#[inject] lifecycle: Depends<ProcessLifecycle>,
) -> ProcessHealth {
	ProcessHealth {
		connection: DatabaseConnection::new(Arc::new(PostgresBackend::new(
			runtime.store.control_pool.clone(),
		))),
		lifecycle: (*lifecycle).clone(),
	}
}

impl ProcessHealth {
	pub async fn ready(&self) -> bool {
		if self.lifecycle.coordinator.is_shutdown() {
			return false;
		}
		// Probe connection capacity without taking the application visibility lock.
		let available = tokio::time::timeout(
			Duration::from_secs(2),
			super::super::models::health::probe(&self.connection),
		)
		.await
		.is_ok_and(|result| result.is_ok());
		available && !self.lifecycle.coordinator.is_shutdown()
	}
}

/// Own the listener and all connections until the process finishes draining.
pub struct ProbeServer {
	shutdown: ShutdownCoordinator,
	task: Option<JoinHandle<Result<()>>>,
}

impl ProbeServer {
	pub async fn start(
		address: SocketAddr,
		context: Arc<InjectionContext>,
		process: ShutdownCoordinator,
	) -> Result<Self> {
		let listener = TcpListener::bind(address).await?;
		let shutdown = ShutdownCoordinator::new(Duration::from_secs(3));
		let server_shutdown = shutdown.clone();
		let server =
			HttpServer::new(super::super::urls::probe_url_patterns()).with_di_context(context);
		let task = tokio::spawn(async move {
			let result = server
				.listen_on_with_shutdown(listener, server_shutdown)
				.await
				.map_err(|error| Error::External(format!("probe listener failed: {error}")));
			if result.is_err() {
				process.shutdown();
			}
			result
		});
		Ok(Self {
			shutdown,
			task: Some(task),
		})
	}

	pub async fn shutdown(mut self) -> Result<()> {
		self.shutdown.shutdown();
		self.task
			.take()
			.expect("probe task is owned")
			.await
			.map_err(|error| Error::External(format!("probe task failed: {error}")))?
	}
}

impl Drop for ProbeServer {
	fn drop(&mut self) {
		self.shutdown.shutdown();
		if let Some(task) = &self.task {
			task.abort();
		}
	}
}

/// Wait for either process shutdown or the disappearance of its sender.
pub async fn stopped(receiver: &mut tokio::sync::watch::Receiver<bool>) {
	while !*receiver.borrow_and_update() {
		if receiver.changed().await.is_err() {
			return;
		}
	}
}
