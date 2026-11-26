//! Conversation writes retain the same access transaction and audit context.
use crate::apps::identity::models::AuthorizationThread;
use crate::apps::workspaces::models::Conversation as ConversationRecord;
use crate::semantic::{native_memory, repositories::bindings, service::Lease};
use crate::{
	authorization::{access::Access, catalog, execution, policy::Resource},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::threads::{ConversationScope, MemoryEmbedding},
};
use aidash_domain::{
	Conversation, NewTask, Task, Thread,
	federation::Delegation,
	registry::{EntityRef, Entry},
	semantic::indexing::IndexingSpec,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::json;
use uuid::Uuid;

pub(crate) struct NativeConversation<'a> {
	pub(crate) federation: &'a Federation,
	pub(crate) access: &'a mut Access,
	thread: Option<Resource>,
	thread_id: Option<Uuid>,
}
impl<'a> NativeConversation<'a> {
	pub(crate) fn new(federation: &'a Federation, access: &'a mut Access) -> Self {
		Self {
			federation,
			access,
			thread: None,
			thread_id: None,
		}
	}
}
#[async_trait]
impl ConversationScope for NativeConversation<'_> {
	async fn create_thread(
		&mut self,
		title: &str,
		node: Option<&aidash_domain::nodes::NodeName>,
		scratch_space_id: Option<Uuid>,
	) -> Result<Thread> {
		self.access
			.create_thread(
				&self.federation.store,
				title,
				node,
				scratch_space_id,
				&self.federation.config.scratch_policy,
			)
			.await
			.map_err(Into::into)
	}
	async fn thread_context(&mut self, thread: Uuid) -> Result<()> {
		let resource = self.access.thread(thread).await?;
		self.access.context = resource.attributes.clone();
		self.thread = Some(resource);
		self.thread_id = Some(thread);
		Ok(())
	}
	async fn require_thread(&mut self, action: &str) -> Result<()> {
		let resource = self
			.thread
			.as_ref()
			.expect("thread context initialized by conversation use case");
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn registry_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		if reference.id == aidash_application::registry::system::configurator::ID
			&& !aidash_domain::identity::tenant_administration::configurator_administrator(
				&self.access.snapshot.bundle,
				&self.access.identity.subject,
				self.federation.config.deployment.as_str(),
			) {
			return Err(aidash_application::Error::Invalid(
				"tenant_configurator_administrator_required".into(),
			));
		}
		catalog::entry(self.access, reference, action)
			.await
			.map_err(Into::into)
	}
	async fn select_scratch_space(
		&mut self,
		thread: &Thread,
		name: &str,
		requested: Option<Uuid>,
	) -> Result<Uuid> {
		let _ = name;
		let (sql, values) = Query::select()
			.column(Alias::new("scratch_space_id"))
			.from(Alias::new("threads"))
			.and_where(Expr::col("id").eq(Expr::value(thread.id)))
			.and_where(Expr::col("tenant").eq(Expr::value(self.access.identity.tenant.clone())))
			.build(PostgresQueryBuilder);
		let row = self
			.access
			.tx
			.fetch_optional(&sql, reinhardt::db::orm::execution::convert_values(values))
			.await
			.map_err(crate::Error::from)?
			.ok_or(aidash_application::Error::Forbidden)?;
		let id: Uuid = row
			.get("scratch_space_id")
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		if requested.is_some_and(|requested| requested != id) {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(id)
	}
	async fn create_conversation(
		&mut self,
		thread_id: Uuid,
		scratch_space_id: Uuid,
		target: &EntityRef,
		target_kind: &str,
	) -> Result<Conversation> {
		if target_kind == "agent"
			&& target.id == aidash_application::registry::system::configurator::ID
		{
			use crate::apps::identity::repositories::approval_preferences;
			let identity = &self.access.identity;
			approval_preferences::lock_owner(
				&mut *self.access.tx,
				&identity.tenant,
				&identity.subject,
			)
			.await?;
			approval_preferences::set_thread_mode(
				&mut *self.access.tx,
				&identity.tenant,
				&identity.subject,
				thread_id,
				Some(aidash_domain::approval_rules::ApprovalMode::Bypass),
			)
			.await?;
		}
		let conversation: ConversationRecord = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = thread_id;
			let query_bind_3 = format!("{}@{}", target.id, target.version);
			let query_bind_4 = target_kind;
			let query_bind_5 = &self.access.identity.subject;
			aidash_server::database::query_as(
				&Query::insert()
					.into_table(Alias::new("conversations"))
					.columns([
						Alias::new("id"),
						Alias::new("thread_id"),
						Alias::new("scratch_space_id"),
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
							.expr(Expr::value(scratch_space_id))
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
								"threads",
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
				Some(conversation.thread_id),
				"conversation.created",
				json!(conversation),
			)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn message(&mut self, thread: Uuid, content: &str) -> Result<()> {
		self.federation
			.store
			.message_in(
				&mut self.access.tx,
				thread,
				&self.access.identity.subject,
				content,
				None,
			)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn create_task(&mut self, thread: Uuid, input: &NewTask) -> Result<Task> {
		self.federation
			.store
			.create_task_in(
				&mut self.access.tx,
				thread,
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
	/// The same resolution a local claim makes before it pins the memory bank.
	async fn memory_embedding(&mut self, agent: &EntityRef) -> Result<Option<MemoryEmbedding>> {
		let thread = self
			.thread_id
			.expect("thread context initialized by conversation use case");
		let entry = catalog::entry(self.access, agent, "registry.read").await?;
		let tenant = AuthorizationThread::tenant_in(&mut **self.access.tx, thread)
			.await?
			.ok_or(aidash_application::Error::Forbidden)?;
		// The agent's dependencies are used by the agent, so resolve them with
		// the subject chain the claim builds: the executing agent appended
		// after whoever starts the conversation. Restore the chain and the
		// approvals cached under it before any check made for the person.
		let node = self.access.thread_node(thread).await?;
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
		let thread = conversation.thread_id;
		let revision = crate::semantic::repositories::mutations::create_first_index(
			&self.federation.store,
			&mut **self.access.tx,
			thread,
			spec,
			"new immutable index generation for the conversation target's memory",
		)
		.await?;
		self.federation
			.store
			.event(
				&mut self.access.tx,
				Some(thread),
				"conversation.index_created",
				// `id` names the conversation, whose readers may see this event.
				json!({
					"id": conversation.id,
					"thread_id": thread,
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
	let Some(role) = &policy.embedding else {
		return Ok(None);
	};
	let embedding = native_memory::definition(&mut lease, tenant, role, "embedding").await?;
	Ok(Some(MemoryEmbedding {
		provider,
		embedding: serde_json::from_value(embedding.config)?,
	}))
}
