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
use reinhardt::db::orm::connection::DatabaseConnectionLease;
use reinhardt::server::ShutdownCoordinator;
use std::{sync::Arc, time::Duration};
use tokio::{
	sync::{Notify, watch},
	task::JoinHandle,
};

/// Assemble management adapters before any runtime settings are resolved.
pub fn management_commands() -> reinhardt::commands::CommandRegistry {
	let mut registry = reinhardt::commands::CommandRegistry::new();
	registry.register_capability(Box::new(
		crate::apps::execution::services::worker::RunWorker,
	));
	registry.register_capability(Box::new(
		crate::apps::execution::services::schema::ApiContract,
	));
	registry
}

/// Populate the same request container used by native runserver and HTTP fixtures.
pub async fn initialize(
	context: &InjectionContext,
	settings: &ProjectSettings,
	connection: BackendConnection,
) -> Result<Federation> {
	let config = Config::from_settings(settings)?;
	let pool = connection
		.into_postgres()
		.ok_or_else(|| Error::Invalid("Aidash requires PostgreSQL".into()))?;
	let store = Store::from_pool(pool, config.node_id.clone()).await?;
	let registry = Registry::new(store.pool.clone(), &store.node_id)?;
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(120))
		.connect_timeout(Duration::from_secs(10))
		.redirect(reqwest::redirect::Policy::none())
		.build()?;
	let federation = Federation {
		store,
		registry,
		config,
		client,
		notify: Arc::new(Notify::new()),
	};
	let lease = DatabaseConnectionLease::register(connection)?;
	context.set_singleton(lease.handle());
	context.set_singleton(lease);
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
		tasks.spawn_service(runtime_task(
			crate::apps::execution::services::metrics::run(coordinator.clone()),
		));
		if let Some(service) = event_streams.clone() {
			let url = federation.config.nats_url.clone();
			let node = federation.config.node_id.clone();
			let stopping = receiver.clone();
			tasks.spawn_service(runtime_task(async move {
				service.run(&url, &node, stopping).await
			}));
		}
		// Acquire pools before detaching tasks. A startup error drops both JoinSets.
		let coordinator_runtime = federation.for_recovery().await?;
		let participant_runtime = federation.for_recovery().await?;
		let worker_runtime = federation.for_runtime_workers().await?;
		tasks.spawn_service(runtime_task(crate::transactions::coordinator::run(
			coordinator_runtime,
		)));
		tasks.spawn_service(runtime_task(crate::transactions::participant::run(
			participant_runtime,
		)));
		tasks.spawn_service(aidash_runtime::generation::run(
			Arc::new(generation_provisioning_repository(&worker_runtime)),
			registry_validation(),
		));
		tasks.spawn_service(runtime_task(crate::semantic::worker::run(
			worker_runtime.clone(),
		)));
		if event_streams.is_some() {
			tasks.spawn_service(runtime_task(EventBus::run(federation.clone())));
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
		if federation.config.oidc.is_some() {
			tasks.spawn_service(runtime_task(crate::dashboard_auth::refresh_active(
				federation.clone(),
				receiver.clone(),
			)));
		}
		if event_streams.is_some() {
			let deliveries = federation.clone();
			tasks.spawn_service(runtime_task(async move {
				loop {
					if let Err(error) = deliveries.retry_deliveries().await {
						tracing::warn!(%error, "delegation retry failed");
					}
					tokio::time::sleep(Duration::from_millis(250)).await;
				}
			}));
		}
		let retention = federation.store.pool.clone();
		tasks.spawn_service(runtime_task(async move {
			loop {
				if let Err(error) = crate::workbench::purge_expired(&retention).await {
					tracing::warn!(%error, "agent test retention cleanup failed");
				}
				if let Err(error) = crate::workbench::purge_incident_evidence(&retention).await {
					tracing::warn!(%error, "incident evidence retention cleanup failed");
				}
				tokio::time::sleep(Duration::from_secs(3600)).await;
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
pub fn authorization(pool: sqlx::PgPool) -> aidash_application::authorization::Authorization {
	aidash_application::authorization::Authorization::new(Arc::new(
		crate::apps::identity::repositories::PolicyRepository { pool },
	))
}

/// External adapters are assembled at the process boundary.
pub fn model_catalog(client: reqwest::Client) -> Arc<dyn aidash_application::ports::ModelCatalog> {
	Arc::new(aidash_integrations::openrouter::OpenRouterCatalog::new(
		client,
	))
}

mod compatibility;
pub use compatibility::{migrate, serve};

/// All inference paths use the same credential resolver and application port.
pub fn model_provider(
	client: reqwest::Client,
	config: aidash_domain::model::ModelConfig,
) -> Result<Arc<dyn aidash_application::ports::ModelProvider>> {
	aidash_integrations::inference::provider(client, config, Arc::new(EnvironmentCredentials))
		.map_err(Into::into)
}

struct EnvironmentCredentials;
impl aidash_application::ports::Credentials for EnvironmentCredentials {
	fn resolve(&self, reference: &str) -> aidash_application::Result<String> {
		crate::config::secret(reference).map_err(Into::into)
	}
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

/// Bind the classifier to the same rotating credential resolver as inference.
pub fn compaction_transport(
	client: reqwest::Client,
	endpoint: String,
	model: String,
	credential_env: String,
) -> Result<aidash_integrations::compaction::JevClient> {
	aidash_integrations::compaction::JevClient::new(
		client,
		endpoint,
		model,
		credential_env,
		Arc::new(EnvironmentCredentials),
	)
	.map_err(Into::into)
}

/// Semantic transports share rotating credentials with inference and compaction.
pub fn semantic_transport(
	client: reqwest::Client,
) -> aidash_integrations::semantic::SemanticClient {
	aidash_integrations::semantic::SemanticClient {
		client,
		credentials: Arc::new(EnvironmentCredentials),
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
		database: store.orm_connection()?,
	})
}

/// HTTP and MCP tools resolve credentials for each invocation, including rotation.
pub(crate) fn tool_transport(client: reqwest::Client) -> aidash_integrations::tools::HttpTools {
	aidash_integrations::tools::HttpTools {
		client,
		credentials: Arc::new(EnvironmentCredentials),
	}
}

/// Discovery and worker retries share the native repository and rotating keys.
pub(crate) fn federation(f: &Federation) -> aidash_application::federation::Federation {
	aidash_application::federation::Federation::new(
		f.config.node_id.clone(),
		Arc::new(crate::apps::federation::remote::repositories::Repository {
			federation: f.clone(),
		}),
		Arc::new(peer_transport(f)),
	)
}

pub(crate) fn peer_transport(f: &Federation) -> aidash_integrations::federation::PeerHttp {
	aidash_integrations::federation::PeerHttp {
		client: f.client.clone(),
		node_id: f.config.node_id.clone(),
		protocol_version: crate::config::PROTOCOL_VERSION.into(),
		credentials: Arc::new(PeerCredentials),
	}
}

struct PeerCredentials;
impl aidash_application::ports::Credentials for PeerCredentials {
	fn resolve(&self, reference: &str) -> aidash_application::Result<String> {
		crate::config::peer_secret(reference).map_err(Into::into)
	}
}

/// Assemble definition admission for both management and worker paths.
pub fn registry_validation() -> aidash_application::registry::DefinitionValidation {
	aidash_application::registry::DefinitionValidation::new(
		Arc::new(EnvironmentCredentials),
		Arc::new(NativeCoreToolCatalog),
	)
}
struct NativeCoreToolCatalog;
impl aidash_application::ports::registry::CoreToolCatalog for NativeCoreToolCatalog {
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

/// Keep conversation authorization, writes, and delegation in one borrowed scope.
pub(crate) fn conversation_scope<'a>(
	federation: &'a Federation,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::workspaces::repositories::conversations::NativeConversation<'a> {
	crate::apps::workspaces::repositories::conversations::NativeConversation::new(
		federation, access,
	)
}

/// Operator conversations use the same Store and transaction implementation.
pub(crate) fn operator_conversations(
	federation: &Federation,
) -> crate::apps::workspaces::repositories::conversations::NativeOperatorConversations {
	crate::apps::workspaces::repositories::conversations::NativeOperatorConversations(
		federation.clone(),
	)
}

/// Channel adapters retain the caller's admission locks and physical transaction.
pub(crate) fn channel_scope<'a>(
	store: &'a Store,
	lease: &'a mut crate::collaboration::access::Lease,
) -> crate::apps::workspaces::repositories::channels::NativeChannels<'a> {
	crate::apps::workspaces::repositories::channels::NativeChannels { store, lease }
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

/// Operator adoption and subject installation use the same staging implementation.
pub(crate) fn marketplace_staging_scope<'a, 'connection>(
	store: &'a Store,
	tx: &'a mut sqlx::Transaction<'connection, sqlx::Postgres>,
	actor: &'a str,
) -> crate::apps::marketplace::repositories::installations::NativeStaging<'a, 'connection> {
	crate::apps::marketplace::repositories::installations::NativeStaging { store, tx, actor }
}

/// Trusted transport authority and the original transaction are shared with maintenance use cases.
pub(crate) fn marketplace_operator_scope<'a, 'connection>(
	store: &'a Store,
	tx: &'a mut sqlx::Transaction<'connection, sqlx::Postgres>,
	principal: aidash_domain::identity::Principal,
) -> crate::apps::marketplace::repositories::operations::NativeOperator<'a, 'connection> {
	crate::apps::marketplace::repositories::operations::NativeOperator {
		staging: marketplace_staging_scope(store, tx, "operator-adoption"),
		principal,
	}
}

pub(crate) fn marketplace_provenance_scope<'a, 'connection>(
	tx: &'a mut sqlx::Transaction<'connection, sqlx::Postgres>,
) -> crate::apps::marketplace::repositories::installations::NativeProvenance<'a, 'connection> {
	crate::apps::marketplace::repositories::installations::NativeProvenance { tx }
}

/// Discovery, exact reads and worker admission share the current Access transaction.
pub(crate) fn catalog_scope(
	access: &mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::catalog::NativeCatalog<'_> {
	crate::apps::identity::repositories::catalog::NativeCatalog(access)
}

pub(crate) fn catalog_mutation_scope<'a, 'connection>(
	tx: &'a mut sqlx::Transaction<'connection, sqlx::Postgres>,
) -> crate::apps::identity::repositories::catalog_mutation::NativeMutation<'a, 'connection> {
	crate::apps::identity::repositories::catalog_mutation::NativeMutation(tx)
}
/// Transport and trusted maintenance callers supply authority, never a request-body actor label.
pub(crate) fn catalog_administrator_scope<'a, 'connection>(
	tx: &'a mut sqlx::Transaction<'connection, sqlx::Postgres>,
	principal: aidash_domain::identity::Principal,
) -> crate::apps::identity::repositories::catalog_mutation::NativeAdministrator<'a, 'connection> {
	crate::apps::identity::repositories::catalog_mutation::NativeAdministrator {
		mutation: catalog_mutation_scope(tx),
		principal,
	}
}
pub(crate) fn catalog_administration_read(
	pool: &sqlx::PgPool,
	principal: aidash_domain::identity::Principal,
) -> crate::apps::identity::repositories::catalog_mutation::NativeAdministrationRead<'_> {
	crate::apps::identity::repositories::catalog_mutation::NativeAdministrationRead {
		pool,
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
) -> aidash_integrations::federation::PeerHttp {
	aidash_integrations::federation::PeerHttp {
		client: access.peer_client.clone(),
		node_id: access.node_id.clone(),
		protocol_version: crate::config::PROTOCOL_VERSION.into(),
		credentials: Arc::new(PeerCredentials),
	}
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
		Actor::Operator => aidash_domain::identity::Principal::Operator,
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
		Actor::Operator => aidash_domain::identity::Principal::Operator,
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

pub(crate) fn generation_lifecycle_scope<'a, 'tx>(
	runtime: &'a crate::federation::Federation,
	transaction: &'a mut sqlx::Transaction<'tx, sqlx::Postgres>,
) -> crate::apps::execution::generation::repositories::lifecycle::NativeLifecycle<'a, 'tx> {
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
		Actor::Operator => aidash_domain::identity::Principal::Operator,
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
	store: &crate::store::Store,
) -> impl aidash_application::ports::generation::dispatch::GenerationDispatchRepository {
	crate::apps::execution::generation::repositories::dispatch::NativeDispatch {
		store: store.clone(),
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
	store: &crate::store::Store,
) -> impl aidash_application::ports::generation::settlement::GenerationSettlementRepository {
	crate::apps::execution::generation::repositories::settlement::NativeSettlementRepository {
		store: store.clone(),
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
) -> impl aidash_application::ports::generation::reservation::GenerationReservationRepository {
	crate::apps::execution::generation::repositories::reservation::NativeReservation {
		store: store.clone(),
	}
}

pub(crate) fn generation_inference_authority_scope(
	access: &mut crate::authorization::access::Access,
) -> impl aidash_application::ports::generation::inference::GenerationInferenceAuthority + '_ {
	crate::apps::execution::generation::repositories::inference::NativeInferenceAuthority { access }
}
pub(crate) fn generation_inference_repository(
	store: &Store,
) -> crate::apps::execution::generation::repositories::inference::NativeInferenceRepository {
	crate::apps::execution::generation::repositories::inference::NativeInferenceRepository {
		database: store.database(),
		node_id: store.node_id.clone(),
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
) -> crate::apps::execution::generation::repositories::embedding::NativeEmbeddingRepository {
	crate::apps::execution::generation::repositories::embedding::NativeEmbeddingRepository {
		database: store.database(),
		node_id: store.node_id.clone(),
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
) -> crate::apps::execution::generation::repositories::compaction::NativeCompactionRepository {
	crate::apps::execution::generation::repositories::compaction::NativeCompactionRepository {
		store: store.clone(),
	}
}
pub(crate) struct ApprovedCompactionProvider;
impl aidash_application::ports::generation::compaction::GenerationCompactionProvider
	for ApprovedCompactionProvider
{
	fn approved(
		&self,
		config: aidash_domain::registry::CompactorConfig,
	) -> aidash_application::Result<
		Arc<dyn aidash_application::ports::generation::compaction::ApprovedCompactionTransport>,
	> {
		Ok(Arc::new(crate::context::jev::JevClient::approved(config)?))
	}
}
pub(crate) fn generation_compaction_provider() -> ApprovedCompactionProvider {
	ApprovedCompactionProvider
}

pub(crate) fn generation_protocol_authority(
	federation: &crate::federation::Federation,
) -> crate::apps::execution::generation::repositories::protocol::NativeProtocolAuthority {
	crate::apps::execution::generation::repositories::protocol::NativeProtocolAuthority {
		federation: federation.clone(),
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
impl aidash_application::ports::generation::compaction::remote::RemoteCompactionProvider
	for ApprovedCompactionProvider
{
	fn approved_remote(&self, config: aidash_domain::registry::CompactorConfig) -> aidash_application::Result<Arc<dyn aidash_application::ports::generation::compaction::remote::RemoteCompactionTransport>>{
		Ok(Arc::new(crate::context::jev::JevClient::approved(config)?))
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
		Actor::Operator => aidash_domain::identity::Principal::Operator,
		Actor::Subject(identity) => aidash_domain::identity::Principal::Subject {
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		},
	};
	crate::apps::execution::generation::repositories::foreign::home::NativeHome {
		federation: f.clone(),
		actor,
		principal,
	}
}

pub(crate) fn generation_receiver_repository(
	f: &Federation,
) -> crate::apps::execution::generation::repositories::foreign::receiver::NativeReceiver {
	crate::apps::execution::generation::repositories::foreign::receiver::NativeReceiver {
		federation: f.clone(),
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
) -> crate::apps::knowledge::repositories::mutations::NativeConfiguration {
	crate::apps::knowledge::repositories::mutations::NativeConfiguration {
		store: store.clone(),
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

/// Both memory representations commit together under the same authority lease.
pub(crate) fn semantic_memory_write_scope<'a, 'scope>(
	store: &Store,
	lease: &'a mut crate::semantic::service::Lease<'scope>,
) -> crate::apps::knowledge::repositories::memory::MemoryWriter<'a, 'scope> {
	crate::apps::knowledge::repositories::memory::MemoryWriter {
		inner: semantic_memory_scope(lease),
		store: store.clone(),
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
	revision: &'a str,
) -> crate::apps::identity::repositories::graph::Projection<'a, 'scope> {
	crate::apps::identity::repositories::graph::Projection {
		f,
		authority,
		source_node,
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
) -> aidash_integrations::federation::PeerHttp {
	let (client, node) = access.remote_transport_parts();
	aidash_integrations::federation::PeerHttp {
		client: client.clone(),
		node_id: node.into(),
		protocol_version: crate::config::PROTOCOL_VERSION.into(),
		credentials: Arc::new(PeerCredentials),
	}
}
pub(crate) fn registry_verification_scope<'a>(
	access: &'a mut crate::authorization::access::Access,
	node: &'a str,
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
pub(crate) fn operator_media_repository(
	store: &Store,
) -> crate::apps::workspaces::repositories::media::OperatorMedia<'_> {
	crate::apps::workspaces::repositories::media::OperatorMedia { store }
}

/// File selections resolve under one owned current-run authority guard and immutable snapshot.
pub(crate) fn selected_media<'a>(
	store: &'a Store,
	run: &'a aidash_domain::Run,
	access: Arc<tokio::sync::Mutex<crate::authorization::access::Access>>,
) -> crate::apps::workspaces::repositories::media::SelectedMedia<'a> {
	crate::apps::workspaces::repositories::media::SelectedMedia::new(store, run, access)
}

/// Current builtin and plugin authority borrows the same worker Access object.
pub(crate) fn agent_tool_repository<'a>(
	remote: Option<&'a Federation>,
	access: &'a Arc<tokio::sync::Mutex<crate::authorization::access::Access>>,
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
	access: &'a Arc<tokio::sync::Mutex<crate::authorization::access::Access>>,
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
	access: &'a Arc<tokio::sync::Mutex<crate::authorization::access::Access>>,
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
	access: &'a Arc<tokio::sync::Mutex<crate::authorization::access::Access>>,
	run: &'a crate::domain::Run,
) -> crate::apps::identity::repositories::execution::admission::Admissions<'a> {
	crate::apps::identity::repositories::execution::admission::Admissions {
		store,
		remote,
		access,
		run,
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
) -> crate::apps::federation::transactions::repositories::authority::Scope<'_> {
	crate::apps::federation::transactions::repositories::authority::Scope { access }
}

pub(crate) fn operation_withdrawal_repository(
	store: &Store,
) -> crate::apps::execution::repositories::withdrawal::Repository<'_> {
	crate::apps::execution::repositories::withdrawal::Repository { store }
}

pub(crate) fn remote_command_scope<'a>(
	runtime: &'a Federation,
	access: &'a mut crate::authorization::access::Access,
) -> crate::apps::identity::repositories::remote_commands::Scope<'a> {
	crate::apps::identity::repositories::remote_commands::Scope { access, runtime }
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
