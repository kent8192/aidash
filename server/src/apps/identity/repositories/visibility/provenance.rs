//! Journal queries retain their original order, strict Run decoding, and caller transaction.
use super::{NativeReads, Reads};
use crate::Result as NativeResult;
use crate::apps::execution::generation::repositories::contracts::RequestAccess;
use aidash_application::{
	Result,
	ports::authorization::visibility::{
		provenance::{ReadProvenanceScope, RecordedRead},
		resources::ResourceEventScope,
	},
};
use aidash_domain::{
	Artifact, Conversation, Message, RunMetadata, Task, generation::requests::Request,
	registry::EntityRef,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use uuid::Uuid;
impl Reads<'_> {
	/// Whether `run` pins `reference` only for the delegated agents of its
	/// Binding closure and the restriction admits those agents (ADR 0095).
	///
	/// Such a definition is a recorded read of the Run as immutable metadata,
	/// but its Catalog Approval restriction names the agents that use it
	/// directly, not the executing agent that ends the subject chain.
	///
	/// Only the Run's own worker authority may substitute those agents: any
	/// other reader of the Run's outputs is still checked as the direct user.
	async fn closure_read(&mut self, run: Uuid, reference: &EntityRef) -> NativeResult<bool> {
		if self.access.read_run != Some(run) {
			return Ok(false);
		}
		let snapshot: Option<serde_json::Value> = crate::database::native::query_scalar(
			&Query::select()
				.expr(Expr::cust("context -> 'binding_snapshot'"))
				.from(Alias::new("runs"))
				.and_where(Expr::col("id").eq(Expr::value(run)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **self.access.tx)
		.await?;
		let Some(snapshot) = snapshot.filter(|snapshot| !snapshot.is_null()) else {
			return Ok(false);
		};
		let snapshot: aidash_domain::registry::bindings::BindingSnapshot =
			serde_json::from_value(snapshot)?;
		let delegated = snapshot.delegated_users()?;
		let Some(users) = delegated.iter().find_map(|(identity, users)| {
			(identity.registry_deployment == self.access.node_id
				&& identity.id == reference.id
				&& identity.version == reference.version)
				.then_some(users)
		}) else {
			return Ok(false);
		};
		let users = users.iter().map(|user| user.local()).collect::<Vec<_>>();
		match crate::apps::identity::services::catalog::closure_entry(
			self.access,
			reference,
			"registry.read",
			&users,
		)
		.await
		{
			Ok(_) => Ok(true),
			Err(crate::Error::Forbidden) => Ok(false),
			Err(error) => Err(error),
		}
	}
}

#[async_trait]
impl ReadProvenanceScope for Reads<'_> {
	async fn catalog_read(&mut self, run: Uuid, reference: &EntityRef) -> Result<()> {
		match crate::apps::identity::services::catalog::entry(
			self.access,
			reference,
			"registry.read",
		)
		.await
		{
			Ok(_) => Ok(()),
			Err(crate::Error::Forbidden)
				if self
					.closure_read(run, reference)
					.await
					.map_err(aidash_application::Error::from)? =>
			{
				Ok(())
			}
			Err(error @ (crate::Error::Forbidden | crate::Error::NotFound(_))) => {
				if crate::apps::identity::repositories::foreign_reads::retired_run_catalog_read(
					self.access,
					run,
					reference,
				)
				.await?
				{
					Ok(())
				} else {
					Err(error.into())
				}
			}
			Err(error) => Err(error.into()),
		}
	}
	async fn remote_reads(&mut self, run: Uuid) -> Result<bool> {
		self.access
			.remote_reads_visible(run)
			.await
			.map_err(Into::into)
	}
	async fn semantic_reads(&mut self, run: Uuid) -> Result<bool> {
		self.access
			.semantic_reads_visible(run)
			.await
			.map_err(Into::into)
	}
	async fn received_semantic(&mut self, run: Uuid) -> Result<bool> {
		self.access
			.received_semantic_visible(run)
			.await
			.map_err(Into::into)
	}
	async fn source_artifact(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Artifact>> {
		self.artifact(id, Some(workspace)).await
	}
	async fn run_base_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		self.access.run_base_visible(run).await.map_err(Into::into)
	}
	async fn generation_visible(&mut self, job: &Request) -> Result<bool> {
		job.visible(self.access).await.map_err(Into::into)
	}
	async fn registry_entries(&mut self, run: Uuid) -> Result<Vec<EntityRef>> {
		let result: NativeResult<Vec<EntityRef>> = async {
			let this = &mut *self.access;
			let mut entries: Vec<(String, String)> = {
				let query_bind_1 = run;
				crate::database::native::query_as(
					&Query::select()
						.columns([Alias::new("entry_id"), Alias::new("entry_version")])
						.from(Alias::new("authorization_run_registry_reads"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("run_id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.order_by(Alias::new("entry_id"), Order::Asc)
						.order_by(Alias::new("entry_version"), Order::Asc)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["entry_id", "entry_version"])
				.fetch_all(&mut **this.tx)
				.await?
			};
			let snapshot: Option<serde_json::Value> = crate::database::native::query_scalar(
				&Query::select()
					.expr(Expr::cust("context -> 'binding_snapshot'"))
					.from(Alias::new("runs"))
					.and_where(Expr::col("id").eq(Expr::value(run)))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **this.tx)
			.await?;
			if let Some(snapshot) = snapshot.filter(|snapshot| !snapshot.is_null()) {
				let snapshot: aidash_domain::registry::bindings::BindingSnapshot =
					serde_json::from_value(snapshot)?;
				for definition in snapshot.definitions {
					if definition.identity.registry_deployment != this.node_id {
						continue;
					}
					if aidash_domain::registry::bindings::sources::is_selection_decider(
						&definition.definition,
					) || aidash_domain::registry::bindings::defaults::is_compaction_decider(
						&definition.definition,
					) {
						// Dormant metadata remains a live policy dependency, without
						// manufacturing a completed approval-gated Registry read.
						let current = crate::apps::identity::services::catalog::metadata(
							this,
							&definition.identity.local(),
						)
						.await?;
						if aidash_domain::registry::rules::digest(&serde_json::to_value(current)?)
							!= definition.digest
						{
							return Err(crate::Error::Conflict(
								"admitted definition changed".into(),
							));
						}
					} else {
						entries.push((definition.identity.id, definition.identity.version));
					}
				}
			}
			entries.sort();
			entries.dedup();
			Ok(entries
				.into_iter()
				.map(|(id, version)| EntityRef { id, version })
				.collect())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn sources(&mut self, run: Uuid) -> Result<Vec<RecordedRead>> {
		let result: NativeResult<Vec<RecordedRead>> = async {
			let this = &mut *self.access;
			let sources: Vec<(Uuid, String, Uuid)> = {
				let query_bind_1 = run;
				crate::database::native::query_as(
					&Query::select()
						.column(Alias::new("workspace_id"))
						.column(Alias::new("resource_kind"))
						.column(Alias::new("resource_id"))
						.from(Alias::new("authorization_run_reads"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("run_id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.order_by(Alias::new("resource_kind"), Order::Asc)
						.order_by(Alias::new("resource_id"), Order::Asc)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["workspace_id", "resource_kind", "resource_id"])
				.fetch_all(&mut **this.tx)
				.await?
			};
			Ok(sources)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn source_task(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Task>> {
		let result: NativeResult<Option<Task>> = async {
			let this = &mut *self.access;
			let source: Option<Task> = {
				let query_bind_1 = id;
				let query_bind_2 = workspace;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("tasks"))
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
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **this.tx)
				.await?
			};
			Ok(source)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn source_message(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Message>> {
		let result: NativeResult<Option<Message>> = async {
			let this = &mut *self.access;
			let source: Option<Message> = {
				let query_bind_1 = id;
				let query_bind_2 = workspace;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("messages"))
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
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **this.tx)
				.await?
			};
			Ok(source)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn source_run(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<RunMetadata>> {
		let result: NativeResult<Option<RunMetadata>> = async {
			let this = &mut *self.access;
			// Only the metadata is decoded: a Run's state grows with every step,
			// and provenance checks reach the same Runs many times (#404).
			let source: Option<crate::domain::RunMetadata> = {
				let query_bind_1 = id;
				let query_bind_2 = workspace;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("runs"))
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
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **this.tx)
				.await?
			};
			Ok(source)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn source_conversation(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Result<Option<Conversation>> {
		let result: NativeResult<Option<Conversation>> = async {
			let this = &mut *self.access;
			let source: Option<crate::domain::Conversation> = {
				let query_bind_1 = id;
				let query_bind_2 = workspace;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("conversations"))
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
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **this.tx)
				.await?
			};
			Ok(source)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn source_generation(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Request>> {
		let result: NativeResult<Option<Request>> = async {
			let this = &mut *self.access;
			let source: Option<crate::generation::Request> = {
				let query_bind_1 = id;
				let query_bind_2 = workspace;
				crate::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("generation_requests"))
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
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **this.tx)
				.await?
			};
			Ok(source)
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl ReadProvenanceScope for NativeReads<'_> {
	async fn registry_entries(&mut self, run: Uuid) -> Result<Vec<EntityRef>> {
		crate::apps::identity::models::AuthorizationRunRegistryRead::for_run(
			self.access.tx.as_mut(),
			run,
		)
		.await
		.map_err(Into::into)
	}
	async fn catalog_read(&mut self, _: Uuid, reference: &EntityRef) -> Result<()> {
		self.access
			.catalog_entry(reference)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn remote_reads(&mut self, run: Uuid) -> Result<bool> {
		self.access
			.remote_reads_visible(run)
			.await
			.map_err(Into::into)
	}
	async fn semantic_reads(&mut self, run: Uuid) -> Result<bool> {
		self.access
			.semantic_reads_visible(run)
			.await
			.map_err(Into::into)
	}
	async fn sources(&mut self, run: Uuid) -> Result<Vec<RecordedRead>> {
		crate::apps::identity::models::AuthorizationRunRead::for_run(self.access.tx.as_mut(), run)
			.await
			.map_err(Into::into)
	}
	async fn source_task(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Task>> {
		crate::apps::workspaces::models::Task::read_in(self.access.tx.as_mut(), id, workspace)
			.await
			.map_err(Into::into)
	}
	async fn source_artifact(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Artifact>> {
		crate::apps::workspaces::models::Artifact::read_in(
			self.access.tx.as_mut(),
			id,
			workspace,
			false,
		)
		.await
		.map_err(Into::into)
	}
	async fn source_message(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Message>> {
		crate::apps::workspaces::models::Message::read_in(
			self.access.tx.as_mut(),
			id,
			workspace,
			false,
		)
		.await
		.map_err(Into::into)
	}
	async fn source_run(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<RunMetadata>> {
		crate::apps::execution::models::Run::read_in(self.access.tx.as_mut(), id, Some(workspace))
			.await
			.map(|run| run.map(|run| run.metadata()))
			.map_err(Into::into)
	}
	async fn source_conversation(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Result<Option<Conversation>> {
		crate::apps::workspaces::models::Conversation::read_in(
			self.access.tx.as_mut(),
			id,
			workspace,
		)
		.await
		.map_err(Into::into)
	}
	async fn source_generation(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Request>> {
		crate::apps::execution::generation::models::GenerationRequest::read_in(
			self.access.tx.as_mut(),
			id,
			workspace,
		)
		.await
		.map_err(Into::into)
	}
	async fn run_base_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		self.access.run_base_visible(run).await.map_err(Into::into)
	}
	async fn generation_visible(&mut self, job: &Request) -> Result<bool> {
		aidash_application::generation::visibility::local_visible(
			&mut crate::bootstrap::native_generation_visibility_scope(self.access),
			job,
		)
		.await
	}
}
