//! Tool ports retain the exact Access mutex guard through authorization and disclosure.
use crate::{
	Result as NativeResult,
	apps::identity::services::{access::Access, catalog},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::authorization::tools::{AgentToolRepository, AgentToolScope},
};
use aidash_domain::{
	HumanRequest, Run, RunMetadata, Task,
	federation::tool_lists::{OwnedRemoteToolLists, ToolList},
	nodes::{NodeAddress, registration::NodeRegistration},
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};
use uuid::Uuid;

pub(crate) struct AgentTools<'a> {
	pub(crate) remote: Option<&'a Federation>,
	pub(crate) access: &'a Arc<RwLock<Access>>,
	pub(crate) run: &'a Run,
}
struct Scope<'a> {
	remote: Option<&'a Federation>,
	access: OwnedRwLockWriteGuard<Access>,
	run: &'a Run,
}
#[async_trait]
impl AgentToolRepository for AgentTools<'_> {
	fn is_remote(&self) -> bool {
		self.remote.is_some()
	}
	fn binding_snapshot(&self) -> Option<&aidash_domain::registry::bindings::BindingSnapshot> {
		self.run.context.binding_snapshot.as_deref()
	}
	fn plugin_tools(&self) -> Option<&aidash_domain::agent_plugin::PluginTools> {
		self.run.context.plugin_tools.as_ref()
	}
	async fn lease(&self) -> Result<Box<dyn AgentToolScope + '_>> {
		Ok(Box::new(Scope {
			remote: self.remote,
			access: self.access.clone().write_owned().await,
			run: self.run,
		}))
	}
	async fn grant_output(&self, task: Uuid) -> Result<bool> {
		// Only the Home records what a Remote Grant's Run created there (#654).
		let Some(federation) = self.remote else {
			return Ok(false);
		};
		let home = crate::federation::Home::new(federation.clone(), self.run.clone());
		let answer: Value = home
			.command("created_task", serde_json::json!({"task_id": task}))
			.await?;
		Ok(answer["created"] == Value::Bool(true))
	}
}
impl Scope<'_> {
	/// What this Node's people allowed this Run to use for this Run only (ADR 0139), kept on
	/// its admission. A Run without an admission, or one never answered, has none.
	async fn run_allowance(&mut self) -> NativeResult<ToolList> {
		use reinhardt::query::{
			Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
		};
		let stored: Option<Value> = crate::database::native::query_scalar(
			&Query::select()
				.expr(Expr::cust("COALESCE(run_tool_allowance, '[]'::jsonb)"))
				.from(Alias::new("authorization_remote_admissions"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(self.run.id)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **self.access.tx)
		.await?;
		Ok(match stored {
			Some(value) => serde_json::from_value(value)?,
			None => ToolList::default(),
		})
	}
	/// This Node's own Node Registration of the Run's Home Node, if it has one.
	async fn home_registration(&mut self) -> Result<Option<NodeRegistration>> {
		let run = self.run.metadata();
		let Ok(home) = NodeAddress::parse(&run.home_node) else {
			return Err(aidash_application::Error::Forbidden);
		};
		let tenant = self.access.identity.tenant.clone();
		Ok(super::node_registrations::get(
			self.access.tx.as_mut(),
			&tenant,
			run.node.as_str(),
			&home,
		)
		.await?)
	}
}
#[async_trait]
impl AgentToolScope for Scope<'_> {
	async fn subnode_allowance(&mut self) -> Result<Option<aidash_domain::subnodes::Allowance>> {
		use crate::apps::execution::models::subnodes::{SubnodeMemberRecord, SubnodeRecord};
		use reinhardt::db::orm::Model;
		let tenant = self.access.identity.tenant.clone();
		let rows = SubnodeRecord::objects()
			.filter(SubnodeRecord::field_tenant().eq(tenant.clone()))
			.filter(SubnodeRecord::field_thread_id().eq(self.run.thread_id))
			.all_with_executor(self.access.tx.native_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		let Some(row) = rows.into_iter().next() else {
			return Ok(None);
		};
		if row.state != "active" {
			return Err(aidash_application::Error::Forbidden);
		}
		let members = SubnodeMemberRecord::objects()
			.filter(SubnodeMemberRecord::field_tenant().eq(tenant))
			.filter(SubnodeMemberRecord::field_subnode_id().eq(row.id))
			.filter(SubnodeMemberRecord::field_agent_id().eq(self.run.agent_id.clone()))
			.filter(SubnodeMemberRecord::field_agent_version().eq(self.run.agent_version.clone()))
			.all_with_executor(self.access.tx.native_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		if members.is_empty() {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(Some(serde_json::from_value(row.allowance.0)?))
	}
	fn transaction_active(&self) -> bool {
		self.access.tx.is_active()
	}
	async fn suspend(&mut self) -> Result<()> {
		self.access.suspend().await.map_err(Into::into)
	}
	async fn replace_remote_authority(&mut self) -> Result<bool> {
		let Some(f) = self.remote else {
			return Ok(false);
		};
		let fresh =
			super::super::services::peer::admission::worker_lease(f, &self.run.metadata()).await?;
		let Some((fresh, _)) = fresh else {
			return Ok(false);
		};
		*self.access = fresh;
		Ok(true)
	}
	async fn remote_tool_lists(&mut self) -> Result<Option<OwnedRemoteToolLists>> {
		if self.remote.is_none() {
			// A Subnode member runs here as a Run of its own, but answers to the lists of the Run
			// that created its Subnode (ADR 0110), whose admission and Node Registration hold them.
			use crate::apps::execution::repositories::operations::remote_tools;
			let bound = remote_tools::bounding_run(self.access.tx.as_mut(), self.run.id).await?;
			return Ok(remote_tools::lists(self.access.tx.as_mut(), bound).await?);
		}
		// Whether the Home is another Tenant's Node is this Node's own knowledge: only then
		// does it hold a Node Registration of it. A description without a Request never
		// turns another Tenant's Home into a same-Tenant one; it asks for nothing instead.
		let request = self.access.home_tool_request.clone();
		let Some(registration) = self.home_registration().await? else {
			// A Home of this Tenant needs no registration and sends no Request.
			return match request {
				None => Ok(None),
				// A Home that asks for tools but that no registration names is not accepted.
				Some(_) => Err(aidash_application::Error::Forbidden),
			};
		};
		Ok(Some(OwnedRemoteToolLists {
			request: request.map(|request| request.tools).unwrap_or_default(),
			allowance: registration.tool_allowance,
			block: registration.tool_block,
			// The Home's Run-only answers already travel inside its Request; only the
			// receiving Node's own are read here.
			run_request: ToolList::default(),
			run_allowance: self.run_allowance().await?,
		}))
	}
	async fn remote_tool_block(&mut self) -> Result<Option<ToolList>> {
		Ok(self
			.home_registration()
			.await?
			.map(|registration| registration.tool_block))
	}
	fn context(&self) -> &Value {
		&self.access.context
	}
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str, tool: &str) -> Result<Entry> {
		catalog::tool_entry(&mut self.access, reference, action, tool)
			.await
			.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn explicitly_denied(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		for subject in self.access.subjects.clone() {
			let input = self.access.evaluation(&subject, resource, action);
			let bundle = &self.access.snapshot.bundle;
			if Access::policy_explicit_deny(bundle, &bundle.evaluate(&input)) {
				return Ok(true);
			}
		}
		Ok(false)
	}
	async fn task_read(&mut self, task: Uuid) -> Result<Task> {
		self.access.task_read(task).await.map_err(Into::into)
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn artifact_creation_resource(&mut self, task: Uuid, creator: &str) -> Result<Resource> {
		self.access
			.artifact_creation_resource(task, creator)
			.await
			.map_err(Into::into)
	}
	async fn memory_resource(&mut self, run: &RunMetadata) -> Result<Resource> {
		self.access
			.memory_resource(run.clone())
			.await
			.map_err(Into::into)
	}
	async fn human_request(&mut self, run: &RunMetadata, id: Uuid) -> Result<Option<HumanRequest>> {
		let result: NativeResult<Option<HumanRequest>> = async {
			Ok({
				let query_bind_1 = id;
				let query_bind_2 = run.id;
				let query_bind_3 = run.thread_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("human_requests"))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
										.eq(SimpleExpr::CustomWithExpr(
											"(?)".to_owned(),
											vec![Expr::value(query_bind_1.to_owned()).into()],
										)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"run_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"thread_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_3.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource> {
		self.access
			.human_resource(request)
			.await
			.map_err(Into::into)
	}
}

use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait as _, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
