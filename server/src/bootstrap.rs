//! Native application startup and background task supervision.
use crate::{
	Error, Result,
	bus::EventBus,
	config::{Config, settings::ProjectSettings},
	federation::Federation,
	harness::Harness,
	registry::Registry,
	store::Store,
};
use reinhardt::InjectionContext;
use reinhardt::db::backends::DatabaseConnection as BackendConnection;
use reinhardt::server::ShutdownCoordinator;
use std::{sync::Arc, time::Duration};
use tokio::{
	sync::{Notify, watch},
	task::JoinHandle,
};

/// Which build a binary belongs to and the Offerings it runs.
pub mod build;
/// Assemble management adapters before any runtime settings are resolved.
pub mod management;

pub fn management_commands() -> reinhardt::commands::CommandRegistry {
	let mut registry = reinhardt::commands::CommandRegistry::new();
	registry.register_capability(Box::new(
		crate::apps::execution::services::worker::RunWorker,
	));
	registry.register_capability(Box::new(
		crate::apps::execution::services::schema::ApiContract,
	));
	for command in crate::apps::execution::services::node_commands::commands() {
		registry.register_capability(command);
	}
	registry.register_capability(Box::new(
		crate::apps::federation::peer::services::key_commands::Command,
	));
	registry.register_capability(Box::new(
		crate::apps::operations::services::diagnostics::Diagnostics,
	));
	registry.register_capability(Box::new(
		crate::apps::operations::services::migration_seeds::MigrationSeeds,
	));
	registry.register_capability(Box::new(
		crate::semantic::services::memory_recovery::Command,
	));
	registry.register_capability(Box::new(
		crate::apps::identity::services::secret_store_recovery::Command,
	));
	registry.register_capability(Box::new(
		crate::apps::identity::services::demo_data::Command,
	));
	registry.register_capability(Box::new(
		crate::apps::identity::services::subject_credential_command::Command,
	));
	registry
}

/// Which part of a Deployment this process runs. It selects the Capability
/// Token key a Tenant Secret call signs with (ADR 0095).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessRole {
	/// HTTP and background workers in one process.
	Serve,
	/// HTTP only; no agent work runs here.
	Server,
	/// Background workers only.
	Worker,
}

/// Populate the same request container used by native runserver and HTTP fixtures.
pub async fn initialize(
	context: &InjectionContext,
	settings: &ProjectSettings,
	connection: BackendConnection,
) -> Result<Federation> {
	initialize_as(context, settings, connection, ProcessRole::Serve).await
}

/// [`initialize`] for a process that knows which role it serves.
pub async fn initialize_as(
	context: &InjectionContext,
	settings: &ProjectSettings,
	connection: BackendConnection,
	role: ProcessRole,
) -> Result<Federation> {
	// Native runserver/worker also need the application response logs. An
	// embedding host's existing subscriber keeps ownership of logging.
	let _ = tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_default_env()
				.unwrap_or_else(|_| "aidash=info".into()),
		)
		.with_writer(std::io::stderr)
		.with_ansi(false)
		.try_init();
	let mut config = Config::from_settings(settings)?;
	// A Deployment Key that cannot be loaded refuses to start (ADR 0064).
	config.deployment_key =
		configure_deployment_key(settings.deployment.key.as_ref(), &settings.core.base_dir).await?;
	// A malformed Research provider declaration refuses to start (ADR 0055),
	// before registry validation reads which providers are available.
	crate::apps::execution::services::research::configure(
		&settings.research_providers,
		&crate::config::settings::profile(),
	)?;
	let cache_salt = settings.cache_salt.keys()?;
	// Only availability and the current key version; never key material.
	match &cache_salt {
		Some(keys) => tracing::info!(
			current_key_version = keys.current(),
			"Tenant Cache Salt available"
		),
		None => tracing::warn!(
			"no Cache Salt Key configured; Runs of salted Projection Versions will be rejected"
		),
	}
	crate::apps::execution::services::metrics::initialize_recorder()?;
	let pool = connection
		.into_postgres()
		.ok_or_else(|| Error::Invalid("Aidash requires PostgreSQL".into()))?;
	let mut store = Store::from_pool(pool, config.deployment.clone())
		.await?
		.with_dashboard_policy(config.dashboard_policy())
		.with_cache_salt(cache_salt)
		.with_keyless_origins(settings.keyless_endpoints.origins()?)
		.with_egress(aidash_integrations::federation::Egress {
			enabled: config.federation,
			key: config.deployment_key.clone(),
		})
		// An invalid Deployment retention period refuses to start (ADR 0045).
		.with_research_retention(settings.research_retention.rules()?)
		.with_test_limit_ceilings(settings.workbench_test_limits.ceilings()?)
		.with_preview_limits(settings.preview_limits.limits()?)
		.with_preview(
			config
				.preview
				.as_ref()
				.map(|preview| preview.domain.clone()),
			config.preview.as_ref().map(|preview| preview.pod_port),
		);
	configure_provider_credentials(&mut store, &settings.provider_credentials).await?;
	configure_plan_connections(
		&mut store,
		&settings.provider_credentials,
		&settings.plan_connections,
	)?;
	build::current().extension.install(&mut store)?;
	config.tier_limits = build::current().extension.tier_limits(&store)?;
	configure_tenant_secrets(&mut store, &settings.provider_credentials, role)?;
	let registry = Registry::new(store.pool.clone(), &store.deployment)?
		.with_keyless_origins(store.keyless.clone())
		.with_provider_credentials(store.provider_credentials.is_some())
		.with_plan_connections(store.plan_connections_enabled && store.plan_connections.is_some())
		.with_tenant_secrets(store.tenant_secrets.is_some());
	registry.seed_system().await?;
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(120))
		.connect_timeout(Duration::from_secs(10))
		.redirect(reqwest::redirect::Policy::none())
		.build()?;
	let federation = Federation {
		sandbox: Default::default(),
		store,
		registry,
		config,
		client,
		notify: Arc::new(Notify::new()),
	};
	// Persist removed restrictions even for inactive Identities before this
	// policy can serve requests or start workers. Provider IO is unnecessary.
	crate::apps::identity::services::sign_in::check_restrictions(&federation).await?;
	dashboard_authority(&federation).reconcile_policy().await?;
	if let Some(sign_in) = &federation.config.sign_in {
		sign_in.reconcile(&federation).await?;
	}
	context.set_singleton(settings.clone());
	context.set_singleton(federation.clone());
	context.set_singleton(reinhardt::di::KeyedFactoryOutput::<
		reinhardt::di::SelfKey<crate::sse::Service>,
		crate::sse::Service,
	>::new(crate::sse::Service::new(
		crate::sse::Settings::from_env()?,
	)));
	context.set_singleton(reinhardt::di::KeyedFactoryOutput::<
		reinhardt::di::SelfKey<crate::http::Protection>,
		crate::http::Protection,
	>::new(crate::http::Protection::new(
		crate::http::Settings::from_env()?,
	)));
	Ok(federation)
}

/// The Egress gateway keeps admission, limits, recording and package-index
/// rewriting. A Secret Manager Store seals a Tenant Secret's hosts with its
/// value, so the gateway cannot read the value and must forward a
/// secret-bearing request to the Tenant Secret broker (ADR 0095). Without one
/// it refuses to start rather than silently reading nothing.
pub fn require_egress_broker(store: &Store) -> Result<()> {
	let secret_manager = store
		.provider_credentials
		.as_ref()
		.is_some_and(|credentials| credentials.store.seals_secret_hosts());
	let brokered = store
		.tenant_secrets
		.as_ref()
		.is_some_and(|tenant_secrets| tenant_secrets.calls.is_some());
	require_egress_broker_settings(secret_manager, brokered)
}

/// The refusal decision itself, so startup coverage names both inputs.
pub fn require_egress_broker_settings(secret_manager: bool, brokered: bool) -> Result<()> {
	if secret_manager && !brokered {
		return Err(Error::Invalid(
			"the Egress gateway needs provider_credentials.settings.tenant_secret_broker \
			 {endpoint, issuer, audience, kid} on this Secret Manager Store (#310)"
				.into(),
		));
	}
	Ok(())
}

/// Process-owned supervisor; dropping it cancels the supervisor and its JoinSets.
pub struct RuntimeTasks {
	stopping: watch::Sender<bool>,
	supervisor: Option<JoinHandle<Result<()>>>,
}

impl RuntimeTasks {
	pub async fn start(
		federation: Federation,
		worker_count: usize,
		coordinator: ShutdownCoordinator,
		event_streams: Option<crate::sse::Service>,
	) -> Result<Self> {
		let (stopping, mut requested) = watch::channel(false);
		let mut tasks = aidash_runtime::Supervisor::new(Duration::from_secs(20));
		let receiver = tasks.stop_receiver();
		if event_streams.is_some()
			&& let Some(egress) = &federation.store.capabilities.0.egress
		{
			require_egress_broker(&federation.store)?;
			let listener = tokio::net::TcpListener::bind(egress.listen).await?;
			let authority = Arc::new(crate::apps::execution::repositories::egress::Gateway::new(
				federation.store.clone(),
			));
			let stopping = receiver.clone();
			tasks.spawn_service(runtime_task(async move {
				aidash_integrations::egress::serve(listener, authority, stopping)
					.await
					.map_err(Into::into)
			}));
		}
		if event_streams.is_some()
			&& let Some(preview) = &federation.config.preview
			&& preview.in_process
		{
			// The Self-hosted default: the API process is also the Preview
			// gateway. Helm runs it apart and turns `in_process` off there.
			let listener = tokio::net::TcpListener::bind(preview.listen).await?;
			let authority = Arc::new(crate::apps::execution::repositories::previews::Gateway {
				store: federation.store.clone(),
				domain: preview.domain.clone(),
				runtime: Some(federation.clone()),
			});
			let stopping = receiver.clone();
			tasks.spawn_service(runtime_task(async move {
				aidash_integrations::preview::serve(listener, authority, stopping)
					.await
					.map_err(Into::into)
			}));
		}
		tasks.spawn_worker(federation.sandbox.clone().run(receiver.clone()));
		tasks.spawn_service(runtime_task(
			crate::apps::execution::services::metrics::run(coordinator.clone()),
		));
		if let Some(service) = event_streams.clone() {
			let url = federation.config.nats_url.clone();
			let node = federation.config.deployment.to_string();
			let stopping = receiver.clone();
			tasks.spawn_service(runtime_task(async move {
				service.run(&url, &node, stopping).await
			}));
		}
		// Acquire pools before detaching tasks. A startup error drops both JoinSets.
		let coordinator_runtime = federation.for_recovery().await?;
		let (active_recovery, aborted_recovery) =
			transaction_recovery_coordinators(&coordinator_runtime).await?;
		let participant_runtime = federation.for_recovery().await?;
		let worker_runtime = federation.for_runtime_workers().await?;
		tasks.spawn_service(aidash_runtime::transactions::run_coordinator(
			active_recovery,
			aborted_recovery,
		));
		tasks.spawn_service(aidash_runtime::transactions::run_participant(
			transaction_participant(&participant_runtime),
		));
		tasks.spawn_service(aidash_runtime::generation::run(
			Arc::new(generation_provisioning_repository(&worker_runtime)),
			registry_validation_for(&worker_runtime.store),
		));
		tasks.spawn_service(runtime_task(crate::apps::execution::observation::run(
			worker_runtime.clone(),
		)));
		tasks.spawn_service(runtime_task(crate::semantic::worker::run(
			worker_runtime.clone(),
		)));
		if event_streams.is_some() {
			tasks.spawn_service(runtime_task(EventBus::run(federation.clone())));
		}
		if let Some(provider_credentials) = federation.store.provider_credentials.clone() {
			let mut stopping = receiver.clone();
			tasks.spawn_service(runtime_task(async move {
				let mut cursor = None;
				loop {
					if *stopping.borrow() {
						return Ok(());
					}
					match provider_credentials.reconcile_page(cursor).await {
						Ok(result) => {
							cursor = result.next;
							if result.failed != 0 {
								tracing::warn!(
									failed = result.failed,
									"Provider Credential cleanup is pending"
								);
							}
						}
						Err(_) => tracing::warn!(
							"Provider Credential reconciliation inventory unavailable"
						),
					}
					tokio::select! { _=stopping.changed()=>{}, _=tokio::time::sleep(Duration::from_secs(60))=>{} }
				}
			}));
		}
		if !federation.config.marketplace.connections.is_empty() {
			tasks.spawn_service(runtime_task(
				crate::apps::registry::services::marketplace_recalls::run(
					federation.clone(),
					receiver.clone(),
				),
			));
		}
		for service in build::current()
			.extension
			.services(&federation, receiver.clone())
		{
			tasks.spawn_service(runtime_task(service));
		}
		if let Some(tenant_secrets) = federation.store.tenant_secrets.clone() {
			let mut stopping = receiver.clone();
			tasks.spawn_service(runtime_task(async move {
				let mut cursor = None;
				loop {
					if *stopping.borrow() {
						return Ok(());
					}
					match tenant_secrets.reconcile_page(cursor).await {
						Ok(result) => {
							cursor = result.next;
							if result.failed != 0 {
								tracing::warn!(
									failed = result.failed,
									"Tenant Secret cleanup is pending"
								);
							}
						}
						Err(_) => {
							tracing::warn!("Tenant Secret reconciliation inventory unavailable")
						}
					}
					tokio::select! { _=stopping.changed()=>{}, _=tokio::time::sleep(Duration::from_secs(60))=>{} }
				}
			}));
		}
		let activation = crate::activation::Runtime::new(
			worker_runtime.clone(),
			crate::activation::Settings::from_env()?,
			worker_count > 0,
		);
		tasks.spawn_service(runtime_task(activation.clone().run(receiver.clone())));
		if std::env::var_os("AIDASH_CAPABILITY_PROFILE").is_some() {
			let operations = federation.for_runtime_workers().await?;
			let stopping = receiver.clone();
			tasks.spawn_service(runtime_task(async move {
				crate::capabilities::operations::run(operations.store, stopping).await
			}));
			let transfers = federation.for_runtime_workers().await?;
			let stopping = receiver.clone();
			tasks.spawn_service(runtime_task(async move {
				crate::capabilities::transfer::run(transfers, stopping).await
			}));
		}
		if federation.config.dashboard_session().is_some() {
			tasks.spawn_service(runtime_task(crate::dashboard_auth::refresh_active(
				federation.clone(),
				receiver.clone(),
			)));
		} else {
			tasks.spawn_service(runtime_task(
				crate::dashboard_auth::enforce_display_retention(
					federation.clone(),
					receiver.clone(),
				),
			));
		}
		let retention = federation.store.clone();
		let research_retention = federation.store.research_retention;
		let audit_retention_days = federation.config.tenancy.audit_retention_days;
		let serving = event_streams.is_some();
		tasks.spawn_service(runtime_task(async move {
			// Inference progress expires minutes after its outcome, so attempts
			// orphaned on unscheduled Runs are closed first; hourly workbench,
			// Research evidence and Tenant purges keep their cadence on the same loop.
			let mut ticks = 0_u32;
			loop {
				if ticks.is_multiple_of(60) {
					// Deployment Scope: every Tenant's snapshots, under each
					// Tenant's own periods.
					if let Err(error) =
						crate::apps::execution::repositories::research::retention::sweep(
							&retention,
							&research_retention,
						)
						.await
					{
						tracing::warn!(%error, "research evidence retention sweep failed");
					}
					// Provider results under the Research account terms (ADR 0058).
					if let Err(error) =
						crate::apps::execution::services::research::sweep_account_terms(
							&retention,
							crate::apps::execution::services::research::providers().account_terms(),
						)
						.await
					{
						tracing::warn!(%error, "research account terms sweep failed");
					}
					// Deleted Tenants past their grace period, and the audit
					// history that outlived an earlier purge (ADR 0061).
					if let Err(error) = crate::apps::identity::services::tenant_purge::sweep(
						&retention,
						audit_retention_days,
					)
					.await
					{
						tracing::warn!(%error, "Tenant purge sweep failed");
					}
					if let Err(error) = crate::workbench::purge_expired(&retention.pool).await {
						tracing::warn!(%error, "agent test retention cleanup failed");
					}
					if let Err(error) =
						crate::workbench::purge_incident_evidence(&retention.pool).await
					{
						tracing::warn!(%error, "incident evidence retention cleanup failed");
					}
				}
				for _ in 0..16 {
					match retention.close_orphaned_inference().await {
						Ok(closed) if closed > 0 => continue,
						Ok(_) => break,
						Err(error) => {
							tracing::warn!(%error, "orphaned inference attempt closure failed");
							break;
						}
					}
				}
				for _ in 0..16 {
					match retention.prune_inference_progress().await {
						Ok(pruned) if pruned > 0 => continue,
						Ok(_) => break,
						Err(error) => {
							tracing::warn!(%error, "inference progress retention cleanup failed");
							break;
						}
					}
				}
				if serving
					&& let Err(error) =
						crate::apps::execution::repositories::live_previews::sweep(&retention).await
				{
					tracing::warn!(%error, "Live preview sweep failed");
				}
				ticks = ticks.wrapping_add(1);
				tokio::time::sleep(Duration::from_secs(60)).await;
			}
		}));
		if worker_count > 0 {
			let harness = Harness {
				federation: worker_runtime.clone(),
			};
			let receiver = receiver.clone();
			tasks.spawn_service(runtime_task(async move {
				harness.deliver_terminal_messages_until(receiver).await
			}));
		}
		for _ in 0..worker_count {
			let harness = Harness {
				federation: worker_runtime.clone(),
			};
			let receiver = receiver.clone();
			let activation = activation.clone();
			tasks.spawn_worker(runtime_task(async move {
				activation.worker(harness, receiver).await
			}));
		}
		let sandbox = federation.sandbox.clone();
		let mut shutdown = coordinator.subscribe();
		let supervisor = tokio::spawn(async move {
			tasks
				.run_with_shutdown(
					async move {
						tokio::select! {
							_ = shutdown.recv() => {},
							_ = requested.changed() => {},
						}
					},
					move || {
						sandbox.close();
						if let Some(service) = event_streams {
							service.shutdown();
						}
						coordinator.shutdown();
					},
				)
				.await
				.map_err(|error| Error::External(error.to_string()))
		});
		Ok(Self {
			stopping,
			supervisor: Some(supervisor),
		})
	}

	pub async fn shutdown(mut self) -> Result<()> {
		self.stopping.send_replace(true);
		self.supervisor
			.take()
			.expect("supervisor is owned")
			.await
			.map_err(|error| Error::External(format!("runtime supervisor failed: {error}")))?
	}
}

impl Drop for RuntimeTasks {
	fn drop(&mut self) {
		self.stopping.send_replace(true);
		if let Some(task) = &self.supervisor {
			task.abort();
		}
	}
}

async fn runtime_task(
	work: impl std::future::Future<Output = Result<()>>,
) -> aidash_application::Result<()> {
	work.await
		.map_err(|error| aidash_application::Error::External(error.to_string()))
}

/// Bind the policy use case to the same PostgreSQL pool used by HTTP and workers.
pub fn authorization(
	pool: impl Into<crate::database::native::Pool>,
) -> aidash_application::authorization::Authorization {
	aidash_application::authorization::Authorization::new(Arc::new(
		crate::apps::identity::repositories::PolicyRepository { pool: pool.into() },
	))
}

/// External adapters are assembled at the process boundary.
pub fn model_catalog(client: reqwest::Client) -> Arc<dyn aidash_application::ports::ModelCatalog> {
	Arc::new(aidash_integrations::openrouter::OpenRouterCatalog::new(
		client,
	))
}

mod compatibility;
mod listener;
pub use compatibility::{migrate, preview_gateway, require_migrated, serve};

/// Legacy-only Keyless inference: no Cache Salt Keys, so any salted request is
/// rejected without being sent. Run inference uses `admitted_model_provider`,
/// which carries this node's keys.
pub fn model_provider(
	client: reqwest::Client,
	config: aidash_domain::model::ModelConfig,
	keyless: aidash_application::provider_access::KeylessAccess,
) -> Result<Arc<dyn aidash_application::ports::ModelProvider>> {
	aidash_integrations::inference::provider(
		aidash_integrations::keyless::Clients::new(
			client,
			Duration::from_secs(120),
			Duration::from_secs(10),
		)?,
		config,
		Arc::new(keyless),
		Default::default(),
	)
	.map_err(Into::into)
}

/// Keyless provider access limited to this Deployment's permitted private origins.
pub(crate) fn keyless_provider_access(
	store: &Store,
) -> Arc<dyn aidash_application::provider_access::ProviderAccess> {
	Arc::new(store.keyless.clone())
}

pub mod migrations;

/// Bind Kubernetes transport settings without exposing them to the use case.
pub fn deployment_observations(
	settings: &crate::apps::operations::serializers::settings::KubernetesSettings,
) -> aidash_application::deployment::DeploymentObservations {
	let observer = settings.enabled.then(|| {
		Arc::new(aidash_integrations::kubernetes::Kubernetes::new(
			aidash_integrations::kubernetes::Settings {
				endpoint: settings.endpoint.clone(),
				namespace: settings.namespace.clone(),
				release: settings.release.clone(),
				ca_file: settings.ca_file.clone(),
				token_file: settings.token_file.clone(),
			},
		)) as Arc<dyn aidash_application::ports::DeploymentObserver>
	});
	aidash_application::deployment::DeploymentObservations::new(
		settings.namespace.clone(),
		settings.release.clone(),
		observer,
	)
}

/// Semantic transports share rotating credentials with inference and compaction.
pub fn semantic_transport(
	store: &Store,
) -> crate::apps::knowledge::repositories::postgres_vector::Transport {
	crate::apps::knowledge::repositories::postgres_vector::Transport {
		pool: store.pool.clone(),
		embedding: aidash_integrations::semantic::SemanticClient {
			clients: store.provider_clients.clone(),
			access: keyless_provider_access(store),
			context: Default::default(),
		},
	}
}

/// Recovery uses the same native repository and lease fence as runnable steps.
pub fn recovery_store(store: &Store) -> crate::apps::execution::repositories::RecoveryRepository {
	crate::apps::execution::repositories::RecoveryRepository {
		store: store.clone(),
	}
}

/// Compose one agent step with the same durable store and scoped worker authority.
pub(crate) fn execution_environment<'a>(
	federation: &'a crate::federation::Federation,
	guard: Option<&'a crate::authorization::execution::Guard>,
	run: &aidash_domain::Run,
) -> crate::apps::execution::repositories::agent::Environment<'a> {
	crate::apps::execution::repositories::agent::Environment::new(federation, guard, run)
}

pub(crate) async fn nats_transport(
	url: &str,
	node_id: &str,
) -> Result<aidash_integrations::nats::NatsBus> {
	aidash_integrations::nats::NatsBus::connect(url, node_id)
		.await
		.map_err(Into::into)
}
pub(crate) fn event_outbox(store: &Store) -> crate::apps::execution::repositories::events::Outbox {
	crate::apps::execution::repositories::events::Outbox {
		store: store.clone(),
	}
}
pub(crate) fn event_inbox(
	store: &Store,
) -> Result<crate::apps::execution::repositories::events::NativeInbox> {
	Ok(crate::apps::execution::repositories::events::NativeInbox {
		pool: store.pool.clone(),
	})
}

/// HTTP and MCP tools never carry a Deployment secret (#296). They connect
/// under the Keyless endpoint rule and its permitted origins (#376).
pub(crate) fn tool_transport(store: &Store) -> aidash_integrations::tools::HttpTools {
	aidash_integrations::tools::HttpTools::new(store.keyless.clone(), store.tool_clients.clone())
}

/// Discovery and worker retries share the native repository and the Deployment Key.
pub(crate) fn federation(
	f: &Federation,
	sender: &aidash_domain::federation::route::Sender,
) -> aidash_application::federation::Federation {
	aidash_application::federation::Federation::new(
		Arc::new(crate::apps::federation::remote::repositories::Repository {
			federation: f.clone(),
			sender: sender.clone(),
		}),
		Arc::new(peer_transport(f)),
	)
}

pub(crate) fn peer_transport(
	f: &Federation,
) -> crate::apps::federation::remote::services::wire::Wire {
	crate::apps::federation::remote::services::wire::Wire::of(f)
}

/// Assemble definition admission for both management and worker paths.
pub fn registry_validation() -> aidash_application::registry::DefinitionValidation {
	use crate::apps::identity::serializers::provider_credentials::StoreConfig;
	let settings = crate::config::settings::get_settings()
		.ok()
		.and_then(|s| s.resolve().ok())
		.map(|s| s.into_parts().0);
	aidash_application::registry::DefinitionValidation::new(Arc::new(NativeCoreToolCatalog))
	// Settings that fail to resolve leave no origins, so only public addresses pass.
	.with_keyless_origins(
		settings
			.as_ref()
			.and_then(|s| s.keyless_endpoints.origins().ok())
			.map(aidash_application::provider_access::KeylessAccess::permitting)
			.unwrap_or_default(),
	)
	.with_provider_credentials(
		settings
			.as_ref()
			.is_some_and(|s| s.provider_credentials.store.is_some()),
	)
	// Startup refuses `enabled` without a PostgreSQL Store; checking both
	// keeps a management command from admitting what the server would not.
	.with_plan_connections(settings.as_ref().is_some_and(|s| {
		s.plan_connections.enabled
			&& build::offering() == aidash_domain::offering::Offering::SelfHosted
			&& matches!(s.provider_credentials.store, Some(StoreConfig::Postgres { .. }))
	}))
	// A PostgreSQL Store reads Tenant Secrets locally; an Enterprise Store
	// needs its Tenant Secret broker (ADR 0051, ADR 0095).
	.with_tenant_secrets(
		settings
			.as_ref()
			.is_some_and(|s| s.provider_credentials.offers_tenant_secrets()),
	)
	.with_preview_available(
		settings
			.as_ref()
			.is_some_and(|s| s.preview.domain.is_some()),
	)
}
pub fn registry_validation_for(
	store: &Store,
) -> aidash_application::registry::DefinitionValidation {
	registry_validation()
		.with_keyless_origins(store.keyless.clone())
		.with_provider_credentials(store.provider_credentials.is_some())
		.with_plan_connections(store.plan_connections_enabled && store.plan_connections.is_some())
		.with_tenant_secrets(store.tenant_secrets.is_some())
		.with_preview_available(store.preview_domain.is_some())
		.with_egress_limits(
			store
				.capabilities
				.0
				.egress
				.as_ref()
				.map(|e| e.limits)
				.unwrap_or_default(),
		)
}
struct NativeCoreToolCatalog;
impl aidash_application::ports::registry::CoreToolCatalog for NativeCoreToolCatalog {
	fn provider_available(
		&self,
		descriptor: &aidash_domain::tool::providers::ToolDescriptor,
	) -> aidash_application::Result<()> {
		use aidash_domain::tool::providers::{ToolTier, requires_runner};
		if descriptor.provider.starts_with("research.") {
			return crate::apps::execution::services::research::providers().available(descriptor);
		}
		if descriptor.tier != ToolTier::Sandboxed {
			return Ok(());
		}
		let runtime = crate::capabilities::Runtime::from_env()?;
		if !runtime.0.admission
			|| requires_runner(&descriptor.operation) && runtime.0.runner.is_none()
		{
			return Err(aidash_application::Error::Invalid(format!(
				"PROVIDER_UNAVAILABLE: {} requires an admitted capability deployment",
				descriptor.provider
			)));
		}
		Ok(())
	}
	fn specifications(
		&self,
		config: &aidash_domain::capabilities::CoreCapabilities,
	) -> std::collections::BTreeMap<String, aidash_domain::provider::ToolSpec> {
		let mut tools = std::collections::BTreeMap::new();
		crate::capabilities::tools::add(&mut tools, config);
		tools
			.into_iter()
			.map(|(name, tool)| (name, tool.specification()))
			.collect()
	}
}

/// Skill transport is external; HTTP handlers depend only on the use case.
pub fn skill_import() -> aidash_application::registry::skill_import::SkillImport {
	aidash_application::registry::skill_import::SkillImport::new(Arc::new(
		aidash_integrations::skill_import::PublicSkillSource,
	))
}

/// Agent Plugin transport is external; HTTP handlers depend only on the use case.
pub fn agent_plugin_import(
	config: &crate::config::Config,
	tenant: &str,
) -> Result<aidash_application::registry::agent_plugin_import::AgentPluginImport> {
	let marketplace = config
		.marketplace
		.connection(tenant)
		.map(|connection| config.marketplace.client_for(connection))
		.transpose()?;
	Ok(
		aidash_application::registry::agent_plugin_import::AgentPluginImport::new(Arc::new(
			aidash_integrations::marketplace::Sources { marketplace },
		)),
	)
}

/// Keep conversation authorization, writes, and delegation in one borrowed scope.
pub(crate) fn conversation_scope<'a>(
	federation: &'a Federation,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::workspaces::repositories::conversations::NativeConversation<'a> {
	crate::apps::workspaces::repositories::conversations::NativeConversation::new(
		federation, access,
	)
}

/// Channel adapters retain the caller's admission locks and physical transaction.
pub(crate) fn channel_scope<'a>(
	store: &'a Store,
	lease: &'a mut crate::collaboration::access::Lease,
) -> crate::apps::workspaces::repositories::channels::NativeChannels<'a> {
	crate::apps::workspaces::repositories::channels::NativeChannels {
		store,
		lease,
		federation: None,
	}
}

/// Marketplace resolution borrows one current credential/policy/distribution lease.
pub(crate) fn marketplace_definitions_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::marketplace::repositories::definitions::NativeDefinitions<'_> {
	crate::apps::marketplace::repositories::definitions::NativeDefinitions { access, events: () }
}

/// Read/distribution and definition ports borrow the same Access transaction.
pub(crate) fn marketplace_distribution_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::marketplace::repositories::definitions::NativeDefinitions<'_> {
	marketplace_definitions_scope(access)
}

/// Publication writes and outbox events borrow the same credential/policy transaction.
pub(crate) fn marketplace_publication_scope<'a>(
	store: &'a Store,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::marketplace::repositories::definitions::NativeDefinitions<'a, &'a Store> {
	crate::apps::marketplace::repositories::definitions::NativeDefinitions {
		access,
		events: store,
	}
}

/// Deployment Administrator adoption and subject installation use the same staging implementation.
pub(crate) fn marketplace_staging_scope<'a>(
	store: &'a Store,
	tx: &'a mut crate::database::native::Transaction,
	actor: &'a str,
) -> crate::apps::marketplace::repositories::installations::NativeStaging<'a> {
	crate::apps::marketplace::repositories::installations::NativeStaging { store, tx, actor }
}

/// Trusted transport authority and the original transaction are shared with maintenance use cases.
pub(crate) fn marketplace_deployment_administrator_scope<'a>(
	store: &'a Store,
	tx: &'a mut crate::database::native::Transaction,
	principal: aidash_domain::identity::Principal,
) -> crate::apps::marketplace::repositories::operations::NativeDeploymentAdministrator<'a> {
	crate::apps::marketplace::repositories::operations::NativeDeploymentAdministrator {
		staging: marketplace_staging_scope(store, tx, "deployment-administrator-adoption"),
		principal,
		approving: None,
		node: None,
	}
}

/// A Tenant subject's Marketplace activation, after its `catalog.approve`
/// decisions on the same transaction.
pub(crate) fn marketplace_tenant_activation_scope<'a>(
	store: &'a Store,
	tx: &'a mut crate::database::native::Transaction,
	principal: aidash_domain::identity::Principal,
	tenant: String,
) -> crate::apps::marketplace::repositories::operations::NativeDeploymentAdministrator<'a> {
	crate::apps::marketplace::repositories::operations::NativeDeploymentAdministrator {
		staging: marketplace_staging_scope(store, tx, "tenant-activation"),
		principal,
		approving: Some(tenant),
		node: None,
	}
}

/// The same with the Node whose Catalog Approvals and activation the caller
/// already selected.
pub(crate) fn marketplace_tenant_activation_scope_at<'a>(
	store: &'a Store,
	tx: &'a mut crate::database::native::Transaction,
	principal: aidash_domain::identity::Principal,
	tenant: String,
	node: aidash_domain::nodes::NodeName,
) -> crate::apps::marketplace::repositories::operations::NativeDeploymentAdministrator<'a> {
	crate::apps::marketplace::repositories::operations::NativeDeploymentAdministrator {
		staging: marketplace_staging_scope(store, tx, "tenant-activation"),
		principal,
		approving: Some(tenant),
		node: Some(node),
	}
}

pub(crate) fn marketplace_provenance_scope<'a>(
	tx: &'a mut crate::database::native::Transaction,
) -> crate::apps::marketplace::repositories::installations::NativeProvenance<'a> {
	crate::apps::marketplace::repositories::installations::NativeProvenance { tx }
}

/// Discovery, exact reads and worker admission share the current Access transaction.
pub(crate) fn catalog_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::catalog::NativeCatalog<'_> {
	crate::apps::identity::repositories::catalog::NativeCatalog(access)
}

pub(crate) fn catalog_mutation_scope<'a>(
	tx: &'a mut crate::database::native::Transaction,
) -> crate::apps::identity::repositories::catalog_mutation::NativeMutation<'a> {
	crate::apps::identity::repositories::catalog_mutation::NativeMutation(tx)
}
/// Transport and trusted maintenance callers supply authority, never a request-body actor label.
pub(crate) fn catalog_administrator_scope<'a>(
	tx: &'a mut crate::database::native::Transaction,
	principal: aidash_domain::identity::Principal,
) -> crate::apps::identity::repositories::catalog_mutation::NativeAdministrator<'a> {
	crate::apps::identity::repositories::catalog_mutation::NativeAdministrator {
		mutation: catalog_mutation_scope(tx),
		principal,
	}
}

/// Compose graph verification around the initiating reader's live authority lease.
pub(crate) fn dependency_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::dependencies::NativeDependencies<'_> {
	crate::apps::identity::repositories::dependencies::NativeDependencies(access)
}

pub(crate) fn dependency_transport(
	access: &crate::authorization::access::Access,
) -> crate::apps::federation::remote::services::wire::Reach {
	crate::apps::federation::remote::services::wire::Reach(access.wire.clone())
}

pub(crate) fn foreign_run_read_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::foreign_reads::NativeForeignReads<'_> {
	crate::apps::identity::repositories::foreign_reads::NativeForeignReads(access)
}

pub(crate) fn generation_policy_repository(
	store: &Store,
	actor: crate::authorization::identity::Actor,
) -> crate::apps::execution::generation::repositories::policy::NativePolicies {
	use crate::authorization::identity::Actor;
	let principal = match &actor {
		Actor::DeploymentAdministrator => {
			aidash_domain::identity::Principal::DeploymentAdministrator
		}
		Actor::Subject(identity) => aidash_domain::identity::Principal::Subject {
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		},
	};
	crate::apps::execution::generation::repositories::policy::NativePolicies {
		store: store.clone(),
		actor,
		principal,
	}
}

pub(crate) fn generation_visibility_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::execution::generation::repositories::contracts::NativeVisibility<'_> {
	crate::apps::execution::generation::repositories::contracts::NativeVisibility { access }
}

pub(crate) fn generation_read_repository(
	store: &Store,
	actor: crate::authorization::identity::Actor,
) -> crate::apps::execution::generation::repositories::reads::NativeReads {
	use crate::authorization::identity::Actor;
	let principal = match &actor {
		Actor::DeploymentAdministrator => {
			aidash_domain::identity::Principal::DeploymentAdministrator
		}
		Actor::Subject(identity) => aidash_domain::identity::Principal::Subject {
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		},
	};
	crate::apps::execution::generation::repositories::reads::NativeReads {
		store: store.clone(),
		actor,
		principal,
	}
}

pub(crate) fn generation_lifecycle_scope<'a>(
	runtime: &'a crate::federation::Federation,
	transaction: &'a mut crate::database::native::Transaction,
) -> crate::apps::execution::generation::repositories::lifecycle::NativeLifecycle<'a> {
	crate::apps::execution::generation::repositories::lifecycle::NativeLifecycle {
		runtime,
		transaction,
	}
}
pub(crate) fn generation_control_repository(
	runtime: &crate::federation::Federation,
	actor: crate::authorization::identity::Actor,
) -> crate::apps::execution::generation::repositories::controls::NativeControls {
	use crate::authorization::identity::Actor;
	let principal = match &actor {
		Actor::DeploymentAdministrator => {
			aidash_domain::identity::Principal::DeploymentAdministrator
		}
		Actor::Subject(identity) => aidash_domain::identity::Principal::Subject {
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		},
	};
	crate::apps::execution::generation::repositories::controls::NativeControls {
		runtime: runtime.clone(),
		actor,
		principal,
	}
}

/// Use the same authority scope for HTTP admission and worker-origin delegation.
pub(crate) fn generation_assignment_scope<'a>(
	runtime: &'a Federation,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::execution::generation::repositories::assignment::NativeAssignment<'a> {
	crate::apps::execution::generation::repositories::assignment::NativeAssignment {
		runtime,
		access,
	}
}
pub(crate) fn generation_assignment_repository(
	runtime: &Federation,
	identity: &crate::authorization::identity::SubjectIdentity,
) -> crate::apps::execution::generation::repositories::assignment::NativeAssignments {
	crate::apps::execution::generation::repositories::assignment::NativeAssignments {
		runtime: runtime.clone(),
		identity: identity.clone(),
	}
}

pub(crate) fn generation_publication_scope<'a>(
	runtime: &'a Federation,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::execution::generation::repositories::publication::NativePublication<'a> {
	crate::apps::execution::generation::repositories::publication::NativePublication {
		runtime,
		access,
	}
}
pub(crate) fn generation_live_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::execution::generation::repositories::publication::NativeLive<'_> {
	crate::apps::execution::generation::repositories::publication::NativeLive { access }
}

/// HTTP and background recovery share the same PostgreSQL authority adapters.
pub(crate) fn generation_provisioning_repository(
	runtime: &Federation,
) -> crate::apps::execution::generation::repositories::provisioning::NativeProvisioning {
	crate::apps::execution::generation::repositories::provisioning::NativeProvisioning {
		runtime: runtime.clone(),
	}
}

pub(crate) fn generation_dispatch_repository(
	store: &Store,
	scope: aidash_application::tenancy::TenancyScope,
) -> impl aidash_application::ports::generation::dispatch::GenerationDispatchRepository {
	crate::apps::execution::generation::repositories::dispatch::NativeDispatch {
		store: store.clone(),
		scope,
	}
}
pub(crate) fn generation_dispatch_settlement(
	runtime: &Federation,
) -> impl aidash_application::ports::generation::dispatch::GenerationDispatchSettlement {
	crate::apps::execution::generation::repositories::dispatch::NativeSettlement {
		runtime: runtime.clone(),
	}
}

pub(crate) fn generation_settlement_repository(
	store: &Store,
	scope: aidash_application::tenancy::TenancyScope,
) -> impl aidash_application::ports::generation::settlement::GenerationSettlementRepository {
	crate::apps::execution::generation::repositories::settlement::NativeSettlementRepository {
		store: store.clone(),
		scope,
	}
}
pub(crate) fn generation_usage_authority_scope(
	access: &mut crate::authorization::access::Access,
) -> impl aidash_application::ports::generation::reservation::GenerationUsageAuthority + '_ {
	crate::apps::execution::generation::repositories::reservation::NativeUsageAuthority {
		catalog: catalog_scope(access),
	}
}
pub(crate) fn generation_reservation_repository(
	store: &Store,
	scope: aidash_application::tenancy::TenancyScope,
) -> impl aidash_application::ports::generation::reservation::GenerationReservationRepository {
	crate::apps::execution::generation::repositories::reservation::NativeReservation {
		store: store.clone(),
		scope,
	}
}

pub(crate) fn generation_inference_authority_scope(
	access: &mut crate::authorization::access::Access,
) -> impl aidash_application::ports::generation::inference::GenerationInferenceAuthority + '_ {
	crate::apps::execution::generation::repositories::inference::NativeInferenceAuthority { access }
}
pub(crate) fn generation_inference_repository(
	store: &Store,
	scope: aidash_application::tenancy::TenancyScope,
) -> crate::apps::execution::generation::repositories::inference::NativeInferenceRepository {
	crate::apps::execution::generation::repositories::inference::NativeInferenceRepository {
		pool: store.pool.clone(),
		scope,
		node_id: store.deployment.to_string(),
	}
}

pub(crate) fn generation_embedding_authority_scope(
	access: &mut crate::authorization::access::Access,
) -> impl aidash_application::ports::generation::embedding::GenerationEmbeddingAuthority + '_ {
	crate::apps::execution::generation::repositories::embedding::NativeEmbeddingAuthority {
		catalog: catalog_scope(access),
	}
}

pub(crate) fn generation_embedding_repository(
	store: &Store,
	scope: aidash_application::tenancy::TenancyScope,
) -> crate::apps::execution::generation::repositories::embedding::NativeEmbeddingRepository {
	crate::apps::execution::generation::repositories::embedding::NativeEmbeddingRepository {
		pool: store.pool.clone(),
		scope,
		node_id: store.deployment.to_string(),
	}
}

pub(crate) fn generation_compaction_authority_scope(
	access: &mut crate::authorization::access::Access,
) -> impl aidash_application::ports::generation::compaction::GenerationCompactionAuthority + '_ {
	crate::apps::execution::generation::repositories::compaction::NativeCompactionAuthority {
		catalog: catalog_scope(access),
	}
}
pub(crate) fn generation_compaction_repository(
	store: &Store,
	scope: aidash_application::tenancy::TenancyScope,
) -> crate::apps::execution::generation::repositories::compaction::NativeCompactionRepository {
	crate::apps::execution::generation::repositories::compaction::NativeCompactionRepository {
		store: store.clone(),
		scope,
	}
}
pub(crate) fn generation_summary_repository(
	store: &Store,
	scope: aidash_application::tenancy::TenancyScope,
) -> crate::apps::execution::generation::repositories::summary::NativeSummaryRepository {
	crate::apps::execution::generation::repositories::summary::NativeSummaryRepository {
		store: store.clone(),
		scope,
	}
}
pub(crate) fn generation_protocol_authority(
	federation: &crate::federation::Federation,
) -> crate::apps::execution::generation::repositories::protocol::NativeProtocolAuthority {
	crate::apps::execution::generation::repositories::protocol::NativeProtocolAuthority {
		federation: federation.clone(),
		receiver: None,
	}
}
/// The authority of an inbound request that arrived at the Node `receiver`.
pub(crate) fn generation_protocol_receiver(
	federation: &crate::federation::Federation,
	receiver: aidash_domain::nodes::NodeName,
) -> crate::apps::execution::generation::repositories::protocol::NativeProtocolAuthority {
	crate::apps::execution::generation::repositories::protocol::NativeProtocolAuthority {
		federation: federation.clone(),
		receiver: Some(receiver),
	}
}
pub(crate) fn generation_protocol_repository(
	store: &Store,
) -> crate::apps::execution::generation::repositories::protocol::NativeProtocolRepository {
	crate::apps::execution::generation::repositories::protocol::NativeProtocolRepository {
		store: store.clone(),
	}
}

pub(crate) fn generation_remote_compaction_scope<'a>(
	access: &'a mut crate::authorization::access::Access,
	federation: &crate::federation::Federation,
) -> crate::apps::execution::generation::repositories::compaction::remote::NativeRemoteCompaction<'a>
{
	crate::apps::execution::generation::repositories::compaction::remote::NativeRemoteCompaction {
		catalog: catalog_scope(access),
		federation: federation.clone(),
	}
}
pub(crate) fn generation_foreign_guard(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::execution::generation::repositories::foreign::NativeForeignGuard<'_> {
	crate::apps::execution::generation::repositories::foreign::NativeForeignGuard { access }
}
pub(crate) fn generation_foreign_binding<'a>(
	access: &'a mut crate::authorization::access::Access,
	federation: &crate::federation::Federation,
) -> crate::apps::execution::generation::repositories::foreign::NativeForeignBinding<'a> {
	crate::apps::execution::generation::repositories::foreign::NativeForeignBinding {
		access,
		federation: federation.clone(),
	}
}

pub(crate) fn generation_home_repository(
	f: &Federation,
	actor: crate::authorization::identity::Actor,
) -> crate::apps::execution::generation::repositories::foreign::home::NativeHome {
	use crate::authorization::identity::Actor;
	let principal = match &actor {
		Actor::DeploymentAdministrator => {
			aidash_domain::identity::Principal::DeploymentAdministrator
		}
		Actor::Subject(identity) => aidash_domain::identity::Principal::Subject {
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		},
	};
	crate::apps::execution::generation::repositories::foreign::home::NativeHome {
		federation: f.clone(),
		actor: Some(actor),
		principal: Some(principal),
		receiver: None,
	}
}
/// A peer-authenticated request has no local actor; its authority is the intent it names.
pub(crate) fn peer_generation_home_repository(
	f: &Federation,
	receiver: &aidash_domain::nodes::NodeAddress,
) -> crate::apps::execution::generation::repositories::foreign::home::NativeHome {
	crate::apps::execution::generation::repositories::foreign::home::NativeHome {
		federation: f.clone(),
		actor: None,
		principal: None,
		receiver: Some(receiver.to_string()),
	}
}

pub(crate) fn generation_receiver_repository(
	f: &Federation,
	receiver: aidash_domain::nodes::NodeName,
) -> crate::apps::execution::generation::repositories::foreign::receiver::NativeReceiver {
	crate::apps::execution::generation::repositories::foreign::receiver::NativeReceiver {
		address: f.config.deployment.node(receiver.clone()).to_string(),
		federation: f.clone(),
		receiver,
	}
}

pub(crate) fn generation_foreign_maintenance(
	f: &Federation,
) -> crate::apps::execution::generation::repositories::foreign::maintenance::NativeMaintenance {
	crate::apps::execution::generation::repositories::foreign::maintenance::NativeMaintenance {
		federation: f.clone(),
	}
}

/// Indexing and cleanup share the worker's existing database pool and transaction gate.
pub(crate) fn semantic_indexing_repository(
	store: &Store,
) -> crate::apps::knowledge::repositories::indexing::NativeIndexing {
	crate::apps::knowledge::repositories::indexing::NativeIndexing {
		store: store.clone(),
	}
}

/// Both public and inherited worker entry writes retain their existing authority scope.
pub(crate) fn semantic_entries_scope<'a, 'scope>(
	lease: &'a mut crate::semantic::service::Lease<'scope>,
) -> crate::apps::knowledge::repositories::mutations::Entries<'a, 'scope> {
	crate::apps::knowledge::repositories::mutations::Entries { lease }
}

pub(crate) fn semantic_configuration_repository(
	store: &Store,
	actor: &crate::authorization::identity::Actor,
	workspace: uuid::Uuid,
) -> crate::apps::knowledge::repositories::mutations::NativeConfiguration {
	crate::apps::knowledge::repositories::mutations::NativeConfiguration {
		store: store.clone(),
		actor: actor.clone(),
		workspace,
	}
}

/// Retrieval retains the same native scope and pool for queries and embedding allowance.
pub(crate) fn semantic_retrieval_scope<'a, 'scope>(
	store: &Store,
	lease: &'a mut crate::semantic::service::Lease<'scope>,
) -> crate::apps::knowledge::repositories::retrieval::Retrieval<'a, 'scope> {
	crate::apps::knowledge::repositories::retrieval::Retrieval {
		entries: crate::apps::knowledge::repositories::mutations::Entries { lease },
		store: store.clone(),
	}
}

/// HTTP, indexing, and worker embedding share the same current authority lease.
pub(crate) fn semantic_embedding_scope<'a, 'scope>(
	store: &Store,
	lease: &'a mut crate::semantic::service::Lease<'scope>,
) -> crate::apps::knowledge::repositories::embedding::Embedding<'a, 'scope> {
	crate::apps::knowledge::repositories::embedding::Embedding {
		lease,
		store: store.clone(),
	}
}

/// Memory and dependency checks borrow the exact same transaction and authority.
pub(crate) fn semantic_memory_scope<'a, 'scope>(
	lease: &'a mut crate::semantic::service::Lease<'scope>,
) -> crate::apps::knowledge::repositories::memory::Memory<'a, 'scope> {
	crate::apps::knowledge::repositories::memory::Memory {
		entries: crate::apps::knowledge::repositories::mutations::Entries { lease },
	}
}

pub(crate) fn execution_grants(
	store: &Store,
) -> crate::apps::identity::repositories::execution::Grants<'_> {
	crate::apps::identity::repositories::execution::Grants { store }
}
pub(crate) fn execution_grant_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::execution::GrantScope<'_> {
	crate::apps::identity::repositories::execution::GrantScope { access }
}

pub(crate) fn graph_projection_scope<'a, 'scope>(
	f: &'a Federation,
	authority: &'a mut crate::apps::identity::services::peer::graph::GraphAuthority<'scope>,
	source_node: &'a str,
	node: &'a str,
	revision: &'a str,
) -> crate::apps::identity::repositories::graph::Projection<'a, 'scope> {
	crate::apps::identity::repositories::graph::Projection {
		f,
		authority,
		source_node,
		node,
		revision,
	}
}

pub(crate) fn execution_admission_scope<'a>(
	f: &'a Federation,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::admission::Admission<'a> {
	crate::apps::identity::repositories::admission::Admission { f, access }
}

/// Disclosure adapters borrow the caller's transaction; no second policy lease is opened.
pub(crate) fn run_visibility_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::visibility::Reads<'_> {
	crate::apps::identity::repositories::visibility::Reads { access }
}
pub(crate) fn native_run_visibility_scope(
	access: &mut crate::authorization::access::NativeAccess,
) -> crate::apps::identity::repositories::visibility::NativeReads<'_> {
	crate::apps::identity::repositories::visibility::NativeReads { access }
}

pub(crate) fn native_generation_visibility_scope(
	access: &mut crate::authorization::access::NativeAccess,
) -> crate::apps::identity::repositories::visibility::generation::NativeGeneration<'_> {
	crate::apps::identity::repositories::visibility::generation::NativeGeneration { access }
}

pub(crate) fn semantic_disclosure_scope<'a, 'scope>(
	lease: &'a mut crate::apps::knowledge::repositories::access::Lease<'scope>,
) -> crate::apps::knowledge::repositories::disclosure::Disclosure<'a, 'scope> {
	crate::apps::knowledge::repositories::disclosure::Disclosure { lease }
}

pub(crate) fn native_registry_transport(
	access: &crate::authorization::access::NativeAccess,
) -> crate::apps::federation::remote::services::wire::Reach {
	crate::apps::federation::remote::services::wire::Reach(access.remote_transport())
}
pub(crate) fn registry_verification_scope<'a>(
	access: &'a mut crate::authorization::access::Access,
	node: &'a aidash_domain::nodes::NodeName,
) -> crate::apps::identity::repositories::visibility::registry_reads::VerificationScope<'a> {
	crate::apps::identity::repositories::visibility::registry_reads::VerificationScope {
		access,
		node,
	}
}

pub(crate) fn stream_authority_store(
	workspaces: &crate::apps::identity::services::workspace::Workspaces,
) -> crate::apps::identity::repositories::visibility::stream::Authority<'_> {
	crate::apps::identity::repositories::visibility::stream::Authority { workspaces }
}

/// Workspace policies and atomic writes share the caller's existing authority transaction.
pub(crate) fn workspace_mutations<'a>(
	access: &'a mut crate::authorization::access::Access,
	store: &'a Store,
) -> crate::apps::workspaces::repositories::mutations::NativeMutations<'a> {
	crate::apps::workspaces::repositories::mutations::NativeMutations { access, store }
}

/// Worker task effects use the caller's inherited authority transaction.
pub(crate) fn worker_task_scope<'a>(
	federation: &'a crate::federation::Federation,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::worker_tasks::WorkerTasks<'a> {
	crate::apps::identity::repositories::worker_tasks::WorkerTasks { federation, access }
}

/// Ordered attachment bytes and current subject disclosure borrow the same Access lease.
pub(crate) fn scoped_media(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::workspaces::repositories::media::ScopedMedia<'_> {
	crate::apps::workspaces::repositories::media::ScopedMedia { access }
}

/// File selections resolve under one owned current-run authority guard and immutable snapshot.
pub(crate) fn selected_media<'a>(
	store: &'a Store,
	run: &'a aidash_domain::Run,
	access: Arc<tokio::sync::RwLock<crate::authorization::access::Access>>,
) -> crate::apps::workspaces::repositories::media::SelectedMedia<'a> {
	crate::apps::workspaces::repositories::media::SelectedMedia::new(store, run, access)
}

/// Current builtin and plugin authority borrows the same worker Access object.
pub(crate) fn agent_tool_repository<'a>(
	remote: Option<&'a Federation>,
	access: &'a Arc<tokio::sync::RwLock<crate::authorization::access::Access>>,
	run: &'a crate::domain::Run,
) -> crate::apps::identity::repositories::tools::AgentTools<'a> {
	crate::apps::identity::repositories::tools::AgentTools {
		remote,
		access,
		run,
	}
}

/// Revalidate current execution and catalog approval on the worker's retained transaction.
pub(crate) fn run_guard_scope<'a>(
	federation: &'a Federation,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::execution::guard::RunGuard<'a> {
	crate::apps::identity::repositories::execution::guard::RunGuard { federation, access }
}

/// Subject Run controls share the original grant and state transaction.
pub(crate) fn run_control_repository<'a>(
	federation: &'a Federation,
	identity: &'a crate::authorization::identity::SubjectIdentity,
) -> crate::apps::identity::repositories::execution::control::Controls<'a> {
	crate::apps::identity::repositories::execution::control::Controls {
		federation,
		identity,
	}
}

/// One current disclosure transaction owns the native Run inspection projection.
pub(crate) fn run_details_repository<'a>(
	federation: &'a Federation,
	identity: &'a crate::authorization::identity::SubjectIdentity,
) -> crate::apps::identity::repositories::execution::details::Details<'a> {
	crate::apps::identity::repositories::execution::details::Details {
		federation,
		identity,
	}
}

/// Semantic worker context retains current local source authority or refreshes remote admission.
pub(crate) fn run_semantic_repository<'a>(
	store: &'a Store,
	remote: Option<&'a Federation>,
	access: &'a Arc<tokio::sync::RwLock<crate::authorization::access::Access>>,
	run: &'a crate::domain::Run,
	agent: &'a crate::registry::AgentConfig,
) -> crate::apps::identity::repositories::execution::semantic::ContextRepository<'a> {
	crate::apps::identity::repositories::execution::semantic::ContextRepository {
		store,
		remote,
		access,
		run,
		agent,
	}
}

/// Scoped cancellation keeps the existing native worker-token and revision fences.
pub(crate) fn scoped_cancellation(
	store: &Store,
) -> crate::apps::identity::repositories::execution::boundaries::Cancellation<'_> {
	crate::apps::identity::repositories::execution::boundaries::Cancellation { store }
}
/// Inference approvals retain the current caller-owned Access transaction.
pub(crate) fn inference_approval_scope<'a>(
	access: &'a mut crate::authorization::access::Access,
	run: &'a crate::domain::Run,
	remote: bool,
) -> crate::apps::identity::repositories::execution::boundaries::InferenceApproval<'a> {
	crate::apps::identity::repositories::execution::boundaries::InferenceApproval {
		access,
		run,
		remote,
	}
}

/// Worker resume borrows the same authority and technical database retry classification.
pub(crate) fn worker_resume_repository<'a>(
	federation: &'a Federation,
	remote: Option<&'a Federation>,
	access: &'a Arc<tokio::sync::RwLock<crate::authorization::access::Access>>,
	run: &'a crate::domain::Run,
	agent: &'a crate::registry::AgentConfig,
) -> crate::apps::identity::repositories::execution::resume::Resume<'a> {
	crate::apps::identity::repositories::execution::resume::Resume {
		federation,
		remote,
		access,
		run,
		agent,
	}
}

/// Inference admission assembles the existing local quota and Home protocol adapters.
pub(crate) fn inference_admission_repository<'a>(
	store: &'a Store,
	remote: Option<&'a Federation>,
	access: &'a Arc<tokio::sync::RwLock<crate::authorization::access::Access>>,
	run: &'a crate::domain::Run,
) -> crate::apps::identity::repositories::execution::admission::Admissions<'a> {
	crate::apps::identity::repositories::execution::admission::Admissions {
		store,
		remote,
		access,
		run,
		purpose: crate::generation::remote::Purpose::Inference,
	}
}

/// Summary Stage admission charges model tokens locally and the `summary`
/// purpose (with its call allowance) at the Home.
pub(crate) fn summary_admission_repository<'a>(
	store: &'a Store,
	remote: Option<&'a Federation>,
	access: &'a Arc<tokio::sync::RwLock<crate::authorization::access::Access>>,
	run: &'a crate::domain::Run,
) -> crate::apps::identity::repositories::execution::admission::Admissions<'a> {
	crate::apps::identity::repositories::execution::admission::Admissions {
		store,
		remote,
		access,
		run,
		purpose: crate::generation::remote::Purpose::Summary,
	}
}

/// Worker and delivery entry share the same native authority implementation.
pub(crate) fn worker_entry_repository(
	federation: &Federation,
) -> crate::apps::identity::repositories::execution::entry::Entries<'_> {
	crate::apps::identity::repositories::execution::entry::Entries { federation }
}

/// Capability admission borrows the existing current Run, Area and authority transaction.
pub(crate) fn operation_admission_scope<'a>(
	store: &'a Store,
	access: &'a mut crate::authorization::access::Access,
	run: &'a crate::domain::Run,
	area: &'a mut crate::capabilities::contracts::Area,
) -> crate::apps::execution::repositories::capabilities::Admission<'a> {
	crate::apps::execution::repositories::capabilities::Admission {
		store,
		access,
		run,
		area,
	}
}

/// Operation controls retain the locked native row and current worker authority.
pub(crate) fn operation_control_scope<'a>(
	store: &'a Store,
	access: &'a mut crate::authorization::access::Access,
	run: &'a crate::domain::Run,
	area: &'a crate::capabilities::contracts::Area,
) -> crate::apps::execution::repositories::capabilities::Control<'a> {
	crate::apps::execution::repositories::capabilities::Control::new(store, access, run, area)
}

pub(crate) fn transaction_authority_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::federation::transactions::repositories::authority::Scope<
	&mut crate::authorization::access::Access,
> {
	crate::apps::federation::transactions::repositories::authority::Scope { access }
}

/// HTTP, peer and recovery callers share the same native authority implementation.
pub(crate) fn transaction_authority_repository(
	runtime: &Federation,
) -> crate::apps::federation::transactions::repositories::authority::control::Repository {
	crate::apps::federation::transactions::repositories::authority::control::Repository {
		runtime: runtime.clone(),
		admitted: None,
		receiver: None,
	}
}

/// The participant's side of a request an inbound Node made.
pub(crate) fn transaction_authority_repository_as(
	runtime: &Federation,
	admitted: &aidash_application::federation::inbound::Admitted,
) -> crate::apps::federation::transactions::repositories::authority::control::Repository {
	crate::apps::federation::transactions::repositories::authority::control::Repository {
		runtime: runtime.clone(),
		admitted: Some(admitted.clone()),
		receiver: Some(admitted.receiver.to_string()),
	}
}

pub(crate) fn transaction_admission_repository(
	runtime: &Federation,
) -> crate::apps::federation::transactions::repositories::admission::Repository {
	crate::apps::federation::transactions::repositories::admission::Repository {
		runtime: runtime.clone(),
	}
}

/// Borrow the caller's physical transaction without reacquiring authority locks.
pub(crate) fn transaction_admission_scope<'a>(
	runtime: &'a Federation,
	tx: &'a mut dyn reinhardt::db::backends::TransactionExecutor,
) -> crate::apps::federation::transactions::repositories::admission::Scope<'a> {
	crate::apps::federation::transactions::repositories::admission::Scope { runtime, tx }
}

/// CLI, HTTP and recovery share the same storage, authority and participant adapters.
pub(crate) fn transaction_coordinator(
	runtime: &Federation,
) -> aidash_application::transactions::coordination::Coordinator {
	use crate::apps::federation::transactions::repositories::coordination::{
		Repository, Transport,
	};
	aidash_application::transactions::coordination::Coordinator::new(
		Arc::new(Repository {
			runtime: runtime.clone(),
		}),
		Arc::new(Transport {
			runtime: runtime.clone(),
		}),
	)
}

/// Management adapters share the existing authority, coordinator and control pools.
pub(crate) fn transaction_management_repository(
	runtime: &Federation,
) -> crate::apps::federation::transactions::repositories::management::Repository {
	crate::apps::federation::transactions::repositories::management::Repository {
		runtime: runtime.clone(),
	}
}

/// Mutation ports borrow the same serializable transaction and visibility gate.
pub(crate) fn transaction_mutation_scope<'a>(
	tx: &'a mut dyn reinhardt::db::backends::TransactionExecutor,
	node: &str,
) -> crate::apps::federation::transactions::repositories::mutation::Scope<'a> {
	crate::apps::federation::transactions::repositories::mutation::Scope(
		tx,
		Vec::new(),
		node.to_owned(),
	)
}

pub(crate) fn transaction_participant(
	runtime: &Federation,
) -> aidash_application::transactions::participation::Participant {
	aidash_application::transactions::participation::Participant::new(
		Arc::new(
			crate::apps::federation::transactions::repositories::participation::Repository {
				runtime: runtime.clone(),
				admitted: None,
				receiver: None,
			},
		),
		transaction_coordinator(runtime),
	)
}

/// The participant's side of a request an inbound Node made.
pub(crate) fn transaction_participant_as(
	runtime: &Federation,
	admitted: &aidash_application::federation::inbound::Admitted,
) -> aidash_application::transactions::participation::Participant {
	aidash_application::transactions::participation::Participant::new(
		Arc::new(
			crate::apps::federation::transactions::repositories::participation::Repository {
				runtime: runtime.clone(),
				admitted: Some(admitted.clone()),
				receiver: Some(admitted.receiver.to_string()),
			},
		),
		transaction_coordinator(runtime),
	)
}

/// Unreachable aborted history cannot consume active recovery's connection capacity.
pub(crate) async fn transaction_recovery_coordinators(
	runtime: &Federation,
) -> Result<(
	aidash_application::transactions::coordination::Coordinator,
	aidash_application::transactions::coordination::Coordinator,
)> {
	let aborted = runtime.for_recovery().await?;
	Ok((
		transaction_coordinator(runtime),
		transaction_coordinator(&aborted),
	))
}

pub(crate) fn operation_withdrawal_repository(
	store: &Store,
	scope: aidash_application::tenancy::TenancyScope,
) -> crate::apps::execution::repositories::withdrawal::Repository<'_> {
	crate::apps::execution::repositories::withdrawal::Repository { store, scope }
}

pub(crate) fn remote_command_scope<'a>(
	runtime: &'a Federation,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::remote_commands::Scope<'a> {
	crate::apps::identity::repositories::remote_commands::Scope {
		access,
		runtime,
		bindings: None,
	}
}

pub(crate) fn operation_runner(store: &Store) -> Result<aidash_integrations::runner::RunnerHttp> {
	let runner = store.capabilities.0.runner.as_ref().ok_or_else(|| {
		crate::Error::Conflict("RUNTIME_UNAVAILABLE: no isolated runner configured".into())
	})?;
	Ok(aidash_integrations::runner::RunnerHttp {
		endpoint: runner.endpoint.clone(),
		credential_env: runner.token_env.clone(),
		credentials: Arc::new(RunnerCredentials),
	})
}
struct RunnerCredentials;
impl aidash_application::ports::Credentials for RunnerCredentials {
	fn resolve(&self, reference: &str) -> aidash_application::Result<String> {
		std::env::var(reference).map_err(|_| {
			aidash_application::Error::Conflict(
				"RUNTIME_UNAVAILABLE: runner credential unavailable".into(),
			)
		})
	}
}
pub(crate) fn runner_health_profile(
	store: &Store,
) -> Result<aidash_domain::capabilities::operations::runner::HealthProfile> {
	let profile = &store.capabilities.0;
	let runner = profile.runner.as_ref().ok_or_else(|| {
		crate::Error::Conflict("RUNTIME_UNAVAILABLE: configure the isolated runner profile".into())
	})?;
	Ok(
		aidash_domain::capabilities::operations::runner::HealthProfile {
			image: runner.image.clone(),
			runtime_class: runner.runtime_class.clone(),
			cpu: profile.cpu,
			memory_bytes: profile.memory_bytes,
			processes: profile.processes,
			working_bytes: profile.working_bytes,
			temporary_bytes: profile.temporary_bytes,
		},
	)
}

pub(crate) fn operation_reconciliation_repository(
	store: &Store,
) -> crate::apps::execution::repositories::operations::reconciliation::Repository<'_> {
	crate::apps::execution::repositories::operations::reconciliation::Repository { store }
}

pub(crate) fn operation_processing_repository(
	store: &Store,
) -> crate::apps::execution::repositories::operations::reconciliation::Repository<'_> {
	operation_reconciliation_repository(store)
}
pub(crate) struct CapabilityJobs<'a> {
	store: &'a Store,
}
pub(crate) fn capability_background_jobs(store: &Store) -> CapabilityJobs<'_> {
	CapabilityJobs { store }
}
#[async_trait::async_trait]
impl aidash_application::ports::capabilities::processing::CapabilityBackgroundJobs
	for CapabilityJobs<'_>
{
	async fn network(
		&self,
		stopping: tokio::sync::watch::Receiver<bool>,
	) -> aidash_application::Result<()> {
		crate::apps::execution::capabilities::services::network::run(self.store.clone(), stopping)
			.await
			.map_err(Into::into)
	}
	async fn references(
		&self,
		stopping: tokio::sync::watch::Receiver<bool>,
	) -> aidash_application::Result<()> {
		crate::apps::execution::capabilities::services::references::run(
			self.store.clone(),
			stopping,
		)
		.await
		.map_err(Into::into)
	}
	async fn cleanup(
		&self,
		stopping: tokio::sync::watch::Receiver<bool>,
	) -> aidash_application::Result<()> {
		crate::apps::execution::capabilities::services::cleanup::run(self.store.clone(), stopping)
			.await
			.map_err(Into::into)
	}
	async fn reclamation(
		&self,
		stopping: tokio::sync::watch::Receiver<bool>,
	) -> aidash_application::Result<()> {
		crate::apps::execution::capabilities::services::reclamation::run(
			self.store.clone(),
			stopping,
		)
		.await
		.map_err(Into::into)
	}
}

pub(crate) fn outbound_repository(
	store: &Store,
) -> crate::apps::execution::repositories::outbound::Repository<'_> {
	crate::apps::execution::repositories::outbound::Repository { store }
}
pub(crate) fn outbound_transport() -> aidash_integrations::outbound::OutboundHttp {
	aidash_integrations::outbound::OutboundHttp
}

pub(crate) fn reference_repository(
	store: &Store,
) -> crate::apps::execution::repositories::references::Repository<'_> {
	crate::apps::execution::repositories::references::Repository { store }
}
pub(crate) fn reference_scope<'a>(
	store: Option<&'a Store>,
	access: &'a mut crate::authorization::access::Access,
	area: Option<&'a mut crate::apps::execution::capabilities::serializers::contracts::Area>,
) -> crate::apps::execution::repositories::references::Scope<'a> {
	use crate::apps::execution::repositories::references::{Authority, Scope};
	Scope {
		store,
		authority: Authority::Borrowed(access),
		area,
		pending: None,
	}
}

pub(crate) fn approval_scope<'a>(
	store: Option<&'a Store>,
	access: &'a mut crate::authorization::access::Access,
	area: Option<&'a crate::apps::execution::capabilities::serializers::contracts::Area>,
) -> crate::apps::execution::repositories::approvals::Scope<'a> {
	crate::apps::execution::repositories::approvals::Scope {
		store,
		access,
		area,
		run: None,
	}
}

pub(crate) fn reclamation_repository(
	store: &Store,
) -> crate::apps::execution::repositories::reclamation::Repository<'_> {
	crate::apps::execution::repositories::reclamation::Repository::new(store)
}

pub(crate) fn session_scope<'a>(
	store: Option<&'a Store>,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::execution::repositories::sessions::Scope<'a> {
	crate::apps::execution::repositories::sessions::Scope { store, access }
}

pub(crate) fn cleanup_scope<'a>(
	store: Option<&'a Store>,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::execution::repositories::cleanup::Scope<'a> {
	crate::apps::execution::repositories::cleanup::Scope { store, access }
}
pub(crate) fn retention_scope<'a>(
	store: Option<&'a Store>,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::execution::repositories::retention::Scope<'a> {
	crate::apps::execution::repositories::retention::Scope { store, access }
}
pub(crate) fn cleanup_repository(
	store: &Store,
) -> crate::apps::execution::repositories::cleanup::Repository<'_> {
	crate::apps::execution::repositories::cleanup::Repository { store }
}

pub(crate) fn python_scope<'a>(
	store: Option<&'a Store>,
	access: &'a mut crate::authorization::access::Access,
	run: Option<&'a crate::domain::Run>,
) -> crate::apps::execution::repositories::python::Scope<'a> {
	crate::apps::execution::repositories::python::Scope { store, access, run }
}
pub(crate) fn python_repository(
	store: &Store,
) -> crate::apps::execution::repositories::python::Repository<'_> {
	crate::apps::execution::repositories::python::Repository { store }
}
pub(crate) fn package_scope<'a>(
	store: &'a Store,
	access: &'a mut crate::authorization::access::Access,
	run: Option<&'a crate::domain::Run>,
) -> crate::apps::execution::repositories::packages::Scope<'a> {
	crate::apps::execution::repositories::packages::Scope { store, access, run }
}

pub(crate) fn file_scope<'a>(
	store: Option<&'a Store>,
	access: &'a mut crate::authorization::access::Access,
	run: Option<&'a crate::domain::Run>,
) -> crate::apps::execution::repositories::files::Scope<'a> {
	crate::apps::execution::repositories::files::Scope {
		store,
		access,
		run,
		pending: None,
	}
}

pub(crate) fn capability_configuration_scope<'a>(
	store: &'a Store,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::execution::repositories::configuration::Scope<'a> {
	crate::apps::execution::repositories::configuration::Scope { store, access }
}

pub(crate) fn skill_headroom(
	store: &Store,
) -> crate::apps::execution::repositories::skills::Headroom<'_> {
	crate::apps::execution::repositories::skills::Headroom { store }
}

pub(crate) fn transfer_repository(
	federation: &crate::federation::Federation,
) -> crate::apps::execution::repositories::transfer::Repository<'_> {
	crate::apps::execution::repositories::transfer::Repository {
		federation,
		receiver: None,
	}
}
/// A transfer arriving at the Node `receiver`.
pub(crate) fn transfer_receiver_repository(
	federation: &crate::federation::Federation,
	receiver: aidash_domain::nodes::NodeName,
) -> crate::apps::execution::repositories::transfer::Repository<'_> {
	crate::apps::execution::repositories::transfer::Repository {
		federation,
		receiver: Some(crate::apps::execution::repositories::transfer::Receiver {
			address: federation
				.config
				.deployment
				.node(receiver.clone())
				.to_string(),
			node: receiver,
		}),
	}
}
pub(crate) fn transfer_scope<'a>(
	store: Option<&'a Store>,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::execution::repositories::transfer::Scope<'a> {
	crate::apps::execution::repositories::transfer::Scope {
		store,
		authority: crate::apps::execution::repositories::transfer::Authority::Borrowed(access),
		receiving: None,
		pending: None,
	}
}

pub(crate) fn semantic_journal_repository(
	store: &Store,
	scope: aidash_application::tenancy::TenancyScope,
) -> crate::apps::knowledge::repositories::remote_journal::Repository<'_> {
	crate::apps::knowledge::repositories::remote_journal::Repository { store, scope }
}
pub(crate) fn semantic_journal_scope<'a>(
	store: &'a Store,
	tx: &'a mut crate::database::native::Transaction,
) -> crate::apps::knowledge::repositories::remote_journal::Scope<'a> {
	crate::apps::knowledge::repositories::remote_journal::Scope {
		store,
		transaction: crate::apps::knowledge::repositories::remote_journal::Transaction::Borrowed(
			tx,
		),
	}
}

/// Receiver HTTP, Home callbacks and worker refresh share current authority and persistence.
pub(crate) fn peer_admission_repository(
	f: &crate::federation::Federation,
) -> crate::apps::identity::repositories::peer_admission::Repository<'_> {
	crate::apps::identity::repositories::peer_admission::Repository { runtime: f }
}
pub(crate) fn peer_admission_records(
	store: &crate::store::Store,
) -> crate::apps::identity::repositories::peer_admission::Records<'_> {
	crate::apps::identity::repositories::peer_admission::Records { store }
}
pub(crate) fn peer_inspection_scope<'a>(
	f: &'a crate::federation::Federation,
	access: &'a mut crate::authorization::access::Access,
	receiver: aidash_domain::nodes::NodeName,
) -> crate::apps::identity::repositories::peer_admission::Borrowed<'a> {
	crate::apps::identity::repositories::peer_admission::Scope {
		runtime: f,
		access,
		receiver,
	}
}

pub(crate) fn semantic_status_scope<'a>(
	store: Option<&'a Store>,
	access: &'a mut crate::authorization::access::Access,
	node_id: &'a str,
) -> crate::apps::knowledge::repositories::remote_status::Scope<'a> {
	crate::apps::knowledge::repositories::remote_status::Scope {
		store,
		access,
		node_id,
	}
}
pub(crate) fn semantic_status_repository(
	store: &Store,
	scope: aidash_application::tenancy::TenancyScope,
) -> crate::apps::knowledge::repositories::remote_status::Repository<'_> {
	crate::apps::knowledge::repositories::remote_status::Repository { store, scope }
}

pub(crate) fn peer_mapping_repository(
	runtime: &crate::federation::Federation,
) -> crate::apps::identity::repositories::peer_mappings::Repository<'_> {
	crate::apps::identity::repositories::peer_mappings::Repository {
		runtime,
		administrator: None,
	}
}

/// Borrow the current draft authority transaction for HTTP and contained workers.
pub(crate) fn draft_authority_scope<'a>(
	tx: &'a mut dyn reinhardt::db::backends::TransactionExecutor,
	actor: &'a crate::authorization::identity::Actor,
	policy: Option<aidash_application::ports::authorization::dashboard::AccountPolicy>,
) -> crate::apps::registry::workbench::repositories::authority::Scope<'a> {
	crate::apps::registry::workbench::repositories::authority::Scope { tx, actor, policy }
}
/// HTTP edits assemble the same repository used by application draft workflows.
pub(crate) fn draft_repository(
	runtime: &crate::federation::Federation,
	actor: crate::authorization::identity::Actor,
) -> crate::apps::registry::workbench::repositories::drafts::Repository {
	crate::apps::registry::workbench::repositories::drafts::Repository {
		runtime: runtime.clone(),
		actor,
	}
}

/// Source and receiver authority RPCs share fresh peer trust and bounded integration transport.
pub(crate) fn authority_peer_client(
	runtime: &crate::federation::Federation,
	sender: &aidash_domain::federation::route::Sender,
) -> aidash_application::federation::authority::Client {
	aidash_application::federation::authority::Client::new(
		Arc::new(crate::apps::federation::remote::repositories::Repository {
			federation: runtime.clone(),
			sender: sender.clone(),
		}),
		Arc::new(peer_transport(runtime)),
	)
}

/// Home commands and native worker delegation share current source authority and persistence.
pub(crate) fn home_execution_repository(
	runtime: &Federation,
	actor: crate::authorization::identity::Actor,
) -> crate::apps::identity::repositories::home_execution::Repository {
	crate::apps::identity::repositories::home_execution::Repository {
		runtime: runtime.clone(),
		actor: Some(actor),
	}
}
/// A peer-authenticated request has no local actor; its authority is the grant it names.
pub(crate) fn peer_home_execution_repository(
	runtime: &Federation,
) -> crate::apps::identity::repositories::home_execution::Repository {
	crate::apps::identity::repositories::home_execution::Repository {
		runtime: runtime.clone(),
		actor: None,
	}
}

/// Borrow source policy and credential locks across the shared application checks.
pub(crate) fn source_authority_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::remote_grants::authority::Scope<'_> {
	crate::apps::identity::repositories::remote_grants::authority::Scope { access }
}

/// Required Home disclosure shares the grant's current policy, credential and index lease.
pub(crate) fn source_semantic_binding_scope<'a>(
	runtime: &'a Federation,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::remote_grants::semantic::Scope<'a> {
	crate::apps::identity::repositories::remote_grants::semantic::Scope { runtime, access }
}

/// Current source readers retain their native transaction and own temporary dependency collections.
pub(crate) fn source_read_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::remote_grants::reads::Scope<'_> {
	crate::apps::identity::repositories::remote_grants::reads::Scope {
		access,
		owned_frontier: false,
	}
}

/// Verify disclosed sources within the caller's current native authority transaction.
pub(crate) fn source_semantic_provenance_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::remote_grants::provenance::Scope<'_> {
	crate::apps::identity::repositories::remote_grants::provenance::Scope { access }
}

/// Home semantic disclosure shares the production journal, dispatch, settlement and provider ports.
pub(crate) fn source_semantic_search_repository<'a>(
	runtime: &'a Federation,
	home: &aidash_domain::nodes::NodeAddress,
	scope: aidash_application::tenancy::TenancyScope,
) -> crate::apps::identity::repositories::remote_grants::search::Repository<'a> {
	crate::apps::identity::repositories::remote_grants::search::Repository {
		runtime,
		home: home.to_string(),
		journal: semantic_journal_repository(&runtime.store, scope.clone()),
		dispatch: crate::apps::execution::generation::repositories::dispatch::NativeDispatch {
			store: runtime.store.clone(),
			scope,
		},
		settlement: crate::apps::execution::generation::repositories::dispatch::NativeSettlement {
			runtime: runtime.clone(),
		},
		transport: semantic_transport(&runtime.store),
	}
}

/// Source snapshots and HTTP adapters retain one current grant, snapshot and audit transaction.
pub(crate) fn source_snapshot_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::remote_grants::snapshot::Scope<'_> {
	crate::apps::identity::repositories::remote_grants::snapshot::Scope { access }
}

/// Workbench factual history shares current draft authority and native ORM transactions.
pub(crate) fn workbench_audit_repository(
	runtime: &Federation,
	actor: crate::authorization::identity::Actor,
) -> crate::apps::registry::workbench::repositories::audit::Repository<'_> {
	crate::apps::registry::workbench::repositories::audit::Repository { runtime, actor }
}

/// Component permission context shares native effective definitions and current policy locks.
pub(crate) fn workbench_permission_repository(
	runtime: &Federation,
	actor: crate::authorization::identity::Actor,
) -> crate::apps::registry::workbench::repositories::permissions::Repository<'_> {
	crate::apps::registry::workbench::repositories::permissions::Repository { runtime, actor }
}

/// Inspection reuses the native audit lease across all contained usage and evidence reads.
pub(crate) fn workbench_inspection_repository(
	runtime: &Federation,
	actor: crate::authorization::identity::Actor,
) -> crate::apps::registry::workbench::repositories::inspection::Repository<'_> {
	crate::apps::registry::workbench::repositories::inspection::Repository { runtime, actor }
}

/// Incident commands share native identity and record locks with atomic history.
pub(crate) fn workbench_incident_repository(
	runtime: &Federation,
	actor: crate::authorization::identity::Actor,
) -> crate::apps::registry::workbench::repositories::incidents::Repository<'_> {
	crate::apps::registry::workbench::repositories::incidents::Repository { runtime, actor }
}
/// Retention reuses the native locked batch and commits copied payload disposal atomically.
pub(crate) fn workbench_incident_retention_repository(
	pool: &crate::database::native::Pool,
) -> crate::apps::registry::workbench::repositories::incidents::RetentionRepository<'_> {
	crate::apps::registry::workbench::repositories::incidents::RetentionRepository { pool }
}

/// Report sources share the independent native inspection, permission and incident adapters.
pub(crate) fn workbench_report_sources(
	runtime: &Federation,
	actor: crate::authorization::identity::Actor,
) -> crate::apps::registry::workbench::repositories::report::Sources<'_> {
	crate::apps::registry::workbench::repositories::report::Sources { runtime, actor }
}

/// Profile workflows use the existing draft and profile transaction boundaries.
pub(crate) fn workbench_profile_repository(
	runtime: &Federation,
	actor: crate::authorization::identity::Actor,
) -> crate::apps::registry::workbench::repositories::profile::Repository<'_> {
	crate::apps::registry::workbench::repositories::profile::Repository { runtime, actor }
}
/// Test endpoints reach the Deployment's permitted private origins only.
pub(crate) fn workbench_profile_configuration(
	runtime: &Federation,
) -> crate::apps::registry::workbench::repositories::profile::Configuration {
	crate::apps::registry::workbench::repositories::profile::Configuration {
		keyless: runtime.store.keyless.clone(),
	}
}

/// Sandbox HTTP and background callers share the native persistence and current actor.
pub(crate) fn workbench_sandbox_repository(
	runtime: &Federation,
	actor: crate::authorization::identity::Actor,
) -> crate::apps::registry::workbench::repositories::sandbox::Repository {
	crate::apps::registry::workbench::repositories::sandbox::Repository {
		store: runtime.store.clone(),
		node_id: runtime.config.deployment.to_string(),
		tenant_actions:
			crate::apps::registry::workbench::repositories::authority::TenantActions::new(
				actor.clone(),
				runtime.config.deployment.to_string(),
			),
		actor,
	}
}

/// Isolated test HTTP requests retain their original timeout, redirect and
/// response limits, and reach public addresses unless the origin is permitted.
pub(crate) fn workbench_sandbox_real_tools(
	runtime: &Federation,
) -> Result<aidash_integrations::sandbox::RealTools> {
	Ok(aidash_integrations::sandbox::RealTools::new(
		runtime.store.keyless.clone(),
	)?)
}

/// Assemble owned adapters for a background sandbox session with the admitted actor's identity.
pub(crate) fn workbench_sandbox_execution(
	runtime: &Federation,
	actor: crate::authorization::identity::Actor,
) -> Result<aidash_application::registry::workbench::sandbox::execution::Execution> {
	Ok(
		aidash_application::registry::workbench::sandbox::execution::Execution {
			repository: Arc::new(workbench_sandbox_repository(runtime, actor)),
			configuration: Arc::new(workbench_profile_configuration(runtime)),
			transport: Arc::new(workbench_sandbox_real_tools(runtime)?),
		},
	)
}

/// Sandbox models retain Keyless access until they have Tenant admission authority.
pub(crate) struct WorkbenchSandboxModels {
	clients: aidash_integrations::keyless::Clients,
	keyless: Arc<dyn aidash_application::provider_access::ProviderAccess>,
}
impl aidash_application::ports::registry::workbench::sandbox::admission::SandboxModels
	for WorkbenchSandboxModels
{
	fn provider(
		&self,
		model: aidash_domain::model::ModelConfig,
	) -> aidash_application::Result<Arc<dyn aidash_application::ports::ModelProvider>> {
		// Sandbox requests are always Legacy, so they never need a salt.
		if model.provider_credential.is_some() {
			return Err(aidash_application::Error::Invalid(
				"Workbench tests do not support Tenant Provider Credentials".into(),
			));
		}
		aidash_integrations::inference::provider(
			self.clients.clone(),
			model,
			self.keyless.clone(),
			Default::default(),
		)
	}
}
pub(crate) fn workbench_sandbox_models(runtime: &Federation) -> WorkbenchSandboxModels {
	WorkbenchSandboxModels {
		clients: aidash_integrations::keyless::Clients {
			direct: runtime.client.clone(),
			permitted: runtime.store.provider_clients.permitted.clone(),
			public: runtime.store.provider_clients.public.clone(),
		},
		keyless: keyless_provider_access(&runtime.store),
	}
}

/// Notification and database recovery borrow the caller's same native transaction.
pub(crate) fn activation_scheduling_scope(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
) -> crate::apps::execution::activation::repositories::scheduling::Scope<
	&mut dyn reinhardt::db::backends::TransactionExecutor,
> {
	crate::apps::execution::activation::repositories::scheduling::Scope {
		tx,
		capabilities: aidash_domain::run_state::PersistedCapability::supported(),
	}
}

pub(crate) fn activation_repository(
	harness: Harness,
) -> crate::apps::execution::activation::repositories::Repository {
	crate::apps::execution::activation::repositories::Repository { harness }
}

pub(crate) fn activation_broker_configuration(
	settings: &crate::activation::Settings,
) -> aidash_integrations::activation::Configuration {
	aidash_integrations::activation::Configuration {
		namespace: settings.namespace.clone(),
		bootstrap: settings.bootstrap,
		max_age: settings.max_age,
		max_bytes: settings.max_bytes,
		replicas: settings.replicas,
	}
}

/// Server and listener-free worker modes use identical persistence and transport ports.
pub(crate) fn activation_driver(
	mut federation: Federation,
	settings: crate::activation::Settings,
	worker: bool,
) -> Arc<aidash_harness::activation::Runtime> {
	if let Ok(url) = std::env::var("AIDASH_ACTIVATION_NATS_URL") {
		federation.config.nats_url = url;
	}
	let connector = aidash_integrations::activation::Connector::new(
		federation.config.nats_url.clone(),
		federation.config.deployment.to_string(),
		activation_broker_configuration(&settings),
		worker,
	);
	aidash_harness::activation::Runtime::new(
		Arc::new(activation_repository(Harness { federation })),
		Arc::new(connector),
		aidash_harness::activation::Settings {
			recovery: settings.recovery,
			fallback: settings.fallback,
			test_pause_file: settings.test_pause_file,
			test_after_ack_pause_file: settings.test_after_ack_pause_file,
		},
		worker,
	)
}

/// Native worker ports share the same store as HTTP without starting a listener.
pub(crate) fn worker_leases(store: &Store) -> crate::apps::execution::repositories::worker::Leases {
	crate::apps::execution::repositories::worker::Leases {
		store: store.clone(),
	}
}

pub(crate) fn terminal_repository(
	federation: &Federation,
) -> crate::apps::execution::repositories::worker::Terminal {
	crate::apps::execution::repositories::worker::Terminal {
		federation: federation.clone(),
	}
}

pub(crate) fn worker_step(
	federation: &Federation,
	run: aidash_domain::Run,
	visibility: crate::transactions::gate::ReadLease,
) -> Box<dyn aidash_application::ports::execution::worker::WorkerStep> {
	Box::new(crate::apps::execution::repositories::worker::Step {
		guard: None,
		run,
		federation: federation.clone(),
		recovery: recovery_store(&federation.store),
		_active: crate::http::ActiveExecution::begin(),
		visibility,
	})
}

pub(crate) fn run_message_scope<'a>(
	federation: &'a Federation,
	run: &aidash_domain::RunMetadata,
) -> crate::apps::federation::remote::repositories::run_messages::Messages<'a> {
	crate::apps::federation::remote::repositories::run_messages::Messages {
		federation,
		node: format!("{}/{}", federation.config.deployment, run.node),
		run: run.clone(),
		execution: None,
	}
}

pub(crate) fn execution_message_scope<'a>(
	federation: &'a Federation,
	run: &'a aidash_domain::Run,
) -> crate::apps::federation::remote::repositories::run_messages::Messages<'a> {
	let mut scope = run_message_scope(federation, &run.metadata());
	scope.execution = Some(run);
	scope
}

pub(crate) fn execution_headroom(
	federation: &Federation,
) -> crate::apps::federation::remote::repositories::headroom::Context<'_> {
	crate::apps::federation::remote::repositories::headroom::Context(federation)
}

pub(crate) fn oidc_settings(
	config: &crate::config::OidcConfig,
) -> aidash_integrations::oidc::Settings {
	aidash_integrations::oidc::Settings {
		issuer: config.issuer.clone(),
		client_id: config.client_id.clone(),
		client_secret: config.client_secret.clone(),
	}
}

pub(crate) fn dashboard_authority(
	federation: &Federation,
) -> aidash_application::authorization::dashboard::DashboardAuthority {
	aidash_application::authorization::dashboard::DashboardAuthority {
		accounts: Arc::new(crate::apps::identity::repositories::dashboard::Repository(
			federation.clone(),
		)),
		status: federation
			.config
			.sign_in
			.as_ref()
			.map(|sign_in| sign_in.status()),
	}
}

pub(crate) fn dashboard_logout(
	federation: &Federation,
) -> crate::apps::identity::repositories::dashboard::Logout<'_> {
	crate::apps::identity::repositories::dashboard::Logout(federation)
}

pub(crate) fn dashboard_login(
	pool: crate::database::native::Pool,
	node: &str,
) -> crate::apps::identity::repositories::dashboard::Login {
	crate::apps::identity::repositories::dashboard::Login(pool, node.into())
}

pub(crate) fn graph_visibility<'a, 'scope>(
	authority: &'a mut crate::apps::identity::repositories::graph::persistence::GraphAuthority<
		'scope,
	>,
) -> crate::apps::identity::repositories::graph::Visibility<'a, 'scope> {
	crate::apps::identity::repositories::graph::Visibility(authority)
}

/// Assemble the desktop broker's native persistence behind its application port.
pub(crate) fn desktop_protocol(
	runtime: &Federation,
) -> impl aidash_application::ports::authorization::desktop::DesktopProtocol {
	crate::apps::identity::repositories::desktop::Repository(runtime.clone())
}

/// The Deployment Key `[deployment.key]` names, or none when it is omitted. A
/// file is read here, and a Cloud KMS key version is read through the build's
/// extension, which only the Enterprise build implements. The error names the
/// reason and never the key.
pub async fn configure_deployment_key(
	settings: Option<&crate::apps::operations::serializers::deployment::DeploymentKeySettings>,
	base_dir: &std::path::Path,
) -> Result<Option<Arc<dyn aidash_application::federation::key::DeploymentKey>>> {
	use crate::apps::operations::serializers::deployment::DeploymentKeySource;
	let Some(settings) = settings else {
		return Ok(None);
	};
	let key: Arc<dyn aidash_application::federation::key::DeploymentKey> =
		match settings.source().map_err(Error::Invalid)? {
			DeploymentKeySource::File(file) => Arc::new(
				aidash_integrations::federation::key::FileDeploymentKey::load(
					&base_dir.join(file),
				)?,
			),
			DeploymentKeySource::Kms(kms) => build::current().extension.deployment_key(kms).await?,
		};
	tracing::info!(key_id = %key.key_id(), "Deployment Key available");
	Ok(Some(key))
}

/// Load privately supplied keys only at shared server/worker bootstrap.
pub async fn configure_provider_credentials(
	store: &mut Store,
	settings: &crate::apps::identity::serializers::provider_credentials::Settings,
) -> Result<()> {
	use crate::apps::identity::serializers::provider_credentials::StoreConfig;
	use reinhardt::conf::settings::{fragment::SettingsValidation, profile::Profile};
	settings
		.validate(&Profile::parse("local"))
		.map_err(|e| Error::Invalid(e.to_string()))?;
	let Some(config) = &settings.store else {
		return Ok(());
	};
	let fingerprint_key = settings.load_fingerprint_key().await?;
	type WriteStore = Arc<dyn aidash_application::provider_credentials::Store>;
	type Reader = Arc<dyn aidash_application::provider_access::KeyMaterialReader>;
	let (adapter, reader): (WriteStore, Option<Reader>) = match config {
		StoreConfig::Enterprise(fields) => {
			let provided = build::current()
				.extension
				.provider_credential_store(fields, settings.broker.as_ref())?;
			store.capability_issuer = provided.issuer;
			(provided.store, None)
		}
		StoreConfig::Postgres { .. } => {
			let (current, retired) = config.load_postgres_keys().await?;
			let adapter = Arc::new(
				crate::apps::identity::repositories::secret_store::PostgresStore::new(
					store.control_pool.clone(),
					current,
					retired,
				)
				.await?,
			);
			(adapter.clone(), Some(adapter as Reader))
		}
	};
	let client = aidash_integrations::semantic::client()?;
	store.provider_credentials = Some(Arc::new(
		aidash_application::provider_credentials::Service {
			repository: Arc::new(
				crate::apps::identity::repositories::provider_credentials::NativeRepository {
					pool: store.control_pool.clone(),
					node: store.deployment.to_string(),
				},
			),
			store: adapter,
			validator: Arc::new(
				aidash_integrations::provider_credentials::OpenRouterKeyValidator {
					client: client.clone(),
				},
			),
			exchange: Arc::new(
				aidash_integrations::provider_authorization::OpenRouterAuthorization::new(client),
			),
			fingerprint_key,
			max_per_tenant: settings.max_per_tenant,
		},
	));
	store.provider_key_material_reader = reader;
	Ok(())
}

/// Plan Tokens live only in a PostgreSQL Secret Store (ADR 0008):
/// Cloud and Dedicated wait for OpenAI's approval of a hosted flow (#248), so a
/// server refuses their enablement at the settings boundary regardless of its
/// Store. Self-hosted also refuses enablement without a PostgreSQL Store.
/// The service exists whenever the Store could hold Plan Tokens, so owners
/// can still read and disconnect their connections
/// after `enabled` is turned off (ADR 0007).
pub fn configure_plan_connections(
	store: &mut Store,
	credentials: &crate::apps::identity::serializers::provider_credentials::Settings,
	settings: &crate::apps::identity::serializers::plan_connections::PlanConnectionSettings,
) -> Result<()> {
	use crate::apps::identity::serializers::provider_credentials::StoreConfig;
	let postgres = matches!(credentials.store, Some(StoreConfig::Postgres { .. }));
	if settings.enabled && !postgres {
		return Err(Error::Invalid(
			"Plan Connections need a PostgreSQL Secret Store; \
			 turn off [plan_connections] enabled"
				.into(),
		));
	}
	store.plan_connections_enabled = settings.enabled;
	let (Some(credentials), Some(reader), true) = (
		store.provider_credentials.as_ref(),
		store.provider_key_material_reader.clone(),
		postgres,
	) else {
		return Ok(());
	};
	store.plan_connections = Some(Arc::new(aidash_application::plan_connections::Service {
		repository: Arc::new(
			crate::apps::identity::repositories::plan_connections::NativeRepository {
				pool: store.control_pool.clone(),
				node: store.deployment.to_string(),
			},
		),
		store: credentials.store.clone(),
		reader,
		authorization: Arc::new(aidash_integrations::chatgpt::ChatgptAuthorization::new(
			aidash_integrations::semantic::client()?,
		)),
	}));
	if settings.enabled {
		tracing::info!("Plan Connections enabled");
	}
	Ok(())
}

/// The most Tenant Secrets one Tenant may hold.
const TENANT_SECRETS_PER_TENANT: usize = 64;

/// Tenant Secrets exist wherever the Store holds their values: a self-hosted
/// PostgreSQL Store reads them locally, and a Secret Manager Store (Cloud,
/// Dedicated) routes calls through its Tenant Secret broker. Without the
/// broker, a Secret Manager Store keeps them unavailable (#310).
pub fn configure_tenant_secrets(
	store: &mut Store,
	settings: &crate::apps::identity::serializers::provider_credentials::Settings,
	role: ProcessRole,
) -> Result<()> {
	use crate::apps::identity::serializers::provider_credentials::StoreConfig;
	let Some(credentials) = store.provider_credentials.as_ref() else {
		return Ok(());
	};
	let (reader, calls) = match &settings.store {
		Some(StoreConfig::Postgres { .. }) => {
			let Some(reader) = store.provider_key_material_reader.clone() else {
				return Ok(());
			};
			(Some(reader), None)
		}
		Some(StoreConfig::Enterprise(_)) => {
			if !settings.offers_tenant_secrets() {
				return Ok(());
			}
			let Some(calls) = build::current().extension.tenant_secret_calls(
				settings.broker.as_ref(),
				settings.tenant_secret_broker.as_ref(),
				role == ProcessRole::Worker,
			)?
			else {
				return Ok(());
			};
			(None, Some(calls))
		}
		None => return Ok(()),
	};
	store.tenant_secrets = Some(Arc::new(aidash_application::tenant_secrets::Service {
		repository: Arc::new(
			crate::apps::identity::repositories::tenant_secrets::NativeRepository {
				pool: store.control_pool.clone(),
			},
		),
		store: credentials.store.clone(),
		reader,
		calls,
		fingerprint_key: credentials.fingerprint_key.clone(),
		max_per_tenant: TENANT_SECRETS_PER_TENANT,
	}));
	Ok(())
}

/// Worker inference is bound to local admission evidence, never a mutable binding.
/// It carries this node's Cache Salt Keys; only requests with a Cache Scope
/// (salted Projection Versions) use them.
pub(crate) fn admitted_model_provider(
	store: &Store,
	config: aidash_domain::model::ModelConfig,
	run: Option<uuid::Uuid>,
	tenant: String,
	maintenance: Option<aidash_application::provider_access::MaintenancePurpose>,
) -> Result<Arc<dyn aidash_application::ports::ModelProvider>> {
	aidash_integrations::inference::salted_provider(
		store.provider_clients.clone(),
		config,
		Arc::new(
			crate::apps::identity::repositories::provider_credentials::AdmittedAccess {
				store: store.clone(),
			},
		),
		aidash_application::provider_access::Context {
			tenant,
			run,
			maintenance,
			observation: None,
			provider_credential_id: None,
			inference: None,
		},
		store.cache_salt.clone(),
	)
	.map_err(Into::into)
}
pub(crate) fn admitted_semantic_transport(
	store: &Store,
	run: Option<uuid::Uuid>,
	tenant: String,
	maintenance: Option<aidash_application::provider_access::MaintenancePurpose>,
) -> crate::apps::knowledge::repositories::postgres_vector::Transport {
	let mut transport = semantic_transport(store);
	transport.embedding.access = Arc::new(
		crate::apps::identity::repositories::provider_credentials::AdmittedAccess {
			store: store.clone(),
		},
	);
	transport.embedding.context = aidash_application::provider_access::Context {
		tenant,
		run,
		maintenance,
		observation: None,
		provider_credential_id: None,
		inference: None,
	};
	transport
}
