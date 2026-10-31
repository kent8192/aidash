//! Conversation writes retain the same access transaction and audit context.
use crate::apps::identity::models::AuthorizationWorkspace;
use crate::apps::workspaces::models::Conversation as ConversationRecord;
use crate::semantic::{native_memory, repositories::bindings, service::Lease};
use crate::{
	authorization::{access::Access, catalog, execution, policy::Resource},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::workspaces::{ConversationScope, MemoryEmbedding},
};
use aidash_domain::{
	Conversation, NewTask, Task, Workspace,
	federation::Delegation,
	registry::{EntityRef, Entry},
	semantic::indexing::IndexingSpec,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::json;
use uuid::Uuid;

pub(crate) struct NativeConversation<'a> {
	pub(crate) federation: &'a Federation,
	pub(crate) access: &'a mut Access,
	workspace: Option<Resource>,
	workspace_id: Option<Uuid>,
}
impl<'a> NativeConversation<'a> {
	pub(crate) fn new(federation: &'a Federation, access: &'a mut Access) -> Self {
		Self {
			federation,
			access,
			workspace: None,
			workspace_id: None,
		}
	}
}
#[async_trait]
impl ConversationScope for NativeConversation<'_> {
	async fn create_workspace(
		&mut self,
		title: &str,
		goal: &str,
		node: Option<&aidash_domain::nodes::NodeName>,
	) -> Result<Workspace> {
		self.access
			.create_workspace(&self.federation.store, title, goal, node)
			.await
			.map_err(Into::into)
	}
	async fn workspace_context(&mut self, workspace: Uuid) -> Result<()> {
		let resource = self.access.workspace(workspace).await?;
		self.access.context = resource.attributes.clone();
		self.workspace = Some(resource);
		self.workspace_id = Some(workspace);
		Ok(())
	}
	async fn require_workspace(&mut self, action: &str) -> Result<()> {
		let resource = self
			.workspace
			.as_ref()
			.expect("workspace context initialized by conversation use case");
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn registry_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		catalog::entry(self.access, reference, action)
			.await
			.map_err(Into::into)
	}
	async fn create_conversation(
		&mut self,
		workspace_id: Uuid,
		target: &EntityRef,
		target_kind: &str,
	) -> Result<Conversation> {
		let conversation: ConversationRecord = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = workspace_id;
			let query_bind_3 = format!("{}@{}", target.id, target.version);
			let query_bind_4 = target_kind;
			let query_bind_5 = &self.access.identity.subject;
			aidash_server::database::query_as(
				&Query::insert()
					.into_table(Alias::new("conversations"))
					.columns([
						Alias::new("id"),
						Alias::new("workspace_id"),
						Alias::new("target"),
						Alias::new("target_kind"),
						Alias::new("created_by"),
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
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **self.access.tx)
			.await?
		};
		Ok(conversation.into())
	}
	async fn require_conversation(
		&mut self,
		conversation: &Conversation,
		action: &str,
	) -> Result<()> {
		let resource = self.access.conversation_resource(conversation).await?;
		self.access
			.require(&resource, action)
			.await
			.map_err(Into::into)
	}
	async fn conversation_event(&mut self, conversation: &Conversation) -> Result<()> {
		self.federation
			.store
			.event(
				&mut self.access.tx,
				Some(conversation.workspace_id),
				"conversation.created",
				json!(conversation),
			)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn message(&mut self, workspace: Uuid, content: &str) -> Result<()> {
		self.federation
			.store
			.message_in(
				&mut self.access.tx,
				workspace,
				&self.access.identity.subject,
				content,
				None,
			)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn create_task(&mut self, workspace: Uuid, input: &NewTask) -> Result<Task> {
		self.federation
			.store
			.create_task_in(
				&mut self.access.tx,
				workspace,
				input,
				&self.access.identity.subject,
				None,
			)
			.await
			.map_err(Into::into)
	}
	async fn require_task(&mut self, task: &Task, action: &str) -> Result<()> {
		let resource = self.access.task_resource(task).await?;
		self.access
			.require(&resource, action)
			.await
			.map_err(Into::into)
	}
	async fn bind_task(&mut self, task: Uuid, conversation: Uuid) -> Result<()> {
		crate::capabilities::sessions::bind_task(self.access, task, conversation)
			.await
			.map_err(Into::into)
	}
	/// The same resolution a local claim makes before it pins the memory bank.
	async fn memory_embedding(&mut self, agent: &EntityRef) -> Result<Option<MemoryEmbedding>> {
		let workspace = self
			.workspace_id
			.expect("workspace context initialized by conversation use case");
		let entry = catalog::entry(self.access, agent, "registry.read").await?;
		let tenant = AuthorizationWorkspace::tenant_in(&mut **self.access.tx, workspace)
			.await?
			.ok_or(aidash_application::Error::Forbidden)?;
		// The agent's dependencies are used by the agent, so resolve them with
		// the subject chain the claim builds: the executing agent appended
		// after whoever starts the conversation. Restore the chain and the
		// approvals cached under it before any check made for the person.
		let node = self.access.workspace_node(workspace).await?;
		let subject = aidash_domain::qualified_agent(
			&self.federation.config.deployment.node(node),
			&agent.id,
			&agent.version,
		);
		let mut subjects = self.access.subjects.clone();
		subjects.push(subject);
		let subjects = std::mem::replace(&mut self.access.subjects, subjects);
		let approved = std::mem::take(&mut self.access.approved_catalog);
		let deployment = self.federation.store.deployment.as_str();
		let resolved = memory_embedding_of(self.access, &tenant, &entry, deployment).await;
		self.access.subjects = subjects;
		self.access.approved_catalog = approved;
		resolved
	}
	async fn create_memory_index(
		&mut self,
		conversation: &Conversation,
		agent: &EntityRef,
		provider: &EntityRef,
		spec: &IndexingSpec,
	) -> Result<()> {
		let workspace = conversation.workspace_id;
		let revision = crate::semantic::repositories::mutations::create_first_index(
			&self.federation.store,
			&mut **self.access.tx,
			workspace,
			spec,
			"new immutable index generation for the conversation target's memory",
		)
		.await?;
		self.federation
			.store
			.event(
				&mut self.access.tx,
				Some(workspace),
				"conversation.index_created",
				// `id` names the conversation, whose readers may see this event.
				json!({
					"id": conversation.id,
					"workspace_id": workspace,
					"revision": revision,
					"agent": agent,
					"memory_provider": provider,
				}),
			)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn delegate(&mut self, task: Uuid, agent: &EntityRef) -> Result<Delegation> {
		execution::delegate_in(self.federation, self.access, task, agent)
			.await
			.map_err(Into::into)
	}
	async fn task(&mut self, task: Uuid) -> Result<Task> {
		let sql = Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("tasks"))
			.and_where(reinhardt::query::ExprTrait::eq(
				Expr::col("id"),
				Expr::value(task),
			))
			.to_string(PostgresQueryBuilder);
		Ok(crate::database::query_as(&sql)
			.fetch_one(&mut **self.access.tx)
			.await?)
	}
}

/// The memory provider `entry` binds and its policy's embedding, resolved with
/// the subject chain already on `access`.
async fn memory_embedding_of(
	access: &mut Access,
	tenant: &str,
	entry: &Entry,
	deployment: &str,
) -> Result<Option<MemoryEmbedding>> {
	let mut lease = Lease::Inherited(access);
	let config = bindings::configuration(&mut lease, tenant, entry, deployment).await?;
	let Some(provider) = config.memory else {
		return Ok(None);
	};
	let policy = native_memory::policy(&mut lease, tenant, &provider).await?;
	let embedding =
		native_memory::definition(&mut lease, tenant, &policy.embedding, "embedding").await?;
	Ok(Some(MemoryEmbedding {
		provider,
		embedding: serde_json::from_value(embedding.config)?,
	}))
}
