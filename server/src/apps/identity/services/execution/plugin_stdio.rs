//! What a Run's stdio MCP sessions need from this Deployment (ADR 0073), read under the Run's
//! own authority: its Execution Pod with the Run credential, the Tenant's approval of the
//! built-in `mcp_stdio` group, the plugin copy's files and `${PLUGIN_DATA}`.
//!
//! `${PLUGIN_DATA}` has no store yet: nothing is restored, and every save fails with
//! `plugin_data_store_unavailable`, which the Run's events record. The Pod keeps the live data
//! of each Run until the Scratch Space is reset.
use super::{Access, Guard};
use crate::{apps::registry::repositories::agent_plugins, domain::Run, store::Store};
use aidash_application::{
	Error, Result,
	ports::tools::{StdioPluginHost, StdioPod},
	tenancy::TenancyScope,
};
use aidash_domain::agent_plugin::{PluginFile, PluginName, stdio_session::mcp_stdio_bundle};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

pub(crate) const DATA_STORE_UNAVAILABLE: &str = "plugin_data_store_unavailable";

impl Guard {
	/// The host of this Run's stdio sessions, reading under the step's authority.
	pub(crate) fn stdio_host(&self, store: &Store) -> Arc<dyn StdioPluginHost> {
		Arc::new(StdioHost {
			store: store.clone(),
			access: self.access.clone(),
			run: self.run.clone(),
			tenancy: self.tenancy.clone(),
		})
	}
}

struct StdioHost {
	store: Store,
	access: Arc<RwLock<Access>>,
	run: Run,
	tenancy: TenancyScope,
}

#[async_trait]
impl StdioPluginHost for StdioHost {
	// Each read runs in its own short transaction under the Run's lease, committed before any
	// Pod call.
	async fn pod(&self) -> Result<Option<StdioPod>> {
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let result = crate::apps::execution::repositories::operations::pods::tool_context(
			&self.store,
			&mut access,
			&self.run.metadata(),
		)
		.await
		.map_err(Into::into);
		let context = access.finish(result).await?;
		Ok(context.map(|context| StdioPod {
			runtime: context.runtime,
			settings: context.settings,
			credential: context.credential,
		}))
	}

	async fn approved(&self) -> Result<bool> {
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let tenant = access.identity.tenant.clone();
		let result = crate::apps::identity::repositories::catalog_approvals::approved(
			&mut access.tx,
			&tenant,
			&self.run.node,
			&mcp_stdio_bundle(),
		)
		.await
		.map_err(Into::into);
		Ok(access.finish(result).await?)
	}

	async fn admit_call(&self) -> Result<()> {
		if !self.store.capabilities.0.admission {
			return Err(Error::Refused("CAPABILITIES_DISABLED".into()));
		}
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let result = crate::apps::execution::repositories::subnodes::require_writable(
			access.tx.native_mut(),
			self.run.workspace_id,
		)
		.await;
		let tenant = access.identity.tenant.clone();
		access.finish(result).await?;
		aidash_application::usage::require(
			self.store.admission.as_ref(),
			&tenant,
			aidash_application::usage::Work::SandboxedTool,
		)
		.await
	}

	async fn plugin_files(&self, digest: &str) -> Result<Vec<PluginFile>> {
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let tenant = access.identity.tenant.clone();
		let result = agent_plugins::files_of(&mut **access.tx, &tenant, digest)
			.await
			.and_then(|files| {
				files.ok_or_else(|| crate::Error::NotFound("Agent Plugin copy".into()))
			});
		Ok(access.finish(result).await?)
	}

	async fn load_data(&self, _plugin: &PluginName) -> Result<Option<Vec<PluginFile>>> {
		Ok(None)
	}

	async fn save_data(&self, _plugin: &PluginName, _files: Vec<PluginFile>) -> Result<()> {
		Err(Error::Conflict(DATA_STORE_UNAVAILABLE.into()))
	}

	async fn data_not_saved(&self, plugin: &PluginName, cause: &str) {
		let workspace = self
			.run
			.is_local(self.store.deployment.as_str())
			.then_some(self.run.workspace_id);
		let data = json!({"plugin": plugin.as_str(), "cause": cause});
		let result = async {
			let mut tx = self.store.pool.begin(&self.tenancy).await?;
			self.store
				.run_event(
					&mut tx,
					self.run.id,
					workspace,
					"agent_plugin_data_not_saved",
					data,
				)
				.await?;
			tx.commit().await?;
			crate::Result::Ok(())
		}
		.await;
		if let Err(error) = result {
			tracing::warn!(run_id = %self.run.id, %error, "plugin data failure not recorded");
		}
	}
}
