//! What a Run's stdio MCP sessions need from this Deployment (ADR 0073), read under the Run's
//! own authority: its Execution Pod with the Run credential, the Tenant's approval of the
//! built-in `mcp_stdio` group, the plugin copy's files and `${PLUGIN_DATA}`.
//!
//! `${PLUGIN_DATA}` is kept in `agent_plugin_data`, one current version per Tenant, Agent and
//! plugin, read and replaced in the Run's Tenant scope.
use super::{Access, Guard};
use crate::apps::execution::models::agent_plugin_data::AgentPluginData;
use crate::{apps::registry::repositories::agent_plugins, domain::Run, store::Store};
use aidash_application::{
	Error, Result,
	ports::tools::{StdioPluginHost, StdioPod},
	tenancy::TenancyScope,
};
use aidash_domain::agent_plugin::{
	PluginFile, PluginName,
	stdio_session::{self, mcp_stdio_bundle},
};
use async_trait::async_trait;
use chrono::Utc;
use reinhardt::db::backends::TransactionExecutor as _;
use reinhardt::db::orm::{Model as _, execution::convert_values};
use reinhardt::query::{
	Alias, Expr, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

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

fn hex(bytes: &[u8]) -> String {
	bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct StdioHost {
	store: Store,
	access: Arc<RwLock<Access>>,
	run: Run,
	tenancy: TenancyScope,
}

impl StdioHost {
	fn tenant(&self) -> Result<String> {
		Ok(self
			.tenancy
			.tenant_name()
			.ok_or(Error::Forbidden)?
			.as_str()
			.to_owned())
	}
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
		let result = async {
			crate::apps::execution::repositories::subnodes::require_writable(
				access.tx.native_mut(),
				self.run.workspace_id,
			)
			.await?;
			// The bound shell's `tool.invoke` (capabilities/files.rs `admit`), decided for the
			// Run's principals on the `mcp_stdio` operation and recorded like any decision.
			let operation = stdio_session::mcp_stdio_operation();
			let entry = crate::apps::identity::repositories::catalog_approvals::definition(
				&mut access.tx,
				&operation,
			)
			.await?;
			let resource = access.resource(
				"tool",
				format!(
					"{}/tools/{}@{}",
					self.store.deployment.as_str(),
					operation.id,
					operation.version
				),
				aidash_application::authorization::catalog::attributes(&entry),
			);
			access.require(&resource, "tool.invoke").await
		}
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

	async fn load_data(&self, plugin: &PluginName) -> Result<Option<Vec<PluginFile>>> {
		let tenant = self.tenant()?;
		let mut tx = self.store.pool.begin(&self.tenancy).await?;
		let saved = AgentPluginData::objects()
			.filter(AgentPluginData::field_tenant().eq(tenant))
			.filter(AgentPluginData::field_agent_id().eq(self.run.agent_id.clone()))
			.filter(AgentPluginData::field_plugin().eq(plugin.as_str().to_owned()))
			.all_with_executor(&mut *tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		tx.commit().await?;
		let Some(saved) = saved.into_iter().next() else {
			return Ok(None);
		};
		// A damaged version is refused rather than restored.
		if hex(&Sha256::digest(&saved.data)) != saved.sha256 {
			return Err(Error::Conflict("PLUGIN_DATA_CORRUPT".into()));
		}
		stdio_session::unpack_data(&saved.data)
			.map(Some)
			.ok_or_else(|| Error::Conflict("PLUGIN_DATA_CORRUPT".into()))
	}

	async fn save_data(&self, plugin: &PluginName, files: Vec<PluginFile>) -> Result<()> {
		stdio_session::check_data(
			files
				.iter()
				.map(|file| (file.path.as_str(), file.bytes.len() as u64)),
			false,
		)
		.map_err(|cause| Error::Invalid(cause.into()))?;
		let size: u64 = files.iter().map(|file| file.bytes.len() as u64).sum();
		let count = files.len();
		let data = stdio_session::pack_data(&files);
		let digest = hex(&Sha256::digest(&data));
		let (sql, values) = Query::insert()
			.into_table(Alias::new(AgentPluginData::table_name()))
			.columns(
				[
					"id", "tenant", "agent_id", "plugin", "data", "size", "files", "sha256",
					"saved_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(Uuid::new_v4()))
					.expr(Expr::value(self.tenant()?))
					.expr(Expr::value(self.run.agent_id.clone()))
					.expr(Expr::value(plugin.as_str().to_owned()))
					.expr(Expr::value(data))
					.expr(Expr::value(i64::try_from(size).unwrap_or(i64::MAX)))
					.expr(Expr::value(i32::try_from(count).unwrap_or(i32::MAX)))
					.expr(Expr::value(digest))
					.expr(Expr::value(Utc::now()))
					.to_owned(),
			)
			// One current version: a save replaces the previous one whole.
			.on_conflict(
				OnConflict::columns(["tenant", "agent_id", "plugin"].map(Alias::new))
					.update_columns(["data", "size", "files", "sha256", "saved_at"].map(Alias::new))
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		let mut tx = self.store.pool.begin(&self.tenancy).await?;
		tx.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		tx.commit().await?;
		Ok(())
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
