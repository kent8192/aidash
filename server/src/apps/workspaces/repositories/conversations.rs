//! Conversation writes retain the same access transaction and audit context.
use crate::apps::workspaces::models::Conversation as ConversationRecord;
use crate::{
	authorization::{access::Access, catalog, execution, policy::Resource},
	federation::Federation,
};
use aidash_application::{Result, ports::workspaces::ConversationScope};
use aidash_domain::{
	Conversation, NewTask, Task, Workspace,
	federation::Delegation,
	registry::{EntityRef, Entry},
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
}
impl<'a> NativeConversation<'a> {
	pub(crate) fn new(federation: &'a Federation, access: &'a mut Access) -> Self {
		Self {
			federation,
			access,
			workspace: None,
		}
	}
}
#[async_trait]
impl ConversationScope for NativeConversation<'_> {
	async fn create_workspace(&mut self, title: &str, goal: &str) -> Result<Workspace> {
		self.access
			.create_workspace(&self.federation.store, title, goal)
			.await
			.map_err(Into::into)
	}
	async fn workspace_context(&mut self, workspace: Uuid) -> Result<()> {
		let resource = self.access.workspace(workspace).await?;
		self.access.context = resource.attributes.clone();
		self.workspace = Some(resource);
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
							.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **self.access.tx)
			.await
			.map_err(crate::Error::from)?
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
			.await
			.map_err(crate::Error::from)?)
	}
}

use aidash_application::ports::workspaces::{
	OperatorConversationTransaction, OperatorConversations,
};
use reinhardt::db::backends::{TransactionExecutor, dialect::postgres::PgTransactionExecutor};

pub(crate) struct NativeOperatorConversations(pub(crate) Federation);
#[async_trait]
impl OperatorConversations for NativeOperatorConversations {
	async fn definition(&self, reference: &EntityRef) -> Result<Entry> {
		self.0
			.registry
			.get(&reference.id, &reference.version)
			.await
			.map_err(Into::into)
	}
	async fn require_legacy_agent(&self, reference: &EntityRef) -> Result<()> {
		self.0
			.store
			.require_legacy_agent(&reference.id, &reference.version)
			.await
			.map_err(Into::into)
	}
	async fn begin(&self) -> Result<Box<dyn OperatorConversationTransaction>> {
		let tx = self
			.0
			.store
			.pool
			.begin()
			.await
			.map_err(crate::Error::from)?;
		Ok(Box::new(NativeOperatorTransaction {
			federation: self.0.clone(),
			phase: OperatorPhase::Admission(tx),
		}))
	}
	async fn deliver(&self, delegation: &Delegation) -> Result<()> {
		self.0.deliver(delegation).await.map_err(Into::into)
	}
}
// Both phases own the same physical transaction. Dropping either rolls it back.
enum OperatorPhase {
	Admission(sqlx::Transaction<'static, sqlx::Postgres>),
	Execution(Box<dyn TransactionExecutor>),
	Consumed,
}
struct NativeOperatorTransaction {
	federation: Federation,
	phase: OperatorPhase,
}
impl NativeOperatorTransaction {
	fn admission(&mut self) -> Result<&mut sqlx::Transaction<'static, sqlx::Postgres>> {
		match &mut self.phase {
			OperatorPhase::Admission(tx) => Ok(tx),
			_ => Err(
				crate::Error::Conflict("conversation transaction has left admission".into()).into(),
			),
		}
	}
}
#[async_trait]
impl OperatorConversationTransaction for NativeOperatorTransaction {
	async fn create_workspace(&mut self, title: &str, goal: &str) -> Result<Workspace> {
		let store = self.federation.store.clone();
		store
			.create_workspace_in(self.admission()?, Uuid::new_v4(), title, goal)
			.await
			.map_err(Into::into)
	}
	async fn create_conversation(
		&mut self,
		workspace_id: Uuid,
		target: &EntityRef,
		target_kind: &str,
	) -> Result<Conversation> {
		let tx = self.admission()?;
		let conversation: ConversationRecord = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = workspace_id;
			let query_bind_3 = format!("{}@{}", target.id, target.version);
			let query_bind_4 = target_kind;
			aidash_server::database::query_as(
				&Query::insert()
					.into_table(Alias::new("conversations"))
					.columns([
						Alias::new("id"),
						Alias::new("workspace_id"),
						Alias::new("target"),
						Alias::new("target_kind"),
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
							.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await
			.map_err(crate::Error::from)?
		};
		Ok(conversation.into())
	}
	async fn conversation_event(&mut self, conversation: &Conversation) -> Result<()> {
		let store = self.federation.store.clone();
		store
			.event(
				self.admission()?,
				Some(conversation.workspace_id),
				"conversation.created",
				json!(conversation),
			)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn message(&mut self, workspace: Uuid, content: &str) -> Result<()> {
		let store = self.federation.store.clone();
		store
			.message_in(self.admission()?, workspace, "human", content, None)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn create_task(&mut self, workspace: Uuid, input: &NewTask) -> Result<Task> {
		let store = self.federation.store.clone();
		store
			.create_task_in(self.admission()?, workspace, input, "human", None)
			.await
			.map_err(Into::into)
	}
	async fn delegate(&mut self, task: &Task, agent: &EntityRef) -> Result<(Task, Delegation)> {
		let phase = std::mem::replace(&mut self.phase, OperatorPhase::Consumed);
		let tx = match phase {
			OperatorPhase::Admission(tx) => tx,
			other => {
				self.phase = other;
				return Err(crate::Error::Conflict(
					"conversation transaction has left admission".into(),
				)
				.into());
			}
		};
		let mut native = PgTransactionExecutor::new(tx);
		let result = self
			.federation
			.delegate_in(&mut native, task, &self.federation.config.node_id, agent)
			.await?;
		self.phase = OperatorPhase::Execution(Box::new(native));
		Ok(result)
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		match std::mem::replace(&mut self.phase, OperatorPhase::Consumed) {
			OperatorPhase::Execution(tx) => tx
				.commit()
				.await
				.map_err(crate::Error::from)
				.map_err(Into::into),
			_ => Err(
				crate::Error::Conflict("conversation delegation has not completed".into()).into(),
			),
		}
	}
}
