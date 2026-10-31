//! Native execution repository preserves caller-owned transactions and lease fences.
use crate::apps::execution::models::{
	Event as EventRecord, HumanRequest as HumanRequestRecord, InferenceUsage,
	Invocation as InvocationRecord, Run as RunRecord, RunInput as InputRecord, event_records,
};
use crate::apps::execution::services::human_interaction;
use crate::apps::federation::remote::models::RemoteRunMessageFence;
use crate::apps::identity::models::AuthorizationRunOutput;
use crate::apps::workspaces::models::{
	Artifact as ArtifactRecord, Message as MessageRecord, Task as TaskRecord,
	Workspace as WorkspaceRecord,
};
use crate::apps::workspaces::serializers::tasks::TaskPage;
use crate::apps::workspaces::services::snapshots;
use crate::database::native::Pool;
use crate::{
	Error, Result,
	domain::*,
	registry::{Entry, Search},
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::ControlSource;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::ColumnRef;
use reinhardt::query::OnConflict;
use reinhardt::query::SimpleExpr;
use reinhardt::query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
use serde_json::{Value, json};
use std::future::Future;
use uuid::Uuid;

/// The Store as one Run's executor sees it: every transaction opens in the
/// Run's Tenant scope, learned from its authority when the step began.
#[derive(Clone)]
pub struct RunStore {
	pub store: Store,
	pub scope: TenancyScope,
}

#[derive(Clone)]
pub struct Store {
	pub provider_key_material_reader:
		Option<std::sync::Arc<dyn aidash_application::provider_access::KeyMaterialReader>>,
	pub capability_issuer:
		Option<std::sync::Arc<dyn aidash_application::provider_access::TokenIssuer>>,
	pub provider_credentials:
		Option<std::sync::Arc<aidash_application::provider_credentials::Service>>,
	/// Present whenever the Store can hold Plan Tokens (PostgreSQL), so owners
	/// can still read and disconnect their connections after
	/// `plan_connections_enabled` is turned off (ADR 0007).
	pub plan_connections: Option<std::sync::Arc<aidash_application::plan_connections::Service>>,
	/// `[plan_connections] enabled`: new authorizations, model lists and
	/// admission of `chatgpt` models.
	pub plan_connections_enabled: bool,
	/// Present only where the Secret Store can hold Tenant Secret values and
	/// the server may read them: a self-hosted node with a PostgreSQL Store
	/// (ADR 0051). Cloud and Dedicated offer none until #310.
	pub tenant_secrets: Option<std::sync::Arc<aidash_application::tenant_secrets::Service>>,
	pub capabilities: crate::capabilities::Runtime,
	pub pool: Pool,
	pub control_pool: Pool,
	/// Restoration claims survive external retirement without consuming the
	/// control connections retirement itself needs. Shared across Store clones.
	pub restoration_pool: Pool,
	pub deployment: aidash_domain::nodes::DeploymentId,
	pub semantic_client: reqwest::Client,
	/// Model and embedding clients: `semantic_client` for Provider Catalog
	/// endpoints and permitted private origins, a public-only one for every
	/// other Keyless endpoint (ADR 0036).
	pub provider_clients: aidash_integrations::keyless::Clients,
	/// Tenant HTTP and MCP tool clients, owned by this runtime like `provider_clients`.
	pub tool_clients: aidash_integrations::tools::ToolClients,
	/// Keyless provider access with this Deployment's permitted private origins.
	pub keyless: aidash_application::provider_access::KeylessAccess,
	/// The MCP sessions of Runs with their Agent Plugins' remote servers, shared by every
	/// clone of this Store and connecting under the same Keyless rule as `tool_clients`.
	pub plugin_sessions: std::sync::Arc<aidash_integrations::plugin_sessions::McpPluginSessions>,
	/// The Deployment Key and the switch Federation requests to other
	/// Deployments are signed and gated by, for callers that hold a Store and
	/// no Federation runtime (ADR 0064).
	pub egress: aidash_integrations::federation::Egress,
	/// The Deployment's Retention Rules from `[research_retention]` (ADR 0045).
	pub research_retention: aidash_domain::retention::RetentionRules,
	/// The Deployment's ceilings on Tenants' Test Limits from
	/// `[workbench_test_limits]` (#361).
	pub test_limit_ceilings: aidash_domain::registry::workbench::sandbox::TestLimitCeilings,
	/// The Deployment's ceilings on Live preview lifetimes from
	/// `[preview_limits]` (ADR 0070).
	pub preview_limits: aidash_domain::preview::live::Limits,
	/// The preview domain this Deployment serves; `None` offers no Previews.
	pub preview_domain: Option<String>,
	/// The only Pod port the Preview NetworkPolicy admits.
	pub preview_pod_port: Option<u16>,
	pub(crate) recovery_cursors: std::sync::Arc<run_state::RecoveryCursors>,
	/// Tenants found for Runs, Workspaces, tasks and grants, shared by every
	/// clone of this Store.
	pub(crate) tenancies: std::sync::Arc<tenancies::Tenancies>,
	/// The process Tool Parallelism ceiling, shared by every clone of this Store.
	pub tool_slots: aidash_application::ports::execution::ToolSlots,
	/// This node's Cache Salt Keys (ADR 0016); `None` admits only Legacy Runs.
	pub cache_salt: Option<aidash_integrations::inference::CacheSaltKeys>,
	/// Where Execution Pod time and storage samples go. The build installs it;
	/// Community's reports nowhere.
	pub usage: std::sync::Arc<dyn aidash_application::usage::UsageEvents>,
	/// Decides whether new work may begin. The build installs it; Community's
	/// admits everything.
	pub admission: std::sync::Arc<dyn aidash_application::usage::Admission>,
}

/// Worker slots are capped at four. Each holds an outer authority transaction
/// and at most one data connection for the step it is running, including a
/// sequential tool call.
const MAX_WORKER_SLOTS: usize = 4;
/// Data connections of each runtime worker pool without Tool Batches.
const WORKER_POOL_CONNECTIONS: usize = 2 * MAX_WORKER_SLOTS;
const MAX_TOOL_PARALLELISM: usize = 4;

/// A slot running a Tool Batch serves one of its calls with its own step
/// connection. Tool Slots admit at most `tool_parallelism` batched calls across
/// the process, so the others need connections beside every other slot's step.
fn worker_pool_connections(tool_parallelism: usize) -> usize {
	WORKER_POOL_CONNECTIONS + tool_parallelism.max(1) - 1
}

/// The process Tool Parallelism ceiling from `AIDASH_TOOL_PARALLELISM`; a pool
/// of `pool_connections` must leave the same headroom beside the worker slots.
pub(crate) fn tool_parallelism(value: Option<&str>, pool_connections: usize) -> Result<usize> {
	let ceiling = match value {
		Some(value) => value
			.parse::<usize>()
			.ok()
			.filter(|value| (1..=MAX_TOOL_PARALLELISM).contains(value))
			.ok_or_else(|| Error::Invalid("invalid AIDASH_TOOL_PARALLELISM".into()))?,
		None => 1,
	};
	if ceiling > 1 && ceiling + MAX_WORKER_SLOTS > pool_connections {
		return Err(Error::Invalid(
			"AIDASH_TOOL_PARALLELISM leaves no database pool headroom for worker slots".into(),
		));
	}
	Ok(ceiling)
}

/// Sessions that connect under the Deployment's Keyless rule, as `keyless` and `clients` say.
pub(crate) fn plugin_sessions(
	keyless: &aidash_application::provider_access::KeylessAccess,
	clients: &aidash_integrations::tools::ToolClients,
) -> std::sync::Arc<aidash_integrations::plugin_sessions::McpPluginSessions> {
	std::sync::Arc::new(
		aidash_integrations::plugin_sessions::McpPluginSessions::new(
			keyless.clone(),
			clients.clone(),
		),
	)
}

pub use crate::apps::execution::serializers::run_inputs::RunInput;

/// How a Run's Artifact is attributed. `run` is the Run that published it on
/// this Node, recorded on the Artifact itself; `tracked` also records it for
/// Run output visibility, which only scoped execution authority needs.
#[derive(Clone, Copy, Default)]
pub(crate) struct Publication {
	pub run: Option<Uuid>,
	pub tracked: bool,
}

impl Publication {
	/// Published by `run` on this Node.
	pub(crate) fn by(run: Uuid, tracked: bool) -> Self {
		Self {
			run: Some(run),
			tracked,
		}
	}
	fn tracked_run(self) -> Option<Uuid> {
		self.run.filter(|_| self.tracked)
	}
}

pub(crate) struct RunResponseMessage<'a> {
	pub run: &'a Run,
	pub worker: uuid::Uuid,
	pub included_input_seq: i64,
	pub sender: &'a str,
	pub content: &'a str,
	pub key: &'a str,
	pub track_output: bool,
}

pub(crate) struct RunMessageDelivery<'a> {
	pub workspace: uuid::Uuid,
	pub task_id: uuid::Uuid,
	pub run_id: uuid::Uuid,
	pub sender: &'a str,
	pub content: &'a str,
	pub input_key: &'a str,
	pub message_key: &'a str,
}

pub(crate) struct FencedRunMessageOutput<'a> {
	pub workspace: uuid::Uuid,
	pub task_id: uuid::Uuid,
	pub run_id: uuid::Uuid,
	pub included_input_seq: i64,
	pub sender: &'a str,
	pub content: &'a str,
	pub key: &'a str,
}

impl Store {
	pub(crate) async fn with_run_response_fence<T, F>(
		&self,
		scope: &TenancyScope,
		run_id: Uuid,
		worker: Uuid,
		included_input_seq: i64,
		effect: F,
	) -> Result<T>
	where
		F: Future<Output = Result<T>>,
	{
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		RunRecord::ensure_response_current(tx.as_mut(), run_id, worker, included_input_seq).await?;
		let result = effect.await?;
		tx.commit().await?;
		Ok(result)
	}
	pub(crate) async fn response_message_in_run(
		&self,
		scope: &TenancyScope,
		request: RunResponseMessage<'_>,
	) -> Result<()> {
		let RunResponseMessage {
			run,
			worker,
			included_input_seq,
			sender,
			content,
			key,
			track_output,
		} = request;
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		RunRecord::ensure_response_current(tx.as_mut(), run.id, worker, included_input_seq).await?;
		RunRecord::worker_context(tx.as_mut()).await?;
		nonempty(content, "message")?;
		let message = MessageRecord::append_in(
			tx.as_mut(),
			run.workspace_id,
			sender,
			content,
			key,
			Some(run.id),
		)
		.await?;
		if track_output {
			AuthorizationRunOutput::record(
				tx.as_mut(),
				run.id,
				run.workspace_id,
				"message",
				message.id,
			)
			.await?;
		}
		tx.commit().await?;
		Ok(())
	}

	pub(crate) async fn run_tenant(&self, run: Uuid) -> Result<aidash_domain::tenancy::TenantName> {
		self.run_tenancy(run)
			.await?
			.tenant_name()
			.cloned()
			.ok_or(Error::Forbidden)
	}

	/// The scope of a Run that a caller names only by id: the Tenant its row
	/// records, found across Tenants.
	pub(crate) async fn run_tenancy(&self, run: Uuid) -> Result<TenancyScope> {
		if let Some(tenant) = self.tenancies.run(run) {
			return Ok(tenant.into());
		}
		let lookup = TenancyScope::Deployment(DeploymentScopeReason::RunTenant);
		let tenant: Option<String> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("tenant"))
				.from(Alias::new("runs"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(run)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(self.pool.in_scope(&lookup))
		.await?;
		let tenant = aidash_domain::tenancy::TenantName::new(
			tenant.ok_or_else(|| Error::NotFound("run".into()))?,
		)?;
		self.tenancies.remember_run(run, tenant.clone());
		Ok(tenant.into())
	}

	/// The Tenant that owns a local workspace.
	pub(crate) async fn workspace_tenant(&self, workspace: Uuid) -> Result<String> {
		if let Some(tenant) = self.tenancies.workspace(workspace) {
			return Ok(tenant.into_inner());
		}
		let scope = &TenancyScope::Deployment(DeploymentScopeReason::RunTenant);
		let tenant: Option<String> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("tenant"))
				.from(Alias::new("workspaces"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(workspace)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(self.pool.in_scope(scope))
		.await?;
		let tenant = tenant.ok_or_else(|| Error::NotFound("workspace".into()))?;
		self.tenancies.remember_workspace(
			workspace,
			aidash_domain::tenancy::TenantName::new(tenant.clone())?,
		);
		Ok(tenant)
	}

	/// The scope of a Workspace that a caller names only by id: its Tenant.
	pub(crate) async fn workspace_tenancy(&self, workspace: Uuid) -> Result<TenancyScope> {
		Ok(TenancyScope::tenant(
			&self.workspace_tenant(workspace).await?,
		)?)
	}

	/// The scope of a cross-node generation or semantic operation, found from
	/// the local record it names: this node's execution grant or admission of
	/// the Run, or the remote grant this node issued.
	pub(crate) async fn remote_tenancy(
		&self,
		admission: Uuid,
		grant: Uuid,
	) -> Result<TenancyScope> {
		if let Some(tenant) = self.tenancies.run(admission) {
			return Ok(tenant.into());
		}
		if let Some(tenant) = self.tenancies.grant(grant) {
			return Ok(tenant.into());
		}
		let lookup = TenancyScope::Deployment(DeploymentScopeReason::RunTenant);
		let tenant: Option<(&str, String)> = self
			.pool
			.scoped(&lookup, async |tx| {
				for (table, column, id) in [
					("authorization_execution", "run_id", admission),
					("authorization_remote_admissions", "id", admission),
					("authorization_remote_grants", "id", grant),
				] {
					let tenant = crate::database::native::query_scalar::<String>(
						&Query::select()
							.column(Alias::new("tenant"))
							.from(Alias::new(table))
							.and_where(Expr::col(Alias::new(column)).eq(Expr::value(id)))
							.to_string(PostgresQueryBuilder),
					)
					.scalar_optional(&mut **tx)
					.await?;
					if let Some(tenant) = tenant {
						return Ok(Some((table, tenant)));
					}
				}
				Ok(None)
			})
			.await?;
		match tenant {
			Some((table, tenant)) => {
				let tenant = aidash_domain::tenancy::TenantName::new(tenant)?;
				if table == "authorization_remote_grants" {
					self.tenancies.remember_grant(grant, tenant.clone());
				} else {
					self.tenancies.remember_run(admission, tenant.clone());
				}
				Ok(tenant.into())
			}
			None => Err(Error::NotFound("remote Run".into())),
		}
	}

	/// The Node that acts for the operation `remote_tenancy` found: the Node a
	/// remote Run executes at, the Node a remote grant was issued from, or the
	/// Node of a local Run.
	pub(crate) async fn remote_node(
		&self,
		scope: &TenancyScope,
		admission: Uuid,
		grant: Uuid,
	) -> Result<aidash_domain::nodes::NodeName> {
		let name: Option<String> = self
			.pool
			.scoped(scope, async |tx| {
				for (table, column, id) in [
					("runs", "id", admission),
					("authorization_remote_admissions", "id", admission),
					("authorization_remote_grants", "id", grant),
				] {
					let node = crate::database::native::query_scalar::<String>(
						&Query::select()
							.column(Alias::new("node"))
							.from(Alias::new(table))
							.and_where(Expr::col(Alias::new(column)).eq(Expr::value(id)))
							.to_string(PostgresQueryBuilder),
					)
					.scalar_optional(&mut **tx)
					.await?;
					if node.is_some() {
						return Ok(node);
					}
				}
				Ok(None)
			})
			.await?;
		Ok(aidash_domain::nodes::NodeName::parse(
			&name.ok_or_else(|| Error::NotFound("remote Run".into()))?,
		)?)
	}

	/// Run creation re-checks the Projection Version pinned by the Binding
	/// snapshot (ADR 0015/0016): it must be implemented, declared by the
	/// pinned model and, when salted, backed by this node's Cache Salt Key.
	/// Legacy Projection Versions are never affected; nothing falls back to
	/// Legacy.
	pub(crate) fn require_projection(
		&self,
		snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
	) -> Result<()> {
		require_projection(snapshot, self.cache_salt.is_some())
	}

	/// The view of one Run's executor, in the Run's Tenant scope.
	pub fn for_run(&self, scope: TenancyScope) -> RunStore {
		RunStore {
			store: self.clone(),
			scope,
		}
	}

	pub async fn from_pool(
		pool: sqlx::PgPool,
		deployment: aidash_domain::nodes::DeploymentId,
	) -> Result<Self> {
		let tool_slots = aidash_application::ports::execution::ToolSlots::new(tool_parallelism(
			std::env::var("AIDASH_TOOL_PARALLELISM").ok().as_deref(),
			pool.options().get_max_connections() as usize,
		)?);
		let control_pool = pool
			.options()
			.clone()
			.max_connections(16)
			.connect_with(pool.connect_options().as_ref().clone())
			.await?;
		let restoration_pool = pool
			.options()
			.clone()
			.max_connections(4)
			.min_connections(0)
			.idle_timeout(std::time::Duration::from_secs(10))
			.connect_lazy_with(pool.connect_options().as_ref().clone());
		let memory_recovery = std::env::var_os("AIDASH_MEMORY_RECOVERY_DIR")
			.map(|directory| {
				crate::semantic::repositories::recovery::FileRecovery::from_env(
					directory.into(),
					deployment.to_string(),
				)
			})
			.transpose()?
			.map(|recovery| {
				std::sync::Arc::new(recovery)
					as std::sync::Arc<dyn aidash_application::ports::memory::MemoryRecovery>
			});
		if let Some(recovery) = &memory_recovery {
			let _ = recovery.verify().await;
		}
		let provider_clients = crate::semantic::backend::clients()?;
		let tool_clients = aidash_integrations::tools::ToolClients::new()?;
		// One access, shared by the sessions and by everything that reads `keyless`, so that
		// an origin permitted later reaches both.
		let keyless = aidash_application::provider_access::KeylessAccess::default();
		Ok(Self {
			pool: pool.into(),
			control_pool: control_pool.into(),
			restoration_pool: restoration_pool.into(),
			deployment,
			semantic_client: provider_clients.direct.clone(),
			provider_clients,
			plugin_sessions: plugin_sessions(&keyless, &tool_clients),
			tool_clients,
			keyless,
			egress: Default::default(),
			research_retention: aidash_domain::retention::RetentionRules::DEFAULT,
			test_limit_ceilings: Default::default(),
			preview_limits: aidash_domain::preview::live::Limits::default(),
			preview_domain: None,
			preview_pod_port: None,
			recovery_cursors: Default::default(),
			tenancies: Default::default(),
			tool_slots,
			cache_salt: None,
			usage: std::sync::Arc::new(aidash_application::usage::NoUsage),
			admission: std::sync::Arc::new(aidash_application::usage::AlwaysAdmit),
			provider_credentials: None,
			plan_connections: None,
			plan_connections_enabled: false,
			tenant_secrets: None,
			provider_key_material_reader: None,
			capability_issuer: None,
			capabilities: crate::capabilities::Runtime::from_env()?,
		}
		.with_memory_recovery(memory_recovery))
	}

	/// Attach the deployment issuer and Tenant Bindings to every authority pool.
	pub fn with_dashboard_policy(
		mut self,
		policy: Option<aidash_application::ports::authorization::dashboard::AccountPolicy>,
	) -> Self {
		self.pool = self.pool.with_dashboard_policy(policy.clone());
		self.control_pool = self.control_pool.with_dashboard_policy(policy);
		self
	}
	/// Attach this node's Cache Salt Keys for salted Run creation and inference.
	pub fn with_cache_salt(
		mut self,
		keys: Option<aidash_integrations::inference::CacheSaltKeys>,
	) -> Self {
		self.cache_salt = keys;
		self
	}
	/// Permit these private origins for Keyless endpoints (ADR 0036).
	pub fn with_research_retention(
		mut self,
		rules: aidash_domain::retention::RetentionRules,
	) -> Self {
		self.research_retention = rules;
		self
	}
	pub fn with_preview_limits(mut self, limits: aidash_domain::preview::live::Limits) -> Self {
		self.preview_limits = limits;
		self
	}

	pub fn with_preview(mut self, domain: Option<String>, pod_port: Option<u16>) -> Self {
		self.preview_domain = domain;
		self.preview_pod_port = pod_port;
		self
	}

	pub fn with_test_limit_ceilings(
		mut self,
		ceilings: aidash_domain::registry::workbench::sandbox::TestLimitCeilings,
	) -> Self {
		self.test_limit_ceilings = ceilings;
		self
	}
	pub fn with_egress(mut self, egress: aidash_integrations::federation::Egress) -> Self {
		self.egress = egress;
		self
	}
	pub fn with_keyless_origins(
		mut self,
		origins: aidash_domain::keyless::PermittedOrigins,
	) -> Self {
		self.keyless = aidash_application::provider_access::KeylessAccess::permitting(origins);
		self.plugin_sessions = plugin_sessions(&self.keyless, &self.tool_clients);
		self
	}
	pub(crate) fn with_memory_recovery(
		mut self,
		recovery: Option<std::sync::Arc<dyn aidash_application::ports::memory::MemoryRecovery>>,
	) -> Self {
		self.pool = self.pool.with_memory_recovery(recovery.clone());
		self.control_pool = self.control_pool.with_memory_recovery(recovery);
		self
	}

	/// Share the database and connection settings without sharing pool capacity.
	pub async fn isolated_pool(&self) -> Result<Self> {
		let mut store = self.worker_pool().await?;
		store.control_pool = self
			.control_pool
			.options()
			.clone()
			.max_connections(4)
			.idle_timeout(std::time::Duration::from_secs(10))
			.connect_with(self.control_pool.connect_options().as_ref().clone())
			.await?
			.into();
		store.control_pool = store
			.control_pool
			.with_memory_recovery(self.control_pool.memory_recovery())
			.with_dashboard_policy(self.control_pool.dashboard_policy());
		Ok(store)
	}

	/// Runtime workers reserve data capacity while sharing visibility leases.
	/// Callers must not explicitly close the shared control pool.
	pub async fn worker_pool(&self) -> Result<Self> {
		let pool = self
			.pool
			.options()
			.clone()
			.max_connections(worker_pool_connections(self.tool_slots.ceiling()) as u32)
			.idle_timeout(std::time::Duration::from_secs(10))
			.connect_with(self.pool.connect_options().as_ref().clone())
			.await?;
		Ok(Self {
			pool: Pool::from(pool)
				.with_memory_recovery(self.pool.memory_recovery())
				.with_dashboard_policy(self.pool.dashboard_policy()),
			control_pool: self.control_pool.clone(),
			restoration_pool: self.restoration_pool.clone(),
			deployment: self.deployment.clone(),
			semantic_client: self.semantic_client.clone(),
			provider_clients: self.provider_clients.clone(),
			tool_clients: self.tool_clients.clone(),
			keyless: self.keyless.clone(),
			plugin_sessions: self.plugin_sessions.clone(),
			egress: self.egress.clone(),
			research_retention: self.research_retention,
			test_limit_ceilings: self.test_limit_ceilings,
			preview_limits: self.preview_limits.clone(),
			preview_domain: self.preview_domain.clone(),
			preview_pod_port: self.preview_pod_port,
			recovery_cursors: self.recovery_cursors.clone(),
			tenancies: self.tenancies.clone(),
			tool_slots: self.tool_slots.clone(),
			cache_salt: self.cache_salt.clone(),
			usage: self.usage.clone(),
			admission: self.admission.clone(),
			provider_credentials: self.provider_credentials.clone(),
			plan_connections: self.plan_connections.clone(),
			plan_connections_enabled: self.plan_connections_enabled,
			tenant_secrets: self.tenant_secrets.clone(),
			provider_key_material_reader: self.provider_key_material_reader.clone(),
			capability_issuer: self.capability_issuer.clone(),
			capabilities: self.capabilities.clone(),
		})
	}

	/// Transaction recovery needs independent control capacity even while
	/// ordinary work retains visibility leases. Other isolated workers share
	/// the node's visibility pool instead of multiplying idle connections.
	pub async fn recovery_pool(&self) -> Result<Self> {
		let pool = self
			.pool
			.options()
			.clone()
			.max_connections(4)
			.idle_timeout(std::time::Duration::from_secs(10))
			.connect_with(self.pool.connect_options().as_ref().clone())
			.await?;
		let control_pool: Pool = self
			.control_pool
			.options()
			.clone()
			.max_connections(4)
			.idle_timeout(std::time::Duration::from_secs(10))
			.connect_with(self.control_pool.connect_options().as_ref().clone())
			.await?
			.into();
		Ok(Self {
			pool: Pool::from(pool)
				.with_memory_recovery(self.pool.memory_recovery())
				.with_dashboard_policy(self.pool.dashboard_policy()),
			restoration_pool: self.restoration_pool.clone(),
			control_pool: control_pool
				.with_memory_recovery(self.control_pool.memory_recovery())
				.with_dashboard_policy(self.control_pool.dashboard_policy()),
			deployment: self.deployment.clone(),
			semantic_client: self.semantic_client.clone(),
			provider_clients: self.provider_clients.clone(),
			tool_clients: self.tool_clients.clone(),
			keyless: self.keyless.clone(),
			plugin_sessions: self.plugin_sessions.clone(),
			egress: self.egress.clone(),
			research_retention: self.research_retention,
			test_limit_ceilings: self.test_limit_ceilings,
			preview_limits: self.preview_limits.clone(),
			preview_domain: self.preview_domain.clone(),
			preview_pod_port: self.preview_pod_port,
			recovery_cursors: self.recovery_cursors.clone(),
			tenancies: self.tenancies.clone(),
			tool_slots: self.tool_slots.clone(),
			cache_salt: self.cache_salt.clone(),
			usage: self.usage.clone(),
			admission: self.admission.clone(),
			provider_credentials: self.provider_credentials.clone(),
			plan_connections: self.plan_connections.clone(),
			plan_connections_enabled: self.plan_connections_enabled,
			tenant_secrets: self.tenant_secrets.clone(),
			provider_key_material_reader: self.provider_key_material_reader.clone(),
			capability_issuer: self.capability_issuer.clone(),
			capabilities: self.capabilities.clone(),
		})
	}
	pub async fn event(
		&self,
		tx: &mut crate::database::native::Transaction,
		workspace: Option<Uuid>,
		kind: &str,
		data: Value,
	) -> Result<Event> {
		self.event_in_executor(&mut **tx, workspace, kind, data)
			.await
	}
	pub(crate) async fn event_in_executor(
		&self,
		tx: &mut dyn TransactionExecutor,
		workspace: Option<Uuid>,
		kind: &str,
		data: Value,
	) -> Result<Event> {
		// Sequence allocation and commit order must agree for Last-Event-ID replay.
		crate::database::native::query(
			&Query::select()
				.expr(Expr::cust("PG_ADVISORY_XACT_LOCK(71003201)"))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		Ok({
			let query_bind_1 = Uuid::new_v4();
			let query_bind_3 = workspace;
			let query_bind_4 = kind;
			let query_bind_5 = data;
			aidash_server::database::query_as(
				&Query::insert()
					.into_table(Alias::new("events"))
					.columns([
						Alias::new("id"),
						Alias::new("node"),
						Alias::new("workspace_id"),
						Alias::new("kind"),
						Alias::new("data"),
						Alias::new("tenant"),
					])
					.from_subquery(
						Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(event_records::event_node(workspace))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_5.to_owned()).into()],
							))
							.expr(crate::database::scope::event_tenant(workspace))
							.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut *tx)
			.await?
		})
	}
	/// An event of the Run `run`; see [`event_records::append_run`].
	pub(crate) async fn run_event(
		&self,
		tx: &mut crate::database::native::Transaction,
		run: Uuid,
		workspace: Option<Uuid>,
		kind: &str,
		data: Value,
	) -> Result<()> {
		event_records::append_run(&mut **tx, run, workspace, kind, data).await
	}
	pub async fn emit(
		&self,
		scope: &TenancyScope,
		workspace: Option<Uuid>,
		kind: &str,
		data: Value,
	) -> Result<Event> {
		self.pool
			.atomic(scope, async |tx| {
				Ok(event_records::create(tx, workspace, kind, data)
					.await?
					.into())
			})
			.await
	}
	pub async fn create_workspace(
		&self,
		scope: &TenancyScope,
		owner: &str,
		title: &str,
		goal: &str,
	) -> Result<Workspace> {
		let tenant = scope.tenant_name().ok_or(Error::Forbidden)?.as_str();
		let mut tx = self.pool.begin(scope).await?;
		let node: String = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("name"))
				.from(Alias::new("nodes"))
				.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
				.and_where(Expr::col("is_default"))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut *tx)
		.await?;
		let node = aidash_domain::nodes::NodeName::parse(&node)?;
		let workspace = self
			.create_workspace_in(&mut tx, Uuid::new_v4(), tenant, &node, title, goal)
			.await?;
		self.record_workspace_tenant_in(&mut tx, workspace.id, tenant, owner)
			.await?;
		tx.commit().await?;
		Ok(workspace)
	}
	pub(crate) async fn record_workspace_tenant_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		workspace: Uuid,
		tenant: &str,
		owner: &str,
	) -> Result<()> {
		crate::database::native::query(
			&Query::insert()
				.into_table(Alias::new("authorization_workspaces"))
				.columns([
					Alias::new("workspace_id"),
					Alias::new("tenant"),
					Alias::new("owner_subject"),
				])
				.values_panic([
					Expr::value(workspace),
					Expr::value(tenant),
					Expr::value(owner),
				])
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?;
		Ok(())
	}
	pub(crate) async fn create_workspace_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		tenant: &str,
		node: &aidash_domain::nodes::NodeName,
		title: &str,
		goal: &str,
	) -> Result<Workspace> {
		nonempty(title, "title")?;
		nonempty(goal, "goal")?;
		let w: Workspace = {
			let query_bind_1 = id;
			let query_bind_2 = title;
			let query_bind_3 = goal;
			aidash_server::database::query_as(
				&Query::insert()
					.into_table(Alias::new("workspaces"))
					.columns([
						Alias::new("id"),
						Alias::new("node"),
						Alias::new("title"),
						Alias::new("goal"),
						Alias::new("tenant"),
					])
					.from_subquery(
						Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(Expr::value(node.as_str().to_owned()))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							))
							.expr(Expr::value(tenant.to_owned()))
							.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		self.event(tx, Some(w.id), "workspace.created", json!(w))
			.await?;
		Ok(w)
	}
	pub async fn workspaces(&self, scope: &TenancyScope) -> Result<Vec<Workspace>> {
		Ok(self
			.pool
			.atomic(scope, async |tx| WorkspaceRecord::newest_first(tx).await)
			.await?
			.into_iter()
			.map(Into::into)
			.collect())
	}
	pub async fn workspace(&self, scope: &TenancyScope, id: Uuid) -> Result<Workspace> {
		Ok(self
			.pool
			.atomic(scope, async |tx| WorkspaceRecord::read(tx, id).await)
			.await?
			.into())
	}
	pub async fn update_state(
		&self,
		scope: &TenancyScope,
		id: Uuid,
		revision: i64,
		state: Value,
	) -> Result<Workspace> {
		let mut tx = self.pool.begin(scope).await?;
		let workspace = self.update_state_in(&mut tx, id, revision, state).await?;
		tx.commit().await?;
		Ok(workspace)
	}
	pub(crate) async fn update_state_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		revision: i64,
		state: Value,
	) -> Result<Workspace> {
		if !state.is_object() {
			return Err(Error::Invalid("workspace state must be an object".into()));
		}
		let w = {
			let query_bind_1 = id;
			let query_bind_2 = revision;
			let query_bind_3 = state;
			aidash_server::database::query_as(
				&Query::update()
					.table(Alias::new("workspaces"))
					.value_expr(
						Alias::new("state"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND revision = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await?
		}
		.ok_or_else(|| Error::Conflict("workspace revision changed".into()))?;
		self.event(tx, Some(id), "workspace.updated", json!(w))
			.await?;
		Ok(w)
	}
	pub async fn task(&self, scope: &TenancyScope, id: Uuid) -> Result<Task> {
		Ok(self
			.pool
			.atomic(scope, async |tx| TaskRecord::read(tx, id).await)
			.await?
			.into())
	}
	pub async fn task_page(&self, scope: &TenancyScope, offset: u64) -> Result<TaskPage> {
		self.pool
			.atomic(scope, async |tx| TaskRecord::page(tx, offset).await)
			.await
	}
	pub async fn tasks(&self, scope: &TenancyScope, workspace: Option<Uuid>) -> Result<Vec<Task>> {
		Ok(self
			.pool
			.atomic(scope, async |tx| {
				TaskRecord::chronological(tx, workspace).await
			})
			.await?
			.into_iter()
			.map(Into::into)
			.collect())
	}
	pub async fn create_task(
		&self,
		scope: &TenancyScope,
		workspace: Uuid,
		input: &NewTask,
		creator: &str,
		key: Option<&str>,
	) -> Result<Task> {
		let mut tx = self.pool.begin(scope).await?;
		let task = self
			.create_task_in(&mut tx, workspace, input, creator, key)
			.await?;
		tx.commit().await?;
		Ok(task)
	}
	pub(crate) async fn create_task_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		workspace: Uuid,
		input: &NewTask,
		creator: &str,
		key: Option<&str>,
	) -> Result<Task> {
		nonempty(&input.title, "task title")?;
		nonempty(&input.description, "task description")?;
		if !input.requirements.is_object() {
			return Err(Error::Invalid("requirements must be an object".into()));
		}
		let _: Search = serde_json::from_value(input.requirements.clone())
			.map_err(|e| Error::Invalid(e.to_string()))?;
		for dep in input.dependencies.iter().chain(input.parent_id.iter()) {
			let valid: bool = {
				let query_bind_1 = dep;
				let query_bind_2 = workspace;
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(EXISTS(SELECT 1 FROM tasks WHERE id = ? AND workspace_id = ?))"
								.to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_one(&mut **tx)
				.await?
			};
			if !valid {
				return Err(Error::Invalid(
					"dependencies and parent must belong to the same workspace".into(),
				));
			}
		}
		let mut ancestor = input.parent_id;
		let mut visited = std::collections::BTreeSet::new();
		while let Some(id) = ancestor {
			if input.dependencies.contains(&id) || !visited.insert(id) {
				return Err(Error::Invalid(
					"task dependencies cannot include an ancestor".into(),
				));
			}
			ancestor = {
				let query_bind_1 = id;
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.column(reinhardt::query::Alias::new("parent_id"))
						.from(reinhardt::query::Alias::new("tasks"))
						.and_where(
							reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
								reinhardt::query::Alias::new("id"),
							))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							)),
						)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_one(&mut **tx)
				.await?
			};
		}
		if let Some(parent) = input.parent_id {
			// Completion takes the same row lock before testing its children.
			let status: TaskStatus = {
				let query_bind_1 = parent;
				crate::database::query_as(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new("status")),
						))
						.from(reinhardt::query::Alias::new("tasks"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.lock(reinhardt::query::LockType::Update)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_one(&mut **tx)
				.await?
			};
			let replay: bool = {
				let query_bind_1 = key;
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(EXISTS(SELECT 1 FROM tasks WHERE creation_key = ?))".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_one(&mut **tx)
				.await?
			};
			if status.is_terminal() && !replay {
				return Err(Error::Conflict(
					"cannot add a child to a terminal parent".into(),
				));
			}
		}
		let task: Option<Task> = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = workspace;
			let query_bind_3 = &input.title;
			let query_bind_4 = &input.description;
			let query_bind_5 = &input.requirements;
			let query_bind_6 = creator;
			let query_bind_7 = &input.dependencies;
			let query_bind_8 = input.parent_id;
			let query_bind_9 = key;
			aidash_server::database::query_as(
				&reinhardt::query::Query::insert()
					.into_table(reinhardt::query::Alias::new("tasks"))
					.columns([
						reinhardt::query::Alias::new("id"),
						reinhardt::query::Alias::new("workspace_id"),
						reinhardt::query::Alias::new("title"),
						reinhardt::query::Alias::new("description"),
						reinhardt::query::Alias::new("requirements"),
						reinhardt::query::Alias::new("created_by"),
						reinhardt::query::Alias::new("dependencies"),
						reinhardt::query::Alias::new("parent_id"),
						reinhardt::query::Alias::new("creation_key"),
						reinhardt::query::Alias::new("tenant"),
					])
					.from_subquery(
						reinhardt::query::Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_5.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_6.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![crate::database::uuid_array(query_bind_7.to_owned())],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_8.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_9.to_owned()).into()],
							))
							.expr(crate::database::scope::tenant_of(
								"workspaces",
								"id",
								query_bind_2,
							))
							.to_owned(),
					)
					.on_conflict(
						reinhardt::query::OnConflict::columns([reinhardt::query::Alias::new(
							"creation_key",
						)])
						.do_nothing()
						.to_owned(),
					)
					.returning_all()
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await?
		};
		let task = match task {
			Some(t) => {
				self.event(tx, Some(workspace), "task.created", json!(t))
					.await?;
				t
			}
			None => {
				let t: Task = {
					let query_bind_1 = key;
					aidash_server::database::query_as(
						&reinhardt::query::Query::select()
							.expr(reinhardt::query::SimpleExpr::from(
								reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
							))
							.from(reinhardt::query::Alias::new("tasks"))
							.and_where(SimpleExpr::CustomWithExpr(
								"(creation_key = ?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.to_string(reinhardt::query::PostgresQueryBuilder),
					)
					.fetch_one(&mut **tx)
					.await?
				};
				if t.workspace_id != workspace
					|| t.title != input.title
					|| t.description != input.description
					|| t.requirements != input.requirements
					|| t.dependencies != input.dependencies
					|| t.parent_id != input.parent_id
					|| t.created_by != creator
				{
					return Err(Error::Conflict(
						"idempotency key reused for a different task".into(),
					));
				}
				t
			}
		};
		Ok(task)
	}
	pub async fn claim(
		&self,
		scope: &TenancyScope,
		id: Uuid,
		revision: i64,
		owner: &str,
		agent: &Entry,
	) -> Result<Task> {
		let task = self.task(scope, id).await?;
		let mut tx = self.pool.begin(scope).await?;
		let claimed = self
			.claim_in(&mut tx, &task, revision, owner, agent, None)
			.await?;
		crate::semantic::repositories::bindings::claimed(
			self,
			&mut crate::semantic::service::Lease::BorrowedWithoutSubject(&mut tx),
			task.id,
			agent,
		)
		.await?;
		tx.commit().await?;
		Ok(claimed)
	}
	/// The Node of a Workspace, read in the caller's transaction.
	pub(crate) async fn workspace_node_in(
		tx: &mut crate::database::native::Transaction,
		workspace: Uuid,
	) -> Result<aidash_domain::nodes::NodeName> {
		let name: String = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("node"))
				.from(Alias::new("workspaces"))
				.and_where(Expr::col("id").eq(Expr::value(workspace)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **tx)
		.await?;
		Ok(aidash_domain::nodes::NodeName::parse(&name)?)
	}
	pub(crate) async fn claim_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		task: &Task,
		revision: i64,
		owner: &str,
		agent: &Entry,
		admitted: Option<&aidash_domain::registry::bindings::BindingSnapshot>,
	) -> Result<Task> {
		super::node_cancellations::require_task_open(&mut **tx, task.id).await?;
		let id = task.id;
		let mut requirements: Search = serde_json::from_value(task.requirements.clone())?;
		requirements.kind = Some("agent".into());
		if !requirements.matches(agent) {
			return Err(Error::Invalid(
				"agent does not satisfy task requirements".into(),
			));
		}
		let claimed: Task = { let query_bind_1 = id; let query_bind_2 = revision; let query_bind_3 = owner; aidash_server::database::query_as(&Query::update().table(Alias::new("tasks")).value_expr(Alias::new("status"), Expr::cust("'CLAIMED'")).value_expr(Alias::new("owner"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into()])).value_expr(Alias::new("revision"), Expr::cust("revision + 1")).and_where(SimpleExpr::CustomWithExpr("(id = ? AND revision = ? AND status = 'OPEN' AND NOT EXISTS(SELECT 1 FROM delegations AS d WHERE d.task_id = tasks.id AND d.node_id || '/agents/' || d.agent_id || '@' || d.agent_version <> ?) AND NOT EXISTS(SELECT 1 FROM tasks AS d WHERE d.id = ANY(tasks.dependencies) AND d.status <> 'COMPLETED'))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()])).returning_all().to_string(PostgresQueryBuilder)).fetch_optional(&mut **tx).await? }.ok_or_else(|| Error::Conflict("task already claimed, revision changed, or dependencies are incomplete".into()))?;
		let node = Self::workspace_node_in(tx, claimed.workspace_id).await?;
		let address = self.deployment.node(node);
		if owner == qualified_agent(&address, &agent.id, &agent.version) {
			let snapshot = if let Some(snapshot) = admitted {
				snapshot.clone()
			} else {
				crate::apps::registry::repositories::bindings::snapshot(
					tx,
					self.deployment.as_str(),
					agent,
					false,
					self.provider_credentials.is_some(),
				)
				.await?
			};
			snapshot.validate()?;
			self.require_projection(&snapshot)?;
			let tenant: String = crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("tenant"))
					.from(Alias::new("workspaces"))
					.and_where(Expr::col("id").eq(Expr::value(claimed.workspace_id)))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut **tx)
			.await?;
			aidash_application::usage::require(
				self.admission.as_ref(),
				&tenant,
				aidash_application::usage::Work::Run,
			)
			.await?;
			let context = crate::context::Context {
				binding_snapshot: Some(Box::new(snapshot)),
				..Default::default()
			};
			// Persist the local execution with the claim; no crash can strand a
			// claimed task between the control API and its worker queue.
			{
				let query_bind_1 = Uuid::new_v4();
				let query_bind_2 = id;
				let query_bind_3 = claimed.workspace_id;
				let query_bind_4 = address.to_string();
				let query_bind_5 = &agent.id;
				let query_bind_6 = &agent.version;
				let query_bind_7 = serde_json::to_value(&context)?;
				crate::database::native::query(
					&Query::insert()
						.into_table(Alias::new("runs"))
						.columns([
							Alias::new("id"),
							Alias::new("task_id"),
							Alias::new("workspace_id"),
							Alias::new("home_node"),
							Alias::new("node"),
							Alias::new("agent_id"),
							Alias::new("agent_version"),
							Alias::new("context"),
							Alias::new("tenant"),
						])
						.from_subquery(
							Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()],
								))
								.expr(Expr::value(address.node.as_str().to_owned()))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_5.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_6.to_owned()).into()],
								))
								.expr(Expr::value(query_bind_7))
								.expr(crate::database::scope::tenant_of(
									"workspaces",
									"id",
									query_bind_3,
								))
								.to_owned(),
						)
						.on_conflict(
							OnConflict::columns([Alias::new("home_node"), Alias::new("task_id")])
								.do_nothing()
								.to_owned(),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **tx)
				.await?
			};
			let executor: (String, String) = {
				let query_bind_1 = address.to_string();
				let query_bind_2 = id;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(Alias::new("agent_id"))))
						.expr(SimpleExpr::from(Expr::col(Alias::new("agent_version"))))
						.from(Alias::new("runs"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(home_node = ? AND task_id = ?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["agent_id", "agent_version"])
				.fetch_one(&mut **tx)
				.await?
			};
			if executor != (agent.id.clone(), agent.version.clone()) {
				return Err(Error::Conflict(
					"task has a queued run for a different agent".into(),
				));
			}
			let run_id: Uuid = crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("runs"))
					.and_where(Expr::col("home_node").eq(Expr::value(address.to_string())))
					.and_where(Expr::col("task_id").eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut **tx)
			.await?;
			crate::apps::identity::repositories::provider_credentials::admit_workspace(
				&mut **tx,
				run_id,
				claimed.workspace_id,
				context
					.binding_snapshot
					.as_deref()
					.expect("admitted Binding snapshot"),
				self.provider_credentials.is_some(),
				// Only a subject's own admission passes its Binding snapshot;
				// its grant pins the Plan Connection next (ADR 0007).
				if admitted.is_some() {
					crate::apps::identity::repositories::provider_credentials::PlanAdmission::Deferred
				} else {
					crate::apps::identity::repositories::provider_credentials::PlanAdmission::Refused
				},
			)
			.await?;
		}
		self.event(
			tx,
			Some(claimed.workspace_id),
			"task.claimed",
			json!(claimed),
		)
		.await?;
		Ok(claimed)
	}
	pub async fn transition(
		&self,
		scope: &TenancyScope,
		id: Uuid,
		revision: i64,
		owner: &str,
		next: TaskStatus,
	) -> Result<Task> {
		let mut tx = self.pool.begin(scope).await?;
		let result = self
			.transition_in(&mut tx, id, revision, owner, next)
			.await?;
		tx.commit().await?;
		Ok(result)
	}
	pub(crate) async fn transition_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		revision: i64,
		owner: &str,
		next: TaskStatus,
	) -> Result<Task> {
		let task: Task = {
			let query_bind_1 = id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::ColumnRef::Asterisk)
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		let terminate_unclaimed = matches!(next, TaskStatus::Cancelled | TaskStatus::Failed)
			&& task.status == crate::domain::TaskStatus::Open
			&& task.owner.is_none();
		if task.owner.as_deref() != Some(owner) && !terminate_unclaimed {
			return Err(Error::Unauthorized);
		}
		let before = task.status;
		let after = next;
		if after == TaskStatus::Completed {
			return Err(Error::Invalid(
				"use completion with an idempotency key".into(),
			));
		}
		if task.status == next {
			return Ok(task);
		}
		if !before.can_transition(&after) {
			return Err(Error::Conflict(format!(
				"invalid task transition {} -> {next}",
				task.status
			)));
		}
		let t: Task = { let query_bind_1 = id; let query_bind_2 = revision; let query_bind_3 = owner; let query_bind_4 = next; aidash_server::database::query_as(&reinhardt::query::Query::update().table(reinhardt::query::Alias::new("tasks")).value_expr(reinhardt::query::Alias::new("status"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_4.as_str()).into()])).value_expr(reinhardt::query::Alias::new("owner"), SimpleExpr::CustomWithExpr("(CASE WHEN ? = 'OPEN' THEN NULL ELSE ? END)".to_owned(), vec![Expr::value(query_bind_4.as_str()).into(), Expr::value(query_bind_3.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("revision"), reinhardt::query::Expr::cust("revision + 1")).and_where(SimpleExpr::CustomWithExpr("(id = ? AND revision = ? AND (owner = ? OR (owner IS NULL AND status = 'OPEN' AND ? IN ('CANCELLED', 'FAILED'))))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.as_str()).into()])).returning_all().to_string(reinhardt::query::PostgresQueryBuilder)).fetch_optional(&mut **tx).await? }.ok_or_else(|| Error::Conflict("task revision changed".into()))?;
		self.event(tx, Some(t.workspace_id), "task.updated", json!(t))
			.await?;
		Ok(t)
	}
	/// Consume this remote run's admitted-message fences and perform an explicit
	/// cancellation or failure while holding the same authoritative task lock.
	#[allow(clippy::too_many_arguments)] // The scope joins the task, Run and fence arguments.
	pub async fn transition_remote_run_message_terminal(
		&self,
		scope: &TenancyScope,
		id: Uuid,
		revision: i64,
		owner: &str,
		next: TaskStatus,
		run_id: Uuid,
		keys: &[String],
	) -> Result<Task> {
		self.transition_remote_run_message_terminal_in(
			scope,
			id,
			revision,
			owner,
			next,
			run_id,
			TerminalRunMessageInputs::Keys(keys),
		)
		.await
	}
	#[allow(clippy::too_many_arguments)] // The scope joins the task, Run and fence arguments.
	async fn transition_remote_run_message_terminal_in(
		&self,
		scope: &TenancyScope,
		id: Uuid,
		revision: i64,
		owner: &str,
		next: TaskStatus,
		run_id: Uuid,
		inputs: TerminalRunMessageInputs<'_>,
	) -> Result<Task> {
		let mut tx = self.pool.begin(scope).await?;
		let result = self
			.transition_remote_terminal_in(&mut tx, id, revision, owner, next, run_id, inputs)
			.await?;
		tx.commit().await?;
		Ok(result)
	}
	/// Explicit Deployment Administrator abandonment preserves the failed outcome and reason
	/// while allowing the parent to finish using the remaining results.
	pub async fn abandon_task(
		&self,
		scope: &TenancyScope,
		id: Uuid,
		revision: i64,
		reason: &str,
	) -> Result<Task> {
		let mut tx = self.pool.begin(scope).await?;
		let task = self
			.abandon_task_in(&mut tx, id, revision, reason, "human")
			.await?;
		tx.commit().await?;
		Ok(task)
	}
	pub(crate) async fn abandon_task_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		revision: i64,
		reason: &str,
		actor: &str,
	) -> Result<Task> {
		nonempty(reason, "abandonment reason")?;
		let task: Task = {
			let query_bind_1 = id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		if task.revision != revision
			|| !matches!(
				task.status,
				crate::domain::TaskStatus::Failed
					| crate::domain::TaskStatus::Blocked
					| crate::domain::TaskStatus::Cancelled
			) {
			return Err(Error::Conflict(
				"only a failed, blocked or cancelled task at the current revision can be abandoned"
					.into(),
			));
		}
		let active_children: bool = {
			let query_bind_1 = id;
			crate::database::native::query_scalar(&reinhardt::query::Query::select().expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM tasks WHERE parent_id = ? AND NOT status IN ('COMPLETED', 'ABANDONED')))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).to_string(reinhardt::query::PostgresQueryBuilder)).scalar_one(&mut **tx).await?
		};
		if active_children {
			return Err(Error::Conflict(
				"resolve or abandon this task's children first".into(),
			));
		}
		let updated: Task = {
			let query_bind_1 = id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("tasks"))
					.value_expr(
						reinhardt::query::Alias::new("status"),
						reinhardt::query::Expr::cust("'ABANDONED'"),
					)
					.value_expr(
						reinhardt::query::Alias::new("revision"),
						reinhardt::query::Expr::cust("revision + 1"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.returning_all()
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		self.event(
			tx,
			Some(task.workspace_id),
			"task.abandoned",
			json!({"task":updated,"previous_status":task.status,"reason":reason,"actor":actor}),
		)
		.await?;
		Ok(updated)
	}
	pub async fn complete(
		&self,
		scope: &TenancyScope,
		id: Uuid,
		owner: &str,
		key: &str,
		artifact: &ArtifactInput,
	) -> Result<Task> {
		self.complete_from_run(
			scope,
			id,
			owner,
			key,
			artifact,
			Publication::default(),
			None,
		)
		.await
	}
	#[allow(clippy::too_many_arguments)] // The tenancy scope joins existing parameters.
	pub(crate) async fn complete_from_run(
		&self,
		scope: &TenancyScope,
		id: Uuid,
		owner: &str,
		key: &str,
		artifact: &ArtifactInput,
		publication: Publication,
		remote_run_fence: Option<(Uuid, i64)>,
	) -> Result<Task> {
		let mut tx = self.pool.begin(scope).await?;
		let result = self
			.complete_in(
				&mut tx,
				id,
				owner,
				key,
				artifact,
				publication,
				remote_run_fence,
			)
			.await?;
		tx.commit().await?;
		Ok(result)
	}
	// Preserve the existing fenced store contract while sharing the caller authority transaction.
	#[allow(clippy::too_many_arguments)]
	pub(crate) async fn complete_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		owner: &str,
		key: &str,
		artifact: &ArtifactInput,
		publication: Publication,
		remote_run_fence: Option<(Uuid, i64)>,
	) -> Result<Task> {
		nonempty(key, "idempotency key")?;
		artifact.validate()?;
		let t: Task = {
			let query_bind_1 = id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		if t.owner.as_deref() != Some(owner) {
			return Err(Error::Unauthorized);
		}
		let existing: Option<Artifact> = {
			let query_bind_1 = key;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
					))
					.from(reinhardt::query::Alias::new("artifacts"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(idempotency_key = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await?
		};
		if let Some(a) = existing {
			if a.task_id != id
				|| a.created_by != owner
				|| a.content != artifact.content
				|| a.kind != artifact.kind
				|| a.name != artifact.name
				|| t.status != crate::domain::TaskStatus::Completed
			{
				return Err(Error::Conflict(
					"completion key reused with different input".into(),
				));
			}
			self.record_output_in(
				tx,
				publication.tracked_run(),
				t.workspace_id,
				"artifact",
				a.id,
			)
			.await?;
			return Ok(t);
		}
		if t.status != crate::domain::TaskStatus::Running {
			return Err(Error::Conflict("only a running task can complete".into()));
		}
		let unresolved: bool = {
			let query_bind_1 = id;
			crate::database::native::query_scalar(&reinhardt::query::Query::select().expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM tasks WHERE parent_id = ? AND NOT status IN ('COMPLETED', 'ABANDONED')))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).to_string(reinhardt::query::PostgresQueryBuilder)).scalar_one(&mut **tx).await?
		};
		if unresolved {
			return Err(Error::Conflict("task has unresolved children".into()));
		}
		if let Some((run_id, through_seq)) = remote_run_fence {
			let unobserved: bool = {
				let query_bind_1 = id;
				let query_bind_2 = run_id;
				let query_bind_3 = through_seq;
				crate::database::native::query_scalar(&reinhardt::query::Query::select()
					.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = ? AND NOT consumed AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP) AND (run_id <> ? OR input_seq IS NULL OR input_seq > ?)))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
					.to_string(reinhardt::query::PostgresQueryBuilder))
			.scalar_one(&mut **tx)
			.await?
			};
			if unobserved {
				return Err(Error::TransactionPending);
			}
			// Keep these fences through every corrected output write. Consume them
			// atomically with terminal completion so an old worker cannot publish
			// between acknowledgement and task completion.
			{
				let query_bind_1 = id;
				let query_bind_2 = run_id;
				let query_bind_3 = through_seq;
				crate::database::native::query(
					&reinhardt::query::Query::update()
						.table(reinhardt::query::Alias::new("remote_run_message_fences"))
						.value_expr(
							reinhardt::query::Alias::new("consumed"),
							reinhardt::query::Expr::cust("TRUE"),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(task_id = ? AND run_id = ? AND NOT consumed AND input_seq <= ?)"
								.to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
								Expr::value(query_bind_3.to_owned()).into(),
							],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.execute(&mut **tx)
				.await?
			};
		}
		let a: Artifact = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = t.workspace_id;
			let query_bind_3 = id;
			let query_bind_4 = &artifact.kind;
			let query_bind_5 = &artifact.name;
			let query_bind_6 = &artifact.content;
			let query_bind_7 = owner;
			let query_bind_8 = key;
			let query_bind_9 = publication.run;
			aidash_server::database::query_as(
				&reinhardt::query::Query::insert()
					.into_table(reinhardt::query::Alias::new("artifacts"))
					.columns([
						reinhardt::query::Alias::new("id"),
						reinhardt::query::Alias::new("workspace_id"),
						reinhardt::query::Alias::new("task_id"),
						reinhardt::query::Alias::new("kind"),
						reinhardt::query::Alias::new("name"),
						reinhardt::query::Alias::new("content"),
						reinhardt::query::Alias::new("created_by"),
						reinhardt::query::Alias::new("idempotency_key"),
						reinhardt::query::Alias::new("run_id"),
						reinhardt::query::Alias::new("tenant"),
					])
					.from_subquery(
						reinhardt::query::Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_5.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_6.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_7.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_8.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_9.to_owned()).into()],
							))
							.expr(crate::database::scope::tenant_of(
								"workspaces",
								"id",
								query_bind_2,
							))
							.to_owned(),
					)
					.returning_all()
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		self.record_output_in(
			tx,
			publication.tracked_run(),
			t.workspace_id,
			"artifact",
			a.id,
		)
		.await?;
		let t: Task = {
			let query_bind_1 = id;
			let query_bind_2 = key;
			aidash_server::database::query_as(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("tasks"))
					.value_expr(
						reinhardt::query::Alias::new("status"),
						reinhardt::query::Expr::cust("'COMPLETED'"),
					)
					.value_expr(
						reinhardt::query::Alias::new("completion_key"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.value_expr(
						reinhardt::query::Alias::new("revision"),
						reinhardt::query::Expr::cust("revision + 1"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.returning_all()
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		self.event(
			tx,
			Some(t.workspace_id),
			"task.completed",
			json!({"task":t,"artifact":a}),
		)
		.await?;
		Ok(t)
	}

	pub async fn publish_artifact(
		&self,
		scope: &TenancyScope,
		task_id: Uuid,
		owner: &str,
		key: &str,
		input: &ArtifactInput,
	) -> Result<Artifact> {
		self.publish_artifact_from_run(scope, task_id, owner, key, input, Publication::default())
			.await
	}
	pub(crate) async fn publish_artifact_from_run(
		&self,
		scope: &TenancyScope,
		task_id: Uuid,
		owner: &str,
		key: &str,
		input: &ArtifactInput,
		publication: Publication,
	) -> Result<Artifact> {
		let mut tx = self.pool.begin(scope).await?;
		let result = self
			.publish_artifact_in(&mut tx, task_id, owner, key, input, publication)
			.await?;
		tx.commit().await?;
		Ok(result)
	}
	pub(crate) async fn publish_artifact_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		task_id: Uuid,
		owner: &str,
		key: &str,
		input: &ArtifactInput,
		publication: Publication,
	) -> Result<Artifact> {
		input.validate()?;
		let task: Task = {
			let query_bind_1 = task_id;
			aidash_server::database::query_as(
				&Query::select()
					.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
					.from(Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		if task.owner.as_deref() != Some(owner) {
			return Err(Error::Unauthorized);
		}
		let a: Option<Artifact> = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = task.workspace_id;
			let query_bind_3 = task_id;
			let query_bind_4 = &input.kind;
			let query_bind_5 = &input.name;
			let query_bind_6 = &input.content;
			let query_bind_7 = owner;
			let query_bind_8 = key;
			let query_bind_9 = publication.run;
			aidash_server::database::query_as(
				&Query::insert()
					.into_table(Alias::new("artifacts"))
					.columns([
						Alias::new("id"),
						Alias::new("workspace_id"),
						Alias::new("task_id"),
						Alias::new("kind"),
						Alias::new("name"),
						Alias::new("content"),
						Alias::new("created_by"),
						Alias::new("idempotency_key"),
						Alias::new("run_id"),
						Alias::new("tenant"),
					])
					.from_subquery(
						Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_5.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_6.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_7.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_8.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_9.to_owned()).into()],
							))
							.expr(crate::database::scope::tenant_of(
								"workspaces",
								"id",
								query_bind_2,
							))
							.to_owned(),
					)
					.on_conflict(
						OnConflict::columns([Alias::new("idempotency_key")])
							.do_nothing()
							.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await?
		};
		let a = match a {
			Some(a) => {
				self.event(tx, Some(task.workspace_id), "artifact.published", json!(a))
					.await?;
				a
			}
			None => {
				let a: Artifact = {
					let query_bind_1 = key;
					aidash_server::database::query_as(
						&Query::select()
							.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
							.from(Alias::new("artifacts"))
							.and_where(SimpleExpr::CustomWithExpr(
								"(idempotency_key = ?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.to_string(PostgresQueryBuilder),
					)
					.fetch_one(&mut **tx)
					.await?
				};
				if a.task_id != task_id
					|| a.kind != input.kind
					|| a.name != input.name
					|| a.content != input.content
				{
					return Err(Error::Conflict("artifact idempotency key reused".into()));
				}
				a
			}
		};
		self.record_output_in(
			tx,
			publication.tracked_run(),
			task.workspace_id,
			"artifact",
			a.id,
		)
		.await?;
		Ok(a)
	}

	pub async fn events(
		&self,
		scope: &TenancyScope,
		after: i64,
		workspace: Option<Uuid>,
		limit: i64,
	) -> Result<Vec<Event>> {
		Ok(self
			.pool
			.atomic(scope, async |tx| {
				EventRecord::after(tx, after, workspace, limit).await
			})
			.await?
			.into_iter()
			.map(Into::into)
			.collect())
	}
	pub async fn snapshot_page(
		&self,
		scope: &TenancyScope,
		workspace: Uuid,
		collection: &str,
		after: Option<Uuid>,
	) -> Result<SnapshotPage> {
		let rows = self
			.pool
			.atomic(scope, async |tx| {
				WorkspaceRecord::snapshot_rows(tx, workspace, collection, after).await
			})
			.await?;
		snapshots::page(rows, after)
	}
	/// Read one exact workspace record without relying on the bounded recent
	/// event/message collections in `snapshot`.
	pub async fn workspace_record(
		&self,
		scope: &TenancyScope,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<Value> {
		self.pool
			.atomic(scope, async |db| match kind {
				"workspace" if workspace == id => Ok(json!(Workspace::from(
					WorkspaceRecord::read(db, workspace).await?
				))),
				"workspace" => Err(Error::Invalid("workspace record not available".into())),
				"event" => Ok(json!(Event::from(
					EventRecord::in_workspace(db, workspace, id).await?
				))),
				_ => WorkspaceRecord::record(db, workspace, kind, id).await,
			})
			.await
	}

	pub async fn child_task_summary(
		&self,
		scope: &TenancyScope,
		workspace: Uuid,
		parent: Uuid,
	) -> Result<ChildTaskSummary> {
		self.pool
			.atomic(scope, async |tx| {
				TaskRecord::child_summary(tx, workspace, parent).await
			})
			.await
	}
	pub async fn snapshot(&self, scope: &TenancyScope, id: Uuid) -> Result<WorkspaceSnapshot> {
		self.pool
			.atomic(scope, async |db| {
				if let Some(tenant) = scope.tenant_name() {
					crate::authorization::remote::deployment_administrator::tenant(
						db,
						tenant.as_str(),
					)
					.await?;
				}
				Ok(WorkspaceSnapshot {
					workspace: WorkspaceRecord::read(db, id).await?.into(),
					tasks: TaskRecord::chronological(db, Some(id))
						.await?
						.into_iter()
						.map(Into::into)
						.collect(),
					artifacts: ArtifactRecord::for_workspace(db, id)
						.await?
						.into_iter()
						.map(Into::into)
						.collect(),
					events: EventRecord::recent(db, id)
						.await?
						.into_iter()
						.map(Into::into)
						.collect(),
					messages: MessageRecord::recent(db, id)
						.await?
						.into_iter()
						.map(Into::into)
						.collect(),
				})
			})
			.await
	}
	pub async fn message(
		&self,
		scope: &TenancyScope,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: Option<&str>,
	) -> Result<()> {
		self.message_record(scope, workspace, sender, content, key)
			.await
			.map(|_| ())
	}
	pub async fn message_record(
		&self,
		scope: &TenancyScope,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: Option<&str>,
	) -> Result<Message> {
		let mut tx = self.pool.begin(scope).await?;
		let message = self
			.message_in(&mut tx, workspace, sender, content, key)
			.await?;
		tx.commit().await?;
		Ok(message)
	}
	/// Reserve home-task lifetime before the executor admits a remote input.
	/// The task lock orders this operation with every terminal transition.
	pub async fn reserve_remote_run_message(
		&self,
		scope: &TenancyScope,
		task_id: Uuid,
		run_id: Uuid,
		peer_node: &str,
		key: &str,
		content: &str,
	) -> Result<()> {
		self.pool
			.atomic(scope, async |tx| {
				RemoteRunMessageFence::reserve(tx, task_id, run_id, peer_node, key, content).await
			})
			.await
	}
	/// Persist a reservation beyond its short lease, and bind the admitted input
	/// sequence when one is available. A durable fence remains active until the
	/// matching run atomically completes or transitions the home task.
	pub async fn commit_remote_run_message(
		&self,
		scope: &TenancyScope,
		task_id: Uuid,
		run_id: Uuid,
		key: &str,
		content: &str,
	) -> Result<()> {
		self.commit_remote_run_message_with_sequence(scope, task_id, run_id, key, content, None)
			.await
	}
	pub(crate) async fn commit_remote_run_message_with_sequence(
		&self,
		scope: &TenancyScope,
		task_id: Uuid,
		run_id: Uuid,
		key: &str,
		content: &str,
		input_seq: Option<i64>,
	) -> Result<()> {
		self.pool
			.atomic(scope, async |tx| {
				RemoteRunMessageFence::commit(tx, task_id, run_id, key, content, input_seq).await
			})
			.await
	}
	pub async fn release_remote_run_message(
		&self,
		scope: &TenancyScope,
		task_id: Uuid,
		run_id: Uuid,
		peer_node: &str,
		keys: &[String],
	) -> Result<()> {
		self.pool
			.atomic(scope, async |tx| {
				RemoteRunMessageFence::release(tx, task_id, run_id, peer_node, keys).await
			})
			.await
	}
	pub async fn acknowledge_remote_run_messages(
		&self,
		_task_id: Uuid,
		_run_id: Uuid,
		_keys: &[String],
	) -> Result<()> {
		// Keep the pre-terminal fence even when an older executor reports that
		// it observed the input. Only an atomic terminal transition can make a
		// stale legacy worker unable to publish or complete the home task.
		Ok(())
	}
	pub(crate) async fn run_message_output_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		output: FencedRunMessageOutput<'_>,
	) -> Result<Message> {
		let FencedRunMessageOutput {
			workspace,
			task_id,
			run_id,
			included_input_seq,
			sender,
			content,
			key,
		} = output;
		if included_input_seq < 0 {
			return Err(Error::Invalid("invalid included input sequence".into()));
		}
		let task: Task = {
			let query_bind_1 = task_id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		if task.workspace_id != workspace {
			return Err(Error::Unauthorized);
		}
		if matches!(
			task.status,
			crate::domain::TaskStatus::Completed
				| crate::domain::TaskStatus::Failed
				| crate::domain::TaskStatus::Cancelled
				| crate::domain::TaskStatus::Abandoned
		) {
			return Err(Error::Conflict("home task is terminal".into()));
		}
		let newer_input_pending: bool = {
			let query_bind_1 = task_id;
			let query_bind_2 = run_id;
			let query_bind_3 = included_input_seq;
			crate::database::native::query_scalar(&reinhardt::query::Query::select()
				.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = ? AND run_id = ? AND NOT consumed AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP) AND (input_seq IS NULL OR input_seq > ?)))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.scalar_one(&mut **tx)
		.await?
		};
		if newer_input_pending {
			return Err(Error::TransactionPending);
		}
		let _: String = crate::database::native::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust(
					"set_config('aidash.input_ledger_worker', 'true', true)",
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.scalar_one(&mut **tx)
		.await?;
		let message = self
			.message_in(tx, workspace, sender, content, Some(key))
			.await?;
		Ok(message)
	}
	pub async fn accept_run_message(
		&self,
		scope: &TenancyScope,
		run_id: Uuid,
		sender: &str,
		content: &str,
		key: &str,
		max_input_tokens: usize,
	) -> Result<()> {
		self.pool
			.atomic(scope, async |tx| {
				InputRecord::admit(
					tx,
					self.deployment.as_str(),
					run_id,
					sender,
					content,
					key,
					max_input_tokens,
				)
				.await
			})
			.await
	}
	pub(crate) async fn accept_run_message_in(
		&self,
		tx: &mut dyn TransactionExecutor,
		run_id: Uuid,
		sender: &str,
		content: &str,
		key: &str,
		max_input_tokens: usize,
	) -> Result<()> {
		InputRecord::admit(
			tx,
			self.deployment.as_str(),
			run_id,
			sender,
			content,
			key,
			max_input_tokens,
		)
		.await
	}
	/// The pages people asked `run_id` to read, oldest first (ADR 0052).
	pub async fn url_intents(
		&self,
		scope: &TenancyScope,
		run_id: Uuid,
	) -> Result<Vec<aidash_domain::research::intent::UrlIntent>> {
		let records = self
			.pool
			.atomic(scope, async |tx| {
				crate::apps::execution::models::url_intents::UrlIntentRecord::for_run(tx, run_id)
					.await
			})
			.await?;
		Ok(records.iter().map(|record| record.intent()).collect())
	}
	pub async fn run_inputs(&self, scope: &TenancyScope, run_id: Uuid) -> Result<Vec<RunInput>> {
		self.pool
			.atomic(scope, async |tx| InputRecord::for_run(tx, run_id).await)
			.await
	}
	pub(crate) async fn run_input_sequence(
		&self,
		scope: &TenancyScope,
		run_id: Uuid,
		key: &str,
		content: &str,
	) -> Result<i64> {
		self.pool
			.atomic(scope, async |tx| {
				InputRecord::matching_sequence(tx, run_id, key, content).await
			})
			.await
	}
	pub(crate) async fn run_input_high_watermark(
		&self,
		scope: &TenancyScope,
		run_id: Uuid,
	) -> Result<i64> {
		self.pool
			.atomic(scope, async |tx| {
				InputRecord::high_watermark(tx, run_id).await
			})
			.await
	}
	pub async fn pending_terminal_run_message(&self) -> Result<Option<RunMetadata>> {
		let scope = &TenancyScope::Deployment(DeploymentScopeReason::ActivationClaim);
		Ok({
			let query_bind_1 = self.deployment.as_str();
			aidash_server::database::query_as(&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk))
				.from(reinhardt::query::Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr("(NOT (home_node = ? || '/' || node) AND phase IN ('COMPLETED', 'FAILED', 'CANCELLED') AND EXISTS (SELECT 1 FROM run_inputs WHERE run_inputs.run_id = runs.id AND message_id IS NULL AND (delivery_retry_at IS NULL OR delivery_retry_at <= CURRENT_TIMESTAMP)))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()]))
				.order_by(reinhardt::query::Alias::new("updated_at"), reinhardt::query::Order::Asc)
				.limit(1)
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.fetch_optional(self.pool.in_scope(scope))
		.await?
		})
	}

	pub async fn defer_run_message_delivery(
		&self,
		scope: &TenancyScope,
		run_id: Uuid,
	) -> Result<()> {
		self.pool
			.atomic(scope, async |tx| {
				InputRecord::defer_delivery(tx, run_id).await
			})
			.await
	}
	pub async fn bind_run_input_message(
		&self,
		scope: &TenancyScope,
		run_id: Uuid,
		key: &str,
		message_id: Uuid,
	) -> Result<()> {
		self.pool
			.atomic(scope, async |tx| {
				InputRecord::bind_message(tx, run_id, key, message_id).await
			})
			.await
	}
	pub async fn import_remote_run_message(
		&self,
		scope: &TenancyScope,
		run_id: Uuid,
		key: &str,
		message: &Message,
		max_input_tokens: usize,
	) -> Result<()> {
		self.import_remote_run_messages(
			scope,
			run_id,
			&[(key.to_owned(), message.clone())],
			max_input_tokens,
		)
		.await
	}
	pub async fn import_remote_run_messages(
		&self,
		scope: &TenancyScope,
		run_id: Uuid,
		messages: &[(String, Message)],
		max_input_tokens: usize,
	) -> Result<()> {
		self.pool
			.atomic(scope, async |tx| {
				InputRecord::import_history(tx, run_id, messages, max_input_tokens).await
			})
			.await
	}
	pub(crate) async fn import_remote_run_messages_in(
		&self,
		tx: &mut dyn TransactionExecutor,
		run_id: Uuid,
		messages: &[(String, Message)],
		max_input_tokens: usize,
	) -> Result<()> {
		InputRecord::import_history(tx, run_id, messages, max_input_tokens).await
	}
	/// Import the remote home's history and new input under one run-row lock.
	#[allow(clippy::too_many_arguments)] // The tenancy scope joins existing parameters.
	pub async fn import_remote_run_messages_and_accept(
		&self,
		scope: &TenancyScope,
		run_id: Uuid,
		messages: &[(String, Message)],
		sender: &str,
		content: &str,
		key: &str,
		max_input_tokens: usize,
	) -> Result<()> {
		self.pool
			.atomic(scope, async |tx| {
				InputRecord::import_history(tx, run_id, messages, max_input_tokens).await?;
				InputRecord::admit(
					tx,
					self.deployment.as_str(),
					run_id,
					sender,
					content,
					key,
					max_input_tokens,
				)
				.await
			})
			.await
	}
	pub async fn begin_final_completion(
		&self,
		scope: &TenancyScope,
		run: &Run,
		worker: Uuid,
	) -> Result<bool> {
		let mut tx = self.pool.begin(scope).await?;
		let mut current: Run = {
			let query_bind_1 = run.id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("runs"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND lease_until > CURRENT_TIMESTAMP)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_optional(&mut *tx)
			.await?
		}
		.ok_or_else(|| Error::Conflict("worker lease lost".into()))?;
		if current.lease_owner != Some(worker)
			|| current.phase() != crate::domain::RunPhase::ToolCall
			|| current.control == crate::domain::RunControl::Cancelled
			|| current.state.failure_delivery()
		{
			return Err(Error::Conflict("worker lease lost".into()));
		}
		let stale: bool = {
			let query_bind_1 = run.id;
			let query_bind_2 = run.observed_input_seq;
			crate::database::native::query_scalar(
				&reinhardt::query::Query::select()
					.expr(SimpleExpr::CustomWithExpr(
						"(EXISTS(SELECT 1 FROM run_inputs WHERE run_id = ? AND seq > ?))"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.scalar_one(&mut *tx)
			.await?
		};
		if stale {
			return Ok(false);
		}
		current.state.tool_mut()?.finalizing = true;
		let changed = {
			let query_bind_1 = run.id;
			let query_bind_2 = worker;
			let query_bind_3 = current.stored_pending()?;
			let mut update = reinhardt::query::Query::update();
			update
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_3.to_owned()).into()],
					),
				);
			// The Completion Gate's progress, with its Completion Basis, becomes
			// durable before the Task completes: recovery from a cut after the
			// completion keeps the Run's saved context (#112).
			if !run.context.completion.is_empty() {
				update.value_expr(
					reinhardt::query::Alias::new("context"),
					SimpleExpr::CustomWithExpr(
						"(jsonb_set(context, '{completion}', ?::jsonb))".to_owned(),
						vec![Expr::value(serde_json::to_value(&run.context.completion)?).into()],
					),
				);
			}
			update.and_where(SimpleExpr::CustomWithExpr(
				"(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP)".to_owned(),
				vec![
					Expr::value(query_bind_1.to_owned()).into(),
					Expr::value(query_bind_2.to_owned()).into(),
				],
			));
			crate::database::native::query(
				&update.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?
		};
		if changed.rows_affected() != 1 {
			return Err(Error::Conflict("worker lease lost".into()));
		}
		tx.commit().await?;
		Ok(true)
	}
	/// A channel message a local Run publishes, attributed to that Run so its
	/// Source citations resolve. Only scoped authority tracks it as Run output.
	pub(crate) async fn message_from_run(
		&self,
		scope: &TenancyScope,
		run: &RunMetadata,
		sender: &str,
		content: &str,
		key: &str,
		track_output: bool,
	) -> Result<()> {
		nonempty(content, "message")?;
		let mut tx = self.pool.begin(scope).await?;
		let message = MessageRecord::append_in(
			tx.as_mut(),
			run.workspace_id,
			sender,
			content,
			key,
			Some(run.id),
		)
		.await?;
		if track_output {
			self.record_output_in(
				&mut tx,
				Some(run.id),
				run.workspace_id,
				"message",
				message.id,
			)
			.await?;
		}
		tx.commit().await?;
		Ok(())
	}
	pub(crate) async fn record_output_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		run: Option<Uuid>,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<()> {
		let Some(run) = run else {
			return Ok(());
		};
		let valid: bool = crate::database::native::query_scalar(
			&Query::select()
				.expr(Expr::exists(
					Query::select()
						.expr(Expr::value(1_i32))
						.from(Alias::new("authorization_execution"))
						.and_where(Expr::col("run_id").eq(Expr::value(run)))
						.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
						.to_owned(),
				))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **tx)
		.await?;
		if !valid {
			return Err(Error::Forbidden);
		}
		crate::apps::identity::models::AuthorizationRunRead::record_sources(
			&mut **tx,
			run,
			workspace,
			&[(kind.to_owned(), id)],
		)
		.await?;
		crate::database::native::query(
			&Query::insert()
				.into_table(Alias::new("authorization_run_outputs"))
				.columns(
					[
						"run_id",
						"workspace_id",
						"resource_kind",
						"resource_id",
						"tenant",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::value(run))
						.expr(Expr::value(workspace))
						.expr(Expr::value(kind))
						.expr(Expr::value(id))
						.expr(crate::database::scope::tenant_of("runs", "id", run))
						.to_owned(),
				)
				.on_conflict(
					OnConflict::columns(["run_id", "resource_kind", "resource_id"])
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?;

		Ok(())
	}
	pub(crate) async fn message_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: Option<&str>,
	) -> Result<Message> {
		nonempty(content, "message")?;
		self.message_in_with_attachments(tx, workspace, sender, content, key)
			.await
	}

	/// The channel may carry a media-only message. The caller must check that
	/// at least one attachment belongs to this submission before calling it.
	pub(crate) async fn message_in_with_attachments(
		&self,
		tx: &mut crate::database::native::Transaction,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: Option<&str>,
	) -> Result<Message> {
		let inserted: Option<Message> = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = workspace;
			let query_bind_3 = sender;
			let query_bind_4 = content;
			let query_bind_5 = key;
			aidash_server::database::query_as(
				&Query::insert()
					.into_table(Alias::new("messages"))
					.columns([
						Alias::new("id"),
						Alias::new("workspace_id"),
						Alias::new("sender"),
						Alias::new("content"),
						Alias::new("idempotency_key"),
						Alias::new("tenant"),
					])
					.from_subquery(
						Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_5.to_owned()).into()],
							))
							.expr(crate::database::scope::tenant_of(
								"workspaces",
								"id",
								query_bind_2,
							))
							.to_owned(),
					)
					.on_conflict(
						OnConflict::columns([Alias::new("idempotency_key")])
							.do_nothing()
							.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await?
		};
		let message = if let Some(message) = inserted {
			self.event(tx, Some(workspace), "message.created", json!(message))
				.await?;
			message
		} else {
			let message: Message = {
				let query_bind_1 = key;
				aidash_server::database::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("messages"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(idempotency_key = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **tx)
				.await?
			};
			// This path never publishes for a Run, so a Run's message is no replay.
			if message.workspace_id != workspace
				|| message.sender != sender
				|| message.content != content
				|| message.run_id.is_some()
			{
				return Err(Error::Conflict("message idempotency key reused".into()));
			}
			message
		};
		Ok(message)
	}
}

impl Store {
	pub async fn run(&self, scope: &TenancyScope, id: Uuid) -> Result<Run> {
		let mut run: Run = self
			.pool
			.atomic(scope, async |tx| RunRecord::read(tx, id).await)
			.await?
			.try_into()?;
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		super::research::resolve_entries(tx.as_mut(), id, &mut run.context.history).await?;
		tx.commit().await?;
		Ok(run)
	}
	/// List validated executable records. Inspection uses inspect_runs and retains invalid rows.
	pub async fn runs(&self, scope: &TenancyScope) -> Result<Vec<Run>> {
		let rows: Vec<RawRun> = aidash_server::database::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new("runs"))
				.order_by_expr(
					reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
						reinhardt::query::Alias::new("updated_at"),
					)),
					reinhardt::query::Order::Desc,
				)
				.limit(500)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_all(self.pool.in_scope(scope))
		.await?;
		Ok(rows
			.into_iter()
			.filter_map(|raw| raw.decode().ok())
			.collect())
	}
	pub async fn lease_run(&self, worker: Uuid, seconds: i32) -> Result<Option<Run>> {
		let scope = &TenancyScope::Deployment(DeploymentScopeReason::ActivationClaim);
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		let mut cursor = self.recovery_cursors.execution.lock().await;
		let run = Self::lease_run_in(
			tx.as_mut(),
			worker,
			seconds,
			None,
			self.deployment.as_str(),
			&mut cursor,
		)
		.await?;
		tx.commit().await?;
		Ok(run)
	}
	pub async fn renew_lease(
		&self,
		scope: &TenancyScope,
		id: Uuid,
		worker: Uuid,
		seconds: i32,
	) -> Result<bool> {
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		let renewed = RunRecord::renew(tx.as_mut(), id, worker, seconds).await?;
		tx.commit().await?;
		Ok(renewed)
	}

	/// Commits the Lease Recovery an Isolated Step charges before it runs (#604).
	pub async fn charge_lease_recovery(
		&self,
		scope: &TenancyScope,
		id: Uuid,
		worker: Uuid,
		lease_recoveries: u32,
	) -> Result<bool> {
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		let charged =
			RunRecord::charge_lease_recovery(tx.as_mut(), id, worker, lease_recoveries).await?;
		tx.commit().await?;
		Ok(charged)
	}

	pub async fn save_run(
		&self,
		scope: &TenancyScope,
		run: &Run,
		worker: Uuid,
		kind: &str,
	) -> Result<Run> {
		let mut tx = self.pool.begin(scope).await?;
		let (saved, closed) = match self.save_run_in(&mut tx, run, worker, kind).await {
			// The stale response's attempt closes as discarded once the save
			// that would have accepted it is rolled back.
			Err(Error::StaleInference) => {
				tx.rollback().await?;
				self.discard_stale_inference(scope, run.id, worker).await;
				return Err(Error::StaleInference);
			}
			result => result?,
		};
		tx.commit().await?;
		crate::apps::execution::repositories::inference::record_interruptions(&closed);
		Ok(saved)
	}
	/// Lease-fenced Context Projection save. New Context Journal entries are
	/// appended in the caller's transaction, so the journal never trails it.
	/// Returns the Inference Attempt outcomes it closed; the caller records them
	/// as interruptions only after committing.
	pub(crate) async fn save_run_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		run: &Run,
		worker: Uuid,
		kind: &str,
	) -> Result<(Run, Vec<aidash_domain::provider::progress::ProgressOutcome>)> {
		let aidash_domain::run_state::persistence::WorkerSnapshot { pending, error } =
			run.worker_snapshot(kind)?;
		if kind == "model.completed" {
			// Message admission takes this same row lock. A response is durable only
			// when every accepted input was present in its provider request.
			let _: Uuid = {
				let query_bind_1 = run.id;
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.column(reinhardt::query::Alias::new("id"))
						.from(reinhardt::query::Alias::new("runs"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.lock(reinhardt::query::LockType::Update)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_one(&mut **tx)
				.await?
			};
			let stale: bool = {
				let query_bind_1 = run.id;
				let query_bind_2 = run.included_input_seq();
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(EXISTS(SELECT 1 FROM run_inputs WHERE run_id = ? AND seq > ?))"
								.to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_one(&mut **tx)
				.await?
			};
			if stale {
				return Err(Error::StaleInference);
			}
		}
		// Source observation, Degraded context and a pinned Estimate Snapshot
		// continue the same step. A revoked summary's restoration keeps the
		// lease so the step's authority pause applies to the saved Run. An
		// adopted compaction ends its step and releases the lease so the next
		// poll can continue the Run immediately.
		let retain_lease = aidash_domain::run_state::persistence::retains_lease(kind);
		let mut safe_context = run.context.clone();
		super::research::protect_projection(tx.as_mut(), run.id, &mut safe_context).await?;
		if kind == "context.compacted"
			&& safe_context.execution_summary != run.context.execution_summary
		{
			return Err(Error::Conflict(
				"Research changed during summary adoption; retry the projection".into(),
			));
		}
		let saved: Run = { let query_bind_1 = run.id; let query_bind_2 = worker; let query_bind_3 = run.phase(); let query_bind_4 = &safe_context; let query_bind_5 = &pending; let query_bind_6 = run.step; let query_bind_7 = error; let query_bind_8 = kind != "model.completed"; let query_bind_9 = run.observed_input_seq; aidash_server::database::query_as(&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs")).value_expr(reinhardt::query::Alias::new("phase"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_3.as_str()).into()])).value_expr(reinhardt::query::Alias::new("context"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(serde_json::to_value(query_bind_4)?).into()])).value_expr(reinhardt::query::Alias::new("pending"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_5.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("step"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_6.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("error"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_7.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("revision"), reinhardt::query::Expr::cust("revision + 1")).value_expr(reinhardt::query::Alias::new("observed_input_seq"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_9.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("updated_at"), reinhardt::query::Expr::cust("CURRENT_TIMESTAMP")).value_expr(reinhardt::query::Alias::new("lease_owner"), reinhardt::query::Expr::cust(if retain_lease { "lease_owner" } else { "NULL" })).value_expr(reinhardt::query::Alias::new("lease_until"), reinhardt::query::Expr::cust(if retain_lease { "lease_until" } else { "NULL" }))
				.and_where(SimpleExpr::CustomWithExpr("(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP AND (? OR control <> 'CANCELLED'))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_8.to_owned()).into()]))
				.returning_all()
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.fetch_optional(&mut **tx)
		.await? }
		.ok_or_else(|| Error::Conflict("worker lease lost".into()))?;
		if kind == "model.completed" {
			super::source_delivery::accept(self, tx, &saved, worker).await?;
		}
		super::context_journal::append(tx, run.id, &safe_context).await?;
		// The accepted response and its attempt's outcome commit together. A Run
		// that ends here cannot be claimed again, so its orphaned attempt closes now.
		let closed = if kind == "model.completed" {
			crate::apps::execution::repositories::inference::close_pending(
				tx.as_mut(),
				self.deployment.as_str(),
				run.id,
				Some(aidash_domain::provider::progress::ProgressOutcome::Accepted),
			)
			.await?;
			Vec::new()
		} else if saved.phase().is_terminal() {
			crate::apps::execution::repositories::inference::close_pending(
				tx.as_mut(),
				self.deployment.as_str(),
				run.id,
				None,
			)
			.await?
		} else {
			Vec::new()
		};
		let mut data = json!({"run_id":saved.id,"task_id":saved.task_id,"workspace_id":saved.workspace_id,"agent_id":saved.agent_id,"phase":saved.phase(),"step":saved.step,"error":saved.error,"context_usage":saved.context.usage});
		if kind == "run.semantic_degraded"
			&& let Some(degraded) = run.recovery.degraded.recent.last()
		{
			// Only the typed cause: no endpoint, upstream body or query (ADR 0059).
			data["cause"] = serde_json::to_value(degraded.cause)?;
		}
		if kind == "run.plugin_tools_pinned" {
			// Only names and reason codes: no URL, header, finding detail or server answer (ADR 0073).
			data["skipped"] = serde_json::to_value(
				run.context
					.plugin_tools
					.as_ref()
					.map(aidash_domain::agent_plugin::PluginTools::skipped)
					.unwrap_or_default(),
			)?;
		}
		if saved.phase().is_terminal() {
			self.close_plugin_sessions_soon(saved.id);
		}
		self.run_event(
			tx,
			run.id,
			(run.is_local(self.deployment.as_str())).then_some(run.workspace_id),
			kind,
			data,
		)
		.await?;
		Ok((saved, closed))
	}
	pub async fn release_lease(&self, scope: &TenancyScope, id: Uuid, worker: Uuid) -> Result<()> {
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		RunRecord::release_worker(tx.as_mut(), id, worker).await?;
		tx.commit().await?;
		Ok(())
	}
	pub(crate) async fn pause_for_execution(
		&self,
		run: impl Into<RunMetadata>,
		worker: Uuid,
		reason: &str,
		event_kind: &str,
		refusal: Option<&aidash_domain::tool::AuthorityRefusal>,
	) -> Result<()> {
		self.pause_for_execution_reason(run, worker, reason, event_kind, None, refusal)
			.await
	}
	pub(crate) async fn cancel_execution(
		&self,
		scope: &TenancyScope,
		run: &Run,
		worker: Uuid,
	) -> Result<()> {
		self.cancel_owned_execution(scope, &run.metadata(), worker)
			.await
	}
	/// Terminal cleanup needs the durable owner and lease, never live Node admission
	/// or decoding the canceled agent's potentially newer execution state.
	pub(crate) async fn cancel_owned_execution(
		&self,
		scope: &TenancyScope,
		run: &RunMetadata,
		worker: Uuid,
	) -> Result<()> {
		let mut tx = self.pool.begin(scope).await?;
		crate::transactions::gate::read_in(&mut tx).await?;
		RunRecord::worker_context(tx.as_mut()).await?;
		let valid: Option<Uuid> = {
			let query_bind_1 = run.id;
			let query_bind_2 = worker;
			crate::database::native::query_scalar(&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("id")),
				))
				.from(reinhardt::query::Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr("(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP AND control = 'CANCELLED')".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
				.lock(reinhardt::query::LockType::Update)
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.scalar_optional(&mut *tx)
		.await?
		};
		if valid.is_none() {
			return Err(Error::Conflict("worker lease lost".into()));
		}
		let saved: crate::domain::run_state::RawRun = crate::database::query_as(
			&Query::select()
				.column(reinhardt::query::ColumnRef::Asterisk)
				.from(Alias::new("runs"))
				.and_where(Expr::col("id").eq(Expr::value(run.id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut *tx)
		.await?;
		if aidash_domain::federation::execution::settlement::has_saved_continuation(&saved) {
			let captured: bool = crate::database::native::query_scalar(
				&Query::select()
					.expr(Expr::exists(
						Query::select()
							.expr(Expr::value(1_i32))
							.from(Alias::new("node_cancellation_jobs"))
							.and_where(Expr::col("kind").eq("run"))
							.and_where(Expr::col("work_id").eq(Expr::value(run.id)))
							.and_where(
								Expr::col("tenant").in_subquery(
									Query::select()
										.column(Alias::new("tenant"))
										.from(Alias::new("runs"))
										.and_where(Expr::col("id").eq(Expr::value(run.id)))
										.to_owned(),
								),
							)
							.to_owned(),
					))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut *tx)
			.await?;
			if captured {
				return Err(Error::Conflict(
					"saved Node execution settlement is pending".into(),
				));
			}
		}
		let phase = if run.is_local(self.deployment.as_str()) {
			let task: Task = {
				let query_bind_1 = run.task_id;
				aidash_server::database::query_as(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
						))
						.from(reinhardt::query::Alias::new("tasks"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.lock(reinhardt::query::LockType::Update)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_one(&mut *tx)
				.await?
			};
			let phase = match task.status {
				crate::domain::TaskStatus::Completed => "COMPLETED",
				crate::domain::TaskStatus::Failed => "FAILED",
				_ => "CANCELLED",
			};
			if !matches!(
				task.status,
				crate::domain::TaskStatus::Completed
					| crate::domain::TaskStatus::Failed
					| crate::domain::TaskStatus::Cancelled
					| crate::domain::TaskStatus::Abandoned
			) {
				let task: Task = {
					let query_bind_1 = run.task_id;
					aidash_server::database::query_as(
						&reinhardt::query::Query::update()
							.table(reinhardt::query::Alias::new("tasks"))
							.value_expr(
								reinhardt::query::Alias::new("status"),
								reinhardt::query::Expr::cust("'CANCELLED'"),
							)
							.value_expr(
								reinhardt::query::Alias::new("revision"),
								reinhardt::query::Expr::cust("revision + 1"),
							)
							.and_where(SimpleExpr::CustomWithExpr(
								"(id = ?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.returning_all()
							.to_string(reinhardt::query::PostgresQueryBuilder),
					)
					.fetch_one(&mut *tx)
					.await?
				};
				self.event(&mut tx, Some(run.workspace_id), "task.updated", json!(task))
					.await?;
			}
			phase
		} else {
			"CANCELLED"
		};

		{
			let query_bind_1 = run.id;
			let query_bind_2 = worker;
			let query_bind_3 = phase;
			let query_bind_4 = crate::domain::run_state::encode(
				&if phase == "COMPLETED" {
					RunState::Completed(TerminalState {})
				} else {
					RunState::Cancelled(TerminalState {})
				},
				&RecoveryState::default(),
			)?;
			crate::database::native::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("runs"))
					.value_expr(
						reinhardt::query::Alias::new("phase"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.value_expr(
						reinhardt::query::Alias::new("pending"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						),
					)
					.value_expr(
						reinhardt::query::Alias::new("error"),
						reinhardt::query::Expr::cust("NULL"),
					)
					.value_expr(
						reinhardt::query::Alias::new("revision"),
						reinhardt::query::Expr::cust("revision + 1"),
					)
					.value_expr(
						reinhardt::query::Alias::new("updated_at"),
						reinhardt::query::Expr::cust("CURRENT_TIMESTAMP"),
					)
					.value_expr(
						reinhardt::query::Alias::new("lease_owner"),
						reinhardt::query::Expr::cust("NULL"),
					)
					.value_expr(
						reinhardt::query::Alias::new("lease_until"),
						reinhardt::query::Expr::cust("NULL"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND lease_owner = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?
		};
		self.run_event(
			&mut tx,
			run.id,
			run.is_local(self.deployment.as_str())
				.then_some(run.workspace_id),
			if phase == "CANCELLED" {
				"run.cancelled"
			} else {
				"run.reconciled"
			},
			json!({"run_id":run.id,"task_id":run.task_id}),
		)
		.await?;
		let closed = crate::apps::execution::repositories::inference::close_pending(
			tx.as_mut(),
			self.deployment.as_str(),
			run.id,
			None,
		)
		.await?;
		tx.commit().await?;
		crate::apps::execution::repositories::inference::record_interruptions(&closed);
		self.close_plugin_sessions_soon(run.id);
		Ok(())
	}

	/// Ends the Run's MCP sessions with its plugin servers without making the caller wait for
	/// them. A Run that pauses, fails, expires or is cancelled may not reach the harness's close, and
	/// a session closed too early reopens on the next call.
	fn close_plugin_sessions_soon(&self, run: Uuid) {
		let sessions = self.plugin_sessions.clone();
		tokio::spawn(async move {
			aidash_application::ports::tools::PluginSessions::close(sessions.as_ref(), run).await;
		});
	}

	/// Release retained sessions only after the caller's control transaction has committed.
	pub(crate) fn run_control_committed(&self, run: Uuid, control: RunControl) {
		if control == RunControl::Paused {
			self.close_plugin_sessions_soon(run);
		}
	}

	pub async fn control(
		&self,
		scope: &TenancyScope,
		id: Uuid,
		action: RunControlAction,
	) -> Result<RunInspection> {
		let mut tx = self.pool.begin(scope).await?;
		let run = self.control_in(&mut tx, id, action).await?;
		tx.commit().await?;
		self.run_control_committed(run.id, run.control);
		Ok(run)
	}

	pub(crate) async fn control_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		action: RunControlAction,
	) -> Result<RunInspection> {
		self.control_in_executor(&mut **tx, id, action).await
	}

	/// The same ledger/CAS/event owner for lifecycle transactions.
	pub(crate) async fn control_in_executor(
		&self,
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		action: RunControlAction,
	) -> Result<RunInspection> {
		self.control_in_as(tx, id, action, ControlSource::Tenant)
			.await
	}

	/// Applies a control decision and records who made it: the control revision
	/// increases and the control source is replaced, even when control itself
	/// keeps its value (a Safety Stop of an already paused Run).
	pub(crate) async fn control_in_as(
		&self,
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		action: RunControlAction,
		source: ControlSource,
	) -> Result<RunInspection> {
		let control = action.control();
		let mut resumed_pending = None;
		let mut semantic_retry = None;
		if action == RunControlAction::Resume {
			let raw: RawRun = {
				let query_bind_1 = id;
				aidash_server::database::query_as(
					&reinhardt::query::Query::select()
						.column(reinhardt::query::ColumnRef::Asterisk)
						.from(reinhardt::query::Alias::new("runs"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.lock(reinhardt::query::LockType::Update)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_one(&mut *tx)
				.await?
			};
			resumed_pending = raw.resumed_pending()?;
			semantic_retry = raw.manual_semantic_retry()?;
		}
		// Control-plane updates are emitted by upgraded code and must remain
		// available while a pre-upgrade worker lease is fenced by run_inputs.
		crate::database::native::query_scalar::<String>(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust(
					"set_config('aidash.input_ledger_worker', 'true', true)",
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.scalar_one(&mut *tx)
		.await?;
		let r: RawRun = { let query_bind_1 = id; let query_bind_2 = control; let query_bind_3 = resumed_pending; let query_bind_4 = source.as_str(); aidash_server::database::query_as(&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs")).value_expr(reinhardt::query::Alias::new("pending"), SimpleExpr::CustomWithExpr("(COALESCE(?::jsonb, pending))".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("control"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_2.as_str()).into()])).value_expr(reinhardt::query::Alias::new("error"), SimpleExpr::CustomWithExpr("(CASE WHEN ?::jsonb IS NOT NULL OR (? = 'PAUSED' AND error = 'identity status unavailable') THEN NULL ELSE error END)".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_2.as_str()).into()])).value_expr(reinhardt::query::Alias::new("revision"), reinhardt::query::Expr::cust("revision + 1")).value_expr(reinhardt::query::Alias::new("control_revision"), reinhardt::query::Expr::cust("control_revision + 1")).value_expr(reinhardt::query::Alias::new("control_source"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_4).into()])).value_expr(reinhardt::query::Alias::new("updated_at"), reinhardt::query::Expr::cust("CURRENT_TIMESTAMP"))
				.and_where(SimpleExpr::CustomWithExpr("(id = ? AND NOT phase IN ('COMPLETED', 'FAILED', 'CANCELLED') AND control <> 'CANCELLED')".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()]))
				.returning_all()
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.fetch_optional(&mut *tx)
		.await? }
		.ok_or_else(|| {
			Error::Conflict("run is terminal or cancellation is already requested".into())
		})?;
		if action == RunControlAction::Cancel {
			crate::apps::execution::repositories::live_previews::mark_stopped_for_run(
				tx,
				id,
				aidash_domain::preview::live::StopReason::RunCancelled,
			)
			.await?;
		}
		let workspace =
			(r.metadata.is_local(self.deployment.as_str())).then_some(r.metadata.workspace_id);
		if let Some(reason) = semantic_retry {
			// An authorized manual retry of a semantic pause opens the Run's next
			// Retry Cycle, so its memory operations get a fresh retry ledger while
			// keeping their operation IDs and durable budgets (ADR 0057).
			let cycle: i32 = crate::database::native::query_scalar(
				&Query::update()
					.table(Alias::new("runs"))
					.value_expr(
						Alias::new("semantic_retry_cycle"),
						Expr::col(Alias::new("semantic_retry_cycle")).add(Expr::value(1_i32)),
					)
					.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
					.returning([Alias::new("semantic_retry_cycle")])
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut *tx)
			.await?;
			event_records::append_run(
				tx,
				id,
				workspace,
				"semantic.manual_retry",
				json!({"run_id":id,"cycle":cycle,"cause":reason,"reason":"authorized_resume"}),
			)
			.await?;
		}
		event_records::append_run(
			tx,
			id,
			workspace,
			"run.control",
			// Only a decision of someone other than the Tenant names its source.
			if source == ControlSource::Tenant {
				json!({"run_id":id,"action":action})
			} else {
				json!({"run_id":id,"action":action,"source":source})
			},
		)
		.await?;
		Ok(r.inspect())
	}

	pub async fn human_request(
		&self,
		scope: &TenancyScope,
		run: &Run,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		let request = self
			.human_request_in(tx.as_mut(), run, kind, prompt, key)
			.await?;
		tx.commit().await?;
		Ok(request)
	}

	pub(crate) async fn cache_home_human(
		&self,
		scope: &TenancyScope,
		run: &Run,
		request: &HumanRequest,
		key: &str,
	) -> Result<()> {
		if request.run_id != run.id || request.workspace_id != run.workspace_id {
			return Err(Error::Forbidden);
		}
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		let (cached, created) = HumanRequestRecord::admit_home(tx.as_mut(), request, key).await?;
		human_interaction::validate_replay(&cached, run, &request.kind, &request.prompt)?;
		if cached.id != request.id {
			return Err(Error::Conflict(
				"Home human request identity changed".into(),
			));
		}
		if created {
			event_records::append(tx.as_mut(), None, "human.requested", json!(request)).await?;
		}
		tx.commit().await?;
		Ok(())
	}

	async fn human_request_in(
		&self,
		tx: &mut dyn TransactionExecutor,
		run: &Run,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		human_interaction::validate_request(kind, prompt)?;
		let (request, created) =
			HumanRequestRecord::admit(tx, run.workspace_id, run.id, kind, prompt, key).await?;
		human_interaction::validate_replay(&request, run, kind, prompt)?;
		if created {
			event_records::append_run(
				tx,
				run.id,
				(run.is_local(self.deployment.as_str())).then_some(run.workspace_id),
				"human.requested",
				json!(request),
			)
			.await?;
		}
		Ok(request)
	}

	pub(crate) async fn reconciliation_request(
		&self,
		scope: &TenancyScope,
		run: &mut Run,
		worker: Uuid,
		key: &str,
		prompt: &str,
	) -> Result<()> {
		let mut tx = self.pool.begin(scope).await?;
		let h = self
			.human_request_sqlx_in(
				&mut tx,
				run,
				"CONFIRMATION",
				prompt,
				&format!("{key}:reconcile"),
			)
			.await?;
		run.state = RunState::Waiting(Box::new(WaitingState::Reconciliation {
			request_id: h.id,
			key: key.into(),
			resume: Box::new(run.state.tool()?.clone()),
		}));
		let changed = {
			let query_bind_1 = run.id;
			let query_bind_2 = worker;
			let query_bind_3 = run.stored_pending()?;
			crate::database::native::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("runs"))
					.value_expr(
						reinhardt::query::Alias::new("pending"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.value_expr(
						reinhardt::query::Alias::new("phase"),
						reinhardt::query::Expr::cust("'WAITING'"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?
		}
		.rows_affected();
		if changed != 1 {
			return Err(Error::Conflict("worker lease lost".into()));
		}
		tx.commit().await?;
		Ok(())
	}

	pub(crate) async fn human_request_by_id(
		&self,
		scope: &TenancyScope,
		id: Uuid,
	) -> Result<HumanRequest> {
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		let request = HumanRequestRecord::read_in(tx.as_mut(), id, false)
			.await?
			.ok_or_else(|| Error::NotFound("human request".into()))?;
		tx.commit().await?;
		Ok(request)
	}

	pub async fn answer(
		&self,
		scope: &TenancyScope,
		id: Uuid,
		response: Value,
	) -> Result<HumanRequest> {
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		let request = self.answer_in(tx.as_mut(), id, response, "human").await?;
		tx.commit().await?;
		Ok(request)
	}

	/// Answer a request on the Run's own behalf, recorded as `system`.
	pub(crate) async fn close_human_request(
		&self,
		scope: &TenancyScope,
		id: Uuid,
		response: Value,
	) -> Result<HumanRequest> {
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		let request = self.answer_in(tx.as_mut(), id, response, "system").await?;
		tx.commit().await?;
		Ok(request)
	}

	pub(crate) async fn answer_in(
		&self,
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		response: Value,
		actor: &str,
	) -> Result<HumanRequest> {
		human_interaction::validate_response(&response)?;
		let old = HumanRequestRecord::read_in(tx, id, true)
			.await?
			.ok_or_else(|| Error::NotFound("human request".into()))?;
		let run = RunRecord::read_in(tx, old.run_id, None)
			.await?
			.ok_or_else(|| Error::NotFound("run".into()))?;
		let human_interaction::Answer::Write { response, actor } =
			human_interaction::answer(&old, &run, response, actor)?
		else {
			return Ok(old);
		};
		let request = HumanRequestRecord::answer_locked(tx, id, response, &actor).await?;
		event_records::append_run(
			tx,
			run.id,
			(run.is_local(self.deployment.as_str())).then_some(run.workspace_id),
			"human.answered",
			json!(request),
		)
		.await?;
		Ok(request)
	}
	#[allow(clippy::too_many_arguments)] // The tenancy scope joins existing parameters.
	pub async fn invocation_start(
		&self,
		scope: &TenancyScope,
		run: &Run,
		worker: Uuid,
		key: &str,
		tool: &str,
		input: &Value,
		replay_safe: bool,
	) -> Result<Invocation> {
		let mut tx = self.pool.begin(scope).await?;
		// An acknowledged MCP input continues the already admitted logical
		// call. New user input is consumed after it finishes, as for a live call.
		let continuation = matches!(tool, "mcp.round" | "mcp.sampling")
			&& run
				.state
				.tool()
				.ok()
				.and_then(|state| state.mcp.as_ref())
				.is_some_and(|checkpoint| {
					checkpoint.input.is_some()
						&& key.starts_with(&format!("{}:mcp:", checkpoint.key))
				});
		// A recovered send also retains its admission across newer user input.
		// The checkpoint alone cannot grant that exemption: a newly prepared
		// send is persisted together with its first journal row below.
		let sent = tool == "mcp.round"
			&& run
				.state
				.tool()
				.ok()
				.and_then(|state| state.mcp.as_ref())
				.and_then(|checkpoint| checkpoint.sent.as_ref())
				.is_some_and(|sent| sent.key == key && sent.input == *input);
		let admitted_send = if !continuation && sent {
			crate::database::native::query_scalar::<bool>(
				&Query::select()
					.expr(Expr::value(true))
					.from(Alias::new("invocations"))
					.and_where(Expr::col(Alias::new("idempotency_key")).eq(Expr::value(key)))
					.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run.id)))
					.and_where(Expr::col(Alias::new("tool")).eq(Expr::value(tool)))
					.and_where(Expr::col(Alias::new("input")).eq(Expr::value(input.clone())))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut *tx)
			.await?
			.unwrap_or(false)
		} else {
			false
		};
		if !continuation && !admitted_send {
			self.ensure_run_response_current_in(&mut tx, run.id, worker, run.included_input_seq())
				.await?;
		}
		self.persist_tool_input_in(&mut tx, run, worker).await?;
		let invocation = self
			.start_invocation_in(&mut tx, run, key, tool, input, replay_safe)
			.await?;
		tx.commit().await?;
		Ok(invocation)
	}
	/// Admit a Tool Batch in one transaction: the Run, whose ToolCall state
	/// records the batch end, and a STARTED row for every replay-safe call.
	pub async fn invocation_start_batch(
		&self,
		scope: &TenancyScope,
		run: &Run,
		worker: Uuid,
		calls: &[(&str, &str, &Value)],
	) -> Result<Vec<Invocation>> {
		let mut tx = self.pool.begin(scope).await?;
		self.ensure_run_response_current_in(&mut tx, run.id, worker, run.included_input_seq())
			.await?;
		self.persist_tool_input_in(&mut tx, run, worker).await?;
		let mut invocations = Vec::with_capacity(calls.len());
		for (key, tool, input) in calls {
			invocations.push(
				self.start_invocation_in(&mut tx, run, key, tool, input, true)
					.await?,
			);
		}
		tx.commit().await?;
		Ok(invocations)
	}
	async fn persist_tool_input_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		run: &Run,
		worker: Uuid,
	) -> Result<()> {
		// The bounded call and any prepared workspace-read chunk must survive a
		// worker crash once the idempotency key becomes durable. Keep this update
		// and its derived phase in the same transaction as invocation creation.
		let persisted = {
			let query_bind_1 = run.id;
			let query_bind_2 = worker;
			let query_bind_3 = run.stored_pending()?;
			crate::database::native::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("runs"))
					.value(reinhardt::query::Alias::new("phase"), run.phase().as_str())
					.value_expr(
						reinhardt::query::Alias::new("pending"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.value_expr(
						reinhardt::query::Alias::new("revision"),
						reinhardt::query::Expr::cust("revision + 1"),
					)
					.value_expr(
						reinhardt::query::Alias::new("updated_at"),
						reinhardt::query::Expr::cust("CURRENT_TIMESTAMP"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.and_where(
						reinhardt::query::Expr::col(reinhardt::query::Alias::new("phase"))
							.is_not_in([
								RunPhase::Completed.as_str(),
								RunPhase::Failed.as_str(),
								RunPhase::Cancelled.as_str(),
							]),
					)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?
		}
		.rows_affected();
		if persisted != 1 {
			return Err(Error::Conflict(
				"worker lease lost before persisting tool input".into(),
			));
		}
		Ok(())
	}
	async fn start_invocation_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		run: &Run,
		key: &str,
		tool: &str,
		input: &Value,
		replay_safe: bool,
	) -> Result<Invocation> {
		let created = {
			let query_bind_1 = key;
			let query_bind_2 = run.id;
			let query_bind_3 = tool;
			let query_bind_4 = input;
			let query_bind_5 = replay_safe;
			crate::database::native::query(&format!(
				"{} ON CONFLICT DO NOTHING",
				reinhardt::query::Query::insert()
					.into_table(reinhardt::query::Alias::new("invocations"))
					.columns([
						reinhardt::query::Alias::new("idempotency_key"),
						reinhardt::query::Alias::new("run_id"),
						reinhardt::query::Alias::new("tool"),
						reinhardt::query::Alias::new("input"),
						reinhardt::query::Alias::new("status"),
						reinhardt::query::Alias::new("replay_safe"),
						reinhardt::query::Alias::new("tenant"),
					])
					.from_subquery(
						reinhardt::query::Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()]
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()]
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()]
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()]
							))
							.expr(reinhardt::query::Expr::cust("'STARTED'"))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_5.to_owned()).into()]
							))
							.expr(crate::database::scope::tenant_of("runs", "id", run.id))
							.to_owned()
					)
					.to_owned()
					.to_string(reinhardt::query::PostgresQueryBuilder)
			))
			.execute(&mut *tx)
			.await?
		}
		.rows_affected()
			== 1;
		let mut invocation: Invocation = {
			let query_bind_1 = key;
			crate::database::native::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
					))
					.from(reinhardt::query::Alias::new("invocations"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(idempotency_key = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut *tx)
			.await?
		};
		if invocation.input != *input || invocation.tool != tool || invocation.run_id != run.id {
			return Err(Error::Conflict(
				"tool idempotency key reused with different input".into(),
			));
		}
		if !created
			&& invocation.status == "STARTED"
			&& (invocation.delivery_pending || invocation.replay_safe)
		{
			crate::database::native::query(
				&Query::update()
					.table(Alias::new("invocations"))
					.value(Alias::new("delivery_pending"), false)
					.value(Alias::new("uncertain_reason"), Option::<String>::None)
					.and_where(Expr::col(Alias::new("idempotency_key")).eq(Expr::value(key)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?;
		} else if !created && invocation.status != "COMPLETED" && !invocation.replay_safe {
			{
				let query_bind_1 = key;
				crate::database::native::query(
					&reinhardt::query::Query::update()
						.table(reinhardt::query::Alias::new("invocations"))
						.value_expr(
							reinhardt::query::Alias::new("status"),
							reinhardt::query::Expr::cust("'UNCERTAIN'"),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(idempotency_key = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
			invocation.status = "UNCERTAIN".into();
		}
		if created {
			self.run_event(
				tx,
				run.id,
				(run.is_local(self.deployment.as_str())).then_some(run.workspace_id),
				"tool.started",
				json!({"run_id":run.id,"tool":tool,"idempotency_key":key,"input":input}),
			)
			.await?;
		}
		Ok(invocation)
	}
	pub async fn invocation_not_delivered(
		&self,
		scope: &TenancyScope,
		run: &Run,
		worker: Uuid,
		key: &str,
		failure: &Value,
	) -> Result<u32> {
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		if !RunRecord::hold_worker(tx.as_mut(), run.id, worker).await? {
			return Err(Error::Conflict(
				"worker lease lost while recording tool delivery".into(),
			));
		}
		let count: i32 = crate::database::native::query_scalar(
			&Query::update()
				.table(Alias::new("invocations"))
				.value_expr(
					Alias::new("delivery_failures"),
					Expr::col(Alias::new("delivery_failures")).add(Expr::value(1_i32)),
				)
				.value(Alias::new("delivery_pending"), true)
				.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run.id)))
				.and_where(Expr::col(Alias::new("idempotency_key")).eq(Expr::value(key)))
				.and_where(Expr::col(Alias::new("status")).eq(Expr::value("STARTED")))
				.returning([Alias::new("delivery_failures")])
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(tx.as_mut())
		.await?;
		let count = u32::try_from(count)
			.map_err(|_| Error::Conflict("invalid tool delivery count".into()))?;
		if count > aidash_domain::semantic::OUTAGE_RETRIES
			&& InvocationRecord::complete(tx.as_mut(), run.id, key, failure).await?
		{
			event_records::append_run(
				tx.as_mut(),
				run.id,
				(run.is_local(self.deployment.as_str())).then_some(run.workspace_id),
				"tool.completed",
				json!({"run_id":run.id,"idempotency_key":key,"result":failure}),
			)
			.await?;
		}
		tx.commit().await?;
		Ok(count)
	}
	pub async fn invocation_answer_lost(
		&self,
		scope: &TenancyScope,
		run: &Run,
		worker: Uuid,
		key: &str,
	) -> Result<()> {
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		if !RunRecord::hold_worker(tx.as_mut(), run.id, worker).await? {
			return Err(Error::Conflict(
				"worker lease lost while recording lost tool answer".into(),
			));
		}
		let changed = crate::database::native::query(
			&Query::update()
				.table(Alias::new("invocations"))
				.value(Alias::new("uncertain_reason"), "answer_lost")
				.value(Alias::new("delivery_pending"), false)
				.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run.id)))
				.and_where(Expr::col(Alias::new("idempotency_key")).eq(Expr::value(key)))
				.and_where(Expr::col(Alias::new("status")).eq(Expr::value("STARTED")))
				.to_string(PostgresQueryBuilder),
		)
		.execute(tx.as_mut())
		.await?
		.rows_affected();
		if changed != 1 {
			return Err(Error::Conflict("tool invocation is not started".into()));
		}
		tx.commit().await?;
		Ok(())
	}
	pub async fn invocation_finish(
		&self,
		scope: &TenancyScope,
		run: &Run,
		worker: Uuid,
		key: &str,
		result: &Value,
	) -> Result<Value> {
		self.finish_invocation(scope, run, worker, key, result, None)
			.await
	}
	#[allow(clippy::too_many_arguments)]
	pub(crate) async fn finish_invocation(
		&self,
		scope: &TenancyScope,
		run: &Run,
		worker: Uuid,
		key: &str,
		result: &Value,
		sampling: Option<aidash_domain::provider::progress::InferenceAttemptId>,
	) -> Result<Value> {
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		if !RunRecord::hold_worker(tx.as_mut(), run.id, worker).await? {
			return Err(Error::Conflict(
				"worker lease lost while recording tool result".into(),
			));
		}
		// A research result whose Provider result account terms expired
		// meanwhile is stored as its tombstone (ADR 0058).
		let expired = crate::apps::execution::models::research::ResearchAttempt::expired_result(
			tx.as_mut(),
			run.id,
			key,
			result,
		)
		.await?;
		let result = expired.as_ref().unwrap_or(result);
		if InvocationRecord::complete(tx.as_mut(), run.id, key, result).await? {
			event_records::append_run(
				tx.as_mut(),
				run.id,
				(run.is_local(self.deployment.as_str())).then_some(run.workspace_id),
				"tool.completed",
				json!({"run_id":run.id,"idempotency_key":key,"result":result}),
			)
			.await?;
		}
		if let Some(attempt) = sampling {
			super::inference::accept_sampling(
				tx.as_mut(),
				self.deployment.as_str(),
				run.id,
				attempt,
			)
			.await?;
		}
		tx.commit().await?;
		// The caller carries on with what was stored, never the expired copy.
		Ok(result.clone())
	}
	/// Durable before provider I/O. Like other worker writes, a lost lease
	/// cannot dispatch; this lease supersedes every attempt left dispatched.
	pub async fn record_usage_dispatch(
		&self,
		scope: &TenancyScope,
		run: &Run,
		worker: Uuid,
		dispatch: &aidash_domain::provider::usage::UsageDispatch,
	) -> Result<()> {
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		if !RunRecord::hold_worker_lease(tx.as_mut(), run.id, worker).await? {
			return Err(Error::Conflict(
				"worker lease lost before dispatching inference".into(),
			));
		}
		InferenceUsage::dispatch(tx.as_mut(), run.id, dispatch).await?;
		tx.commit().await?;
		Ok(())
	}
	/// Usage observed after a lost lease is still a provider fact, so completion
	/// is not lease-fenced. It writes only a record that is still dispatched;
	/// a later dispatch has already made an abandoned record unknown.
	pub async fn complete_usage_record(
		&self,
		scope: &TenancyScope,
		run: &Run,
		attempt: Uuid,
		outcome: &aidash_domain::provider::usage::UsageOutcome,
	) -> Result<bool> {
		let mut tx = self.pool.begin(scope).await?.into_executor().await?;
		let completed = InferenceUsage::complete(tx.as_mut(), run.id, attempt, outcome).await?;
		tx.commit().await?;
		Ok(completed)
	}
	/// The newest Deployment-wide calibration samples of one key (ADR 0017).
	pub async fn calibration_samples(
		&self,
		upstream_model: &str,
		projection_version: u8,
	) -> Result<Vec<aidash_domain::provider::estimate::CalibrationSample>> {
		// Calibration samples hold per-model counts and no Tenant content.
		self.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				crate::apps::execution::models::EstimateCalibrationSample::newest(
					tx,
					upstream_model,
					i32::from(projection_version),
				)
				.await
			})
			.await
	}
	#[allow(clippy::too_many_arguments)]
	pub(crate) async fn transition_remote_terminal_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		revision: i64,
		owner: &str,
		next: TaskStatus,
		run_id: Uuid,
		inputs: TerminalRunMessageInputs<'_>,
	) -> Result<Task> {
		if !matches!(next, TaskStatus::Cancelled | TaskStatus::Failed) {
			return Err(Error::Invalid(
				"remote run-message terminal transition must be cancelled or failed".into(),
			));
		}
		let task: Task = {
			let query_bind_1 = id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		let terminate_unclaimed = task.status == crate::domain::TaskStatus::Open
			&& task.owner.is_none()
			&& matches!(next, TaskStatus::Cancelled | TaskStatus::Failed);
		if task.owner.as_deref() != Some(owner) && !terminate_unclaimed {
			return Err(Error::Unauthorized);
		}
		if task.revision != revision {
			return Err(Error::Conflict("task revision changed".into()));
		}
		if task.status != next {
			let before = task.status;
			let after = next;
			if !before.can_transition(&after) {
				return Err(Error::Conflict(format!(
					"invalid task transition {} -> {next}",
					task.status
				)));
			}
		}
		match inputs {
			TerminalRunMessageInputs::Keys(keys) => {
				for key in keys {
					{
						let query_bind_1 = id;
						let query_bind_2 = run_id;
						let query_bind_3 = key;
						crate::database::native::query(
							&reinhardt::query::Query::update()
								.table(reinhardt::query::Alias::new("remote_run_message_fences"))
								.value_expr(
									reinhardt::query::Alias::new("consumed"),
									reinhardt::query::Expr::cust("TRUE"),
								)
								.and_where(SimpleExpr::CustomWithExpr(
									"(task_id = ? AND run_id = ? AND idempotency_key = ?)"
										.to_owned(),
									vec![
										Expr::value(query_bind_1.to_owned()).into(),
										Expr::value(query_bind_2.to_owned()).into(),
										Expr::value(query_bind_3.to_owned()).into(),
									],
								))
								.to_string(reinhardt::query::PostgresQueryBuilder),
						)
						.execute(&mut **tx)
						.await?
					};
				}
			}
			TerminalRunMessageInputs::Through(through_seq) => {
				// Only a durable admission acknowledgement assigns input_seq.
				// Unadmitted reservations and inputs newer than this snapshot stay
				// active and cause the task update below to roll back atomically.
				{
					let query_bind_1 = id;
					let query_bind_2 = run_id;
					let query_bind_3 = through_seq;
					crate::database::native::query(
						&reinhardt::query::Query::update()
							.table(reinhardt::query::Alias::new("remote_run_message_fences"))
							.value(reinhardt::query::Alias::new("consumed"), true)
							.and_where(SimpleExpr::CustomWithExpr(
								"(task_id = ? AND run_id = ? AND input_seq <= ?)".to_owned(),
								vec![
									Expr::value(query_bind_1.to_owned()).into(),
									Expr::value(query_bind_2.to_owned()).into(),
									Expr::value(query_bind_3.to_owned()).into(),
								],
							))
							.to_string(reinhardt::query::PostgresQueryBuilder),
					)
					.execute(&mut **tx)
					.await?
				};
			}
		}
		if task.status == next {
			return Ok(task);
		}
		// Any concurrently reserved key that was not admitted by this executor is
		// intentionally left active; gate_remote_task_terminal then rejects this
		// update instead of losing the correction.
		let updated: Task = { let query_bind_1 = id; let query_bind_2 = revision; let query_bind_3 = owner; let query_bind_4 = next; aidash_server::database::query_as(&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("tasks")).value_expr(reinhardt::query::Alias::new("status"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_4.as_str()).into()])).value_expr(reinhardt::query::Alias::new("owner"), SimpleExpr::CustomWithExpr("(CASE WHEN ? = 'OPEN' THEN NULL ELSE ? END)".to_owned(), vec![Expr::value(query_bind_4.as_str()).into(), Expr::value(query_bind_3.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("revision"), reinhardt::query::Expr::cust("revision + 1"))
				.and_where(SimpleExpr::CustomWithExpr("(id = ? AND revision = ? AND (owner = ? OR (owner IS NULL AND status = 'OPEN' AND ? IN ('CANCELLED', 'FAILED'))))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.as_str()).into()]))
				.returning_all()
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.fetch_optional(&mut **tx)
		.await? }
		.ok_or_else(|| Error::Conflict("task revision changed".into()))?;
		self.event(
			tx,
			Some(updated.workspace_id),
			"task.updated",
			json!(updated),
		)
		.await?;
		Ok(updated)
	}
	pub(crate) async fn run_message_delivery_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		delivery: RunMessageDelivery<'_>,
	) -> Result<Message> {
		let RunMessageDelivery {
			workspace,
			task_id,
			run_id,
			sender,
			content,
			input_key,
			message_key,
		} = delivery;
		let task: Task = {
			let query_bind_1 = task_id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		if task.workspace_id != workspace {
			return Err(Error::Unauthorized);
		}
		let reservation: Option<(Uuid, String, bool, bool, bool)> = {
			let query_bind_1 = task_id;
			let query_bind_2 = run_id;
			let query_bind_3 = input_key;
			crate::database::native::query_as(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::Alias::new("run_id"))
					.column(reinhardt::query::Alias::new("content"))
					.expr_as(
						reinhardt::query::Expr::cust("expires_at IS NULL"),
						reinhardt::query::Alias::new("permanent"),
					)
					.expr_as(
						reinhardt::query::Expr::cust(
							"expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP",
						),
						reinhardt::query::Alias::new("unexpired"),
					)
					.column(reinhardt::query::Alias::new("consumed"))
					.from(reinhardt::query::Alias::new("remote_run_message_fences"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.columns(&["run_id", "content", "permanent", "unexpired", "consumed"])
			.fetch_optional(&mut **tx)
			.await?
		};
		let Some((reserved_run, reserved_content, committed, active, consumed)) = reservation
		else {
			return Err(Error::Conflict(
				"remote run message was not reserved at home".into(),
			));
		};
		if reserved_run != run_id || reserved_content != content {
			return Err(Error::Conflict(
				"remote run message reservation changed".into(),
			));
		}
		if !active && !consumed {
			return Err(Error::Conflict(
				"remote run message reservation expired".into(),
			));
		}
		if !committed
			&& !consumed
			&& matches!(
				task.status,
				crate::domain::TaskStatus::Completed
					| crate::domain::TaskStatus::Failed
					| crate::domain::TaskStatus::Cancelled
					| crate::domain::TaskStatus::Abandoned
			) {
			return Err(Error::Conflict("home task is terminal".into()));
		}
		// The database gate rejects the old home-write path during rolling
		// upgrades. Only the ledger-backed delivery endpoint sets this marker.
		crate::database::native::query_scalar::<String>(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust(
					"set_config('aidash.run_message_delivery', 'true', true)",
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.scalar_one(&mut **tx)
		.await?;
		let message = self
			.message_in(tx, workspace, sender, content, Some(message_key))
			.await?;
		{
			let query_bind_1 = task_id;
			let query_bind_2 = run_id;
			let query_bind_3 = input_key;
			let query_bind_4 = content;
			crate::database::native::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("remote_run_message_fences"))
					.value_expr(
						reinhardt::query::Alias::new("expires_at"),
						reinhardt::query::Expr::cust("NULL"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ? AND run_id = ? AND idempotency_key = ? AND content = ?)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
							Expr::value(query_bind_4.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?
		};
		Ok(message)
	}
	pub(crate) async fn reserve_remote_run_message_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		task_id: Uuid,
		run_id: Uuid,
		peer_node: &str,
		key: &str,
		content: &str,
	) -> Result<()> {
		let task: Task = {
			let query_bind_1 = task_id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		let terminal = matches!(
			task.status,
			crate::domain::TaskStatus::Completed
				| crate::domain::TaskStatus::Failed
				| crate::domain::TaskStatus::Cancelled
				| crate::domain::TaskStatus::Abandoned
		);
		let full_message_key = format!("{peer_node}:{task_id}:{key}");
		let message_exists: bool = {
			let query_bind_1 = task.workspace_id;
			let query_bind_2 = &full_message_key;
			let query_bind_3 = format!("human@{peer_node}");
			let query_bind_4 = content;
			crate::database::native::query_scalar(&reinhardt::query::Query::select()
				.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM messages WHERE workspace_id = ? AND idempotency_key = ? AND sender = ? AND content = ?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.to_owned()).into()]))
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.scalar_one(&mut **tx)
		.await?
		};
		let previous: Option<(Uuid, String, bool, bool, bool)> = {
			let query_bind_1 = task_id;
			let query_bind_2 = key;
			crate::database::native::query_as(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::Alias::new("run_id"))
					.column(reinhardt::query::Alias::new("content"))
					.expr_as(
						reinhardt::query::Expr::cust("expires_at IS NULL"),
						reinhardt::query::Alias::new("permanent"),
					)
					.expr_as(
						reinhardt::query::Expr::cust(
							"expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP",
						),
						reinhardt::query::Alias::new("unexpired"),
					)
					.column(reinhardt::query::Alias::new("consumed"))
					.from(reinhardt::query::Alias::new("remote_run_message_fences"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ? AND idempotency_key = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.columns(&["run_id", "content", "permanent", "unexpired", "consumed"])
			.fetch_optional(&mut **tx)
			.await?
		};
		if let Some((previous_run, previous_content, committed, active, consumed)) = previous {
			if previous_run != run_id || previous_content != content {
				return Err(Error::Conflict("run message idempotency key reused".into()));
			}
			if terminal {
				if committed || consumed {
					return Ok(());
				}
				if !message_exists {
					return Err(Error::Conflict("home task is terminal".into()));
				}
				{
					let query_bind_1 = task_id;
					let query_bind_2 = run_id;
					let query_bind_3 = key;
					crate::database::native::query(
						&reinhardt::query::Query::update()
							.table(reinhardt::query::Alias::new("remote_run_message_fences"))
							.value_expr(
								reinhardt::query::Alias::new("expires_at"),
								reinhardt::query::Expr::cust("NULL"),
							)
							.and_where(SimpleExpr::CustomWithExpr(
								"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
								vec![
									Expr::value(query_bind_1.to_owned()).into(),
									Expr::value(query_bind_2.to_owned()).into(),
									Expr::value(query_bind_3.to_owned()).into(),
								],
							))
							.to_string(reinhardt::query::PostgresQueryBuilder),
					)
					.execute(&mut **tx)
					.await?
				};
				return Ok(());
			}
			if !committed && !active && !terminal {
				{
					let query_bind_1 = task_id;
					let query_bind_2 = run_id;
					let query_bind_3 = key;
					crate::database::native::query(
						&reinhardt::query::Query::update()
							.table(reinhardt::query::Alias::new("remote_run_message_fences"))
							.value_expr(
								reinhardt::query::Alias::new("expires_at"),
								reinhardt::query::Expr::cust(
									"CURRENT_TIMESTAMP + INTERVAL '60 seconds'",
								),
							)
							.and_where(SimpleExpr::CustomWithExpr(
								"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
								vec![
									Expr::value(query_bind_1.to_owned()).into(),
									Expr::value(query_bind_2.to_owned()).into(),
									Expr::value(query_bind_3.to_owned()).into(),
								],
							))
							.to_string(reinhardt::query::PostgresQueryBuilder),
					)
					.execute(&mut **tx)
					.await?
				};
			}
		} else {
			if terminal && !message_exists {
				return Err(Error::Conflict("home task is terminal".into()));
			}
			{
				let query_bind_1 = task_id;
				let query_bind_2 = run_id;
				let query_bind_3 = key;
				let query_bind_4 = content;
				crate::database::native::query(
					&reinhardt::query::Query::insert()
						.into_table(reinhardt::query::Alias::new("remote_run_message_fences"))
						.columns([
							reinhardt::query::Alias::new("task_id"),
							reinhardt::query::Alias::new("run_id"),
							reinhardt::query::Alias::new("idempotency_key"),
							reinhardt::query::Alias::new("content"),
							reinhardt::query::Alias::new("expires_at"),
							reinhardt::query::Alias::new("tenant"),
						])
						.from_subquery(
							reinhardt::query::Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()],
								))
								.expr(if terminal {
									reinhardt::query::Expr::cust("NULL")
								} else {
									reinhardt::query::Expr::cust(
										"CURRENT_TIMESTAMP + INTERVAL '60 seconds'",
									)
								})
								.expr(crate::database::scope::tenant_of(
									"tasks",
									"id",
									query_bind_1,
								))
								.to_owned(),
						)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.execute(&mut **tx)
				.await?
			};
		}
		Ok(())
	}
	pub(crate) async fn commit_remote_run_message_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		task_id: Uuid,
		run_id: Uuid,
		key: &str,
		content: &str,
		input_seq: Option<i64>,
	) -> Result<()> {
		if input_seq.is_some_and(|seq| seq <= 0) {
			return Err(Error::Invalid("invalid admitted input sequence".into()));
		}
		let task: Task = {
			let query_bind_1 = task_id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		let reservation: Option<(String, bool, bool, Option<i64>)> = {
			let query_bind_1 = task_id;
			let query_bind_2 = run_id;
			let query_bind_3 = key;
			crate::database::native::query_as(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::Alias::new("content"))
					.expr_as(
						reinhardt::query::Expr::cust("expires_at IS NULL"),
						reinhardt::query::Alias::new("permanent"),
					)
					.expr_as(
						reinhardt::query::Expr::cust(
							"expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP",
						),
						reinhardt::query::Alias::new("unexpired"),
					)
					.column(reinhardt::query::Alias::new("input_seq"))
					.from(reinhardt::query::Alias::new("remote_run_message_fences"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.columns(&["content", "permanent", "unexpired", "input_seq"])
			.fetch_optional(&mut **tx)
			.await?
		};
		let Some((reserved_content, committed, active, previous_seq)) = reservation else {
			return Err(Error::Conflict(
				"remote run message was not reserved at home".into(),
			));
		};
		if reserved_content != content {
			return Err(Error::Conflict(
				"remote run message reservation changed".into(),
			));
		}
		if input_seq
			.zip(previous_seq)
			.is_some_and(|(seq, previous)| seq != previous)
		{
			return Err(Error::Conflict("run message input sequence changed".into()));
		}
		if committed && (input_seq.is_none() || input_seq == previous_seq) {
			return Ok(());
		}
		// A run-bound acknowledgement from the delegated executor describes an
		// already persisted input, not a new admission. Its exact fence remains
		// recoverable after expiry or task termination; this never reopens the task.
		if !committed
			&& input_seq.is_none()
			&& (!active
				|| matches!(
					task.status,
					crate::domain::TaskStatus::Completed
						| crate::domain::TaskStatus::Failed
						| crate::domain::TaskStatus::Cancelled
						| crate::domain::TaskStatus::Abandoned
				)) {
			return Err(Error::Conflict(
				"remote run message reservation expired before admission was committed".into(),
			));
		}
		{
			let query_bind_1 = task_id;
			let query_bind_2 = run_id;
			let query_bind_3 = key;
			let query_bind_4 = content;
			let query_bind_5 = input_seq;
			crate::database::native::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("remote_run_message_fences"))
					.value_expr(
						reinhardt::query::Alias::new("expires_at"),
						reinhardt::query::Expr::cust("NULL"),
					)
					.value_expr(
						reinhardt::query::Alias::new("input_seq"),
						SimpleExpr::CustomWithExpr(
							"(COALESCE(input_seq, ?))".to_owned(),
							vec![Expr::value(query_bind_5.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ? AND run_id = ? AND idempotency_key = ? AND content = ?)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
							Expr::value(query_bind_4.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?
		};
		Ok(())
	}
	pub(crate) async fn release_remote_run_message_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		task_id: Uuid,
		run_id: Uuid,
		peer_node: &str,
		keys: &[String],
	) -> Result<()> {
		// Deletion and task termination must use the same lock order.
		let task: Task = {
			let query_bind_1 = task_id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		for key in keys {
			let full_key = format!("{peer_node}:{task_id}:{key}");
			{
				let query_bind_1 = task_id;
				let query_bind_2 = run_id;
				let query_bind_3 = key;
				let query_bind_4 = task.workspace_id;
				let query_bind_5 = full_key;
				crate::database::native::query(&reinhardt::query::Query::delete()
					.from_table(reinhardt::query::Alias::new("remote_run_message_fences"))
					.and_where(SimpleExpr::CustomWithExpr("(task_id = ? AND run_id = ? AND idempotency_key = ? AND NOT consumed AND (expires_at IS NOT NULL OR input_seq IS NULL) AND NOT EXISTS (SELECT 1 FROM messages WHERE workspace_id = ? AND idempotency_key = ?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.to_owned()).into(), Expr::value(query_bind_5.to_owned()).into()]))
					.to_string(reinhardt::query::PostgresQueryBuilder))
			.execute(&mut **tx)
			.await?
			};
		}
		Ok(())
	}
	pub(crate) async fn run_message_has_media(
		&self,
		scope: &TenancyScope,
		message_ids: &[Uuid],
	) -> Result<bool> {
		if message_ids.is_empty() {
			return Ok(false);
		}
		let id: Option<Uuid> = {
			let query_bind_1 = message_ids;
			crate::database::native::query_scalar(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::Alias::new("id"))
					.from(reinhardt::query::Alias::new("channel_attachments"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(message_id = ANY(?))".into(),
						vec![
							Expr::cust_with_values(
								"CAST(? AS uuid[])",
								[SqlValue::Array(
									ArrayType::Uuid,
									Some(Box::new(
										query_bind_1.iter().copied().map(SqlValue::from).collect(),
									)),
								)],
							)
							.into(),
						],
					))
					.limit(1)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.scalar_optional(self.pool.in_scope(scope))
			.await?
		};
		Ok(id.is_some())
	}
	/// Borrow the caller's native transaction for both targeted and recovery claims.
	pub(crate) async fn lease_run_in(
		tx: &mut dyn TransactionExecutor,
		worker: Uuid,
		seconds: i32,
		run_id: Option<Uuid>,
		node_id: &str,
		cursor: &mut run_state::RecoveryCursor,
	) -> Result<Option<Run>> {
		aidash_application::activation::lease(
			&mut crate::bootstrap::activation_scheduling_scope(tx),
			worker,
			seconds,
			run_id,
			node_id,
			cursor,
		)
		.await
		.map_err(Into::into)
	}
}

pub use crate::apps::execution::serializers::invocations::Invocation;

pub(crate) use crate::apps::federation::remote::models::input_fences::TerminalInputs as TerminalRunMessageInputs;

use reinhardt::query::{ArrayType, Value as SqlValue};

impl Store {
	pub(crate) async fn pause_for_semantic_execution(
		&self,
		run: &Run,
		worker: Uuid,
		reason: crate::semantic::remote::Failure,
	) -> Result<()> {
		self.pause_for_execution_reason(
			run,
			worker,
			&reason.to_string(),
			"run.semantic_blocked",
			Some(("semantic_reason", serde_json::to_value(reason)?)),
			None,
		)
		.await
	}
	/// Context pauses persist their typed reason like semantic pauses, so local
	/// and remote-hosted Runs expose the same paused state, error, and event.
	pub(crate) async fn pause_for_context_execution(
		&self,
		run: &Run,
		worker: Uuid,
		reason: aidash_domain::context::recovery::Failure,
	) -> Result<()> {
		self.pause_for_execution_reason(
			run,
			worker,
			&reason.to_string(),
			"run.context_blocked",
			Some(("context_reason", serde_json::to_value(reason)?)),
			None,
		)
		.await
	}
}

impl Store {
	/// `recovery_reason` names a typed `pending.recovery` field recorded only
	/// when this pause, rather than a Deployment Administrator, stops an active Run.
	async fn pause_for_execution_reason(
		&self,
		run: impl Into<RunMetadata>,
		worker: Uuid,
		reason: &str,
		event_kind: &str,
		recovery_reason: Option<(&'static str, Value)>,
		refusal: Option<&aidash_domain::tool::AuthorityRefusal>,
	) -> Result<()> {
		let run = run.into();
		// Pausing a claimed Run is the worker's lease bookkeeping.
		let mut tx = self
			.pool
			.begin(&TenancyScope::Deployment(
				DeploymentScopeReason::ActivationClaim,
			))
			.await?;
		let (query_bind_4, query_bind_5, query_bind_6) = match recovery_reason {
			Some((field, value)) => (true, format!("{{recovery,{field}}}"), value),
			None => (false, "{recovery}".to_owned(), Value::Null),
		};
		let changed = { let query_bind_1 = run.id; let query_bind_2 = worker; let query_bind_3 = reason; crate::database::native::query(&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs")).value_expr(reinhardt::query::Alias::new("pending"), SimpleExpr::CustomWithExpr("(CASE WHEN ? AND control='ACTIVE' THEN jsonb_set(pending, ?::text[], ?::jsonb, true) ELSE pending END)".to_owned(), vec![Expr::value(query_bind_4).into(), Expr::value(query_bind_5).into(), Expr::value(query_bind_6).into()])).value_expr(reinhardt::query::Alias::new("control"), reinhardt::query::Expr::cust(
						"CASE WHEN control = 'CANCELLED' THEN control ELSE 'PAUSED' END",
					)).value_expr(reinhardt::query::Alias::new("error"), SimpleExpr::CustomWithExpr("(CASE WHEN control = 'PAUSED' AND error IS DISTINCT FROM 'identity status unavailable' THEN error ELSE ? END)".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("revision"), reinhardt::query::Expr::cust("revision + 1")).value_expr(reinhardt::query::Alias::new("updated_at"), reinhardt::query::Expr::cust("CURRENT_TIMESTAMP")).value_expr(reinhardt::query::Alias::new("lease_owner"), reinhardt::query::Expr::cust("NULL")).value_expr(reinhardt::query::Alias::new("lease_until"), reinhardt::query::Expr::cust("NULL"))
				.and_where(SimpleExpr::CustomWithExpr("(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.execute(&mut *tx)
		.await? }
		.rows_affected();
		if changed == 0 {
			return Err(Error::Conflict("worker lease lost".into()));
		}
		self.run_event(
			&mut tx,
			run.id,
			(run.is_local(self.deployment.as_str())).then_some(run.workspace_id),
			event_kind,
			// The tool and the check only; never what the call addressed.
			match refusal {
				Some(refusal) => {
					let mut payload = serde_json::to_value(refusal)?;
					payload["run_id"] = json!(run.id);
					payload["task_id"] = json!(run.task_id);
					payload
				}
				None => json!({"run_id":run.id,"task_id":run.task_id}),
			},
		)
		.await?;
		// Pausing releases the lease; the worker can no longer finish its attempt.
		let closed = crate::apps::execution::repositories::inference::close_pending(
			tx.as_mut(),
			self.deployment.as_str(),
			run.id,
			None,
		)
		.await?;
		tx.commit().await?;
		crate::apps::execution::repositories::inference::record_interruptions(&closed);
		Ok(())
	}
}
#[path = "store/run_state.rs"]
pub(crate) mod run_state;
#[path = "store/tenancies.rs"]
pub(crate) mod tenancies;

impl Store {
	async fn ensure_run_response_current_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		run_id: Uuid,
		worker: Uuid,
		included_input_seq: i64,
	) -> Result<()> {
		let valid: Option<Uuid> = {
			let query_bind_1 = run_id;
			let query_bind_2 = worker;
			crate::database::native::query_scalar(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::Alias::new("id"))
					.from(reinhardt::query::Alias::new("runs"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.scalar_optional(&mut **tx)
			.await?
		};
		if valid.is_none() {
			return Err(Error::Conflict(
				"worker lease lost before response effect".into(),
			));
		}
		let stale: bool = {
			let query_bind_1 = run_id;
			let query_bind_2 = included_input_seq;
			crate::database::native::query_scalar(
				&reinhardt::query::Query::select()
					.expr(SimpleExpr::CustomWithExpr(
						"(EXISTS(SELECT 1 FROM run_inputs WHERE run_id = ? AND seq > ?))"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.scalar_one(&mut **tx)
			.await?
		};
		if stale {
			return Err(Error::StaleInference);
		}
		Ok(())
	}
}

/// Dashboard journals expose bounded previews; durable invocation records keep
/// the original values for recovery and execution.
pub(crate) fn invocation_summary(alias: Option<&str>) -> reinhardt::query::SelectStatement {
	use reinhardt::query::{Alias, Expr, Query};
	let mut query = Query::select();
	for column in [
		"idempotency_key",
		"run_id",
		"tool",
		"status",
		"replay_safe",
		"delivery_failures",
		"delivery_pending",
		"uncertain_reason",
		"created_at",
	] {
		if let Some(table) = alias {
			query.column((Alias::new(table), Alias::new(column)));
		} else {
			query.column(Alias::new(column));
		}
	}
	for column in ["input", "result"] {
		let name = alias.map_or_else(|| column.to_owned(), |table| format!("{table}.{column}"));
		query.expr_as(Expr::cust(format!("CASE WHEN octet_length({name}::text) > 1024 THEN jsonb_build_object('truncated',true,'preview',left({name}::text,1024)) ELSE {name} END")), Alias::new(column));
	}
	query
}

impl Store {
	async fn human_request_sqlx_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		run: &Run,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		if !matches!(
			kind,
			"QUESTION" | "APPROVAL_REQUIRED" | "CONFIRMATION" | "INFORMATION_REQUEST"
		) {
			return Err(Error::Invalid("unknown human request kind".into()));
		}
		nonempty(prompt, "human request prompt")?;
		let h: Option<HumanRequest> = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = run.workspace_id;
			let query_bind_3 = run.id;
			let query_bind_4 = kind;
			let query_bind_5 = prompt;
			let query_bind_6 = key;
			aidash_server::database::query_as(
				&reinhardt::query::Query::insert()
					.into_table(reinhardt::query::Alias::new("human_requests"))
					.columns([
						reinhardt::query::Alias::new("id"),
						reinhardt::query::Alias::new("workspace_id"),
						reinhardt::query::Alias::new("run_id"),
						reinhardt::query::Alias::new("kind"),
						reinhardt::query::Alias::new("prompt"),
						reinhardt::query::Alias::new("request_key"),
						reinhardt::query::Alias::new("tenant"),
					])
					.from_subquery(
						reinhardt::query::Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_5.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_6.to_owned()).into()],
							))
							.expr(crate::database::scope::tenant_of(
								"runs",
								"id",
								query_bind_3,
							))
							.to_owned(),
					)
					.on_conflict(
						reinhardt::query::OnConflict::columns([reinhardt::query::Alias::new(
							"request_key",
						)])
						.do_nothing()
						.to_owned(),
					)
					.returning_all()
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await?
		};
		let h = match h {
			Some(h) => {
				self.run_event(
					tx,
					run.id,
					(run.is_local(self.deployment.as_str())).then_some(run.workspace_id),
					"human.requested",
					json!(h),
				)
				.await?;
				h
			}
			None => {
				let query_bind_1 = key;
				aidash_server::database::query_as(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
						))
						.from(reinhardt::query::Alias::new("human_requests"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(request_key = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_one(&mut **tx)
				.await?
			}
		};
		if h.run_id != run.id || h.kind != kind || h.prompt != prompt {
			return Err(Error::Conflict(
				"human request key reused with different input".into(),
			));
		}
		Ok(h)
	}
}

fn require_projection(
	snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
	cache_salt: bool,
) -> Result<()> {
	let agent = crate::registry::AgentConfig::from_snapshot(snapshot)?;
	let version = agent.projection_version.unwrap_or_default();
	let contract = agent.harness_contract.unwrap_or_default();
	if !contract.is_implemented() {
		return Err(Error::Invalid(format!(
			"Harness Contract {contract} is not implemented"
		)));
	}
	if !contract.supports(version) {
		return Err(Error::Invalid(format!(
			"Harness Contract {contract} does not support Projection Version {version}"
		)));
	}
	if version.is_legacy() {
		return Ok(());
	}
	if !version.is_implemented() {
		return Err(Error::Invalid(format!(
			"Projection Version {version} is not implemented"
		)));
	}
	let model = snapshot
		.definitions
		.iter()
		.find(|d| {
			d.identity.id == agent.model.id
				&& d.identity.version == agent.model.version
				&& d.identity.registry_deployment == snapshot.agent.registry_deployment
		})
		.ok_or_else(|| Error::Invalid("pinned model is outside the Binding snapshot".into()))?;
	let model: aidash_domain::model::ModelConfig =
		serde_json::from_value(model.definition.config.clone())?;
	if !model.supports_projection(version) {
		return Err(Error::Invalid(format!(
			"model {} does not support Projection Version {version}",
			model.model_id
		)));
	}
	if version.salted() && !cache_salt {
		return Err(Error::Invalid(format!(
			"this node has no Cache Salt Key for Projection Version {version}"
		)));
	}
	Ok(())
}

#[cfg(test)]
#[path = "../tests/targeted_leases.rs"]
mod targeted_leases;

#[cfg(test)]
#[path = "../tests/store_tool_parallelism.rs"]
mod store_tool_parallelism;
