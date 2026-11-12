//! Journal queries retain their original order, strict Run decoding, and caller transaction.
use super::{NativeReads, Reads};
use crate::Result as NativeResult;
use crate::apps::execution::generation::repositories::contracts::RequestAccess;
use aidash_application::{
	Result,
	ports::authorization::visibility::{
		provenance::{LocalOutputScope, OutputScope, ReadProvenanceScope, RecordedRead},
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
#[async_trait]
impl ReadProvenanceScope for Reads<'_> {
	async fn output_producers(
		&mut self,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<Option<Vec<Uuid>>> {
		for grant in self.remote_grants(workspace, kind, id).await? {
			if !self.grant_visible(grant).await? {
				return Ok(None);
			}
		}
		Ok(Some(self.producers(workspace, kind, id).await?))
	}
	async fn open_run_reads(&mut self, run: Uuid) -> Result<bool> {
		self.access.open_run_reads(run).await.map_err(Into::into)
	}
	fn close_run_reads(&mut self) {
		self.access.close_run_reads();
	}
	/// A recorded definition read is checked by `registry.read` alone: a Catalog Approval
	/// decides use, not who reads what a Run produced (#769).
	async fn catalog_read(&mut self, _: Uuid, reference: &EntityRef) -> Result<()> {
		let node = self.access.node.clone();
		if self
			.access
			.read_lookups
			.catalog_read_passed(&node, reference, false)
		{
			return Ok(());
		}
		crate::apps::identity::services::catalog::read_recorded(self.access, reference)
			.await
			.map_err(aidash_application::Error::from)?;
		self.access
			.read_lookups
			.remember_catalog_read(&node, reference, false);
		Ok(())
	}
	async fn remote_reads(
		&mut self,
		run: Uuid,
	) -> Result<aidash_application::ports::federation::registry_reads::RemoteReads> {
		self.access.remote_reads(run).await.map_err(Into::into)
	}
	async fn semantic_reads(&mut self, run: Uuid) -> Result<bool> {
		self.access
			.semantic_reads_visible(run)
			.await
			.map_err(Into::into)
	}
	async fn semantic_invalidity(
		&mut self,
		run: Uuid,
		pending: &mut Vec<Uuid>,
	) -> Result<Option<aidash_domain::semantic::Failure>> {
		self.access
			.semantic_reads_invalidity(run, pending)
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
		if let Some(artifact) = self.access.read_lookups.artifact(id, workspace) {
			return Ok(artifact);
		}
		self.artifact(id, Some(workspace)).await
	}
	fn first_visit(&mut self, run: Uuid) -> bool {
		self.access.read_lookups.first_visit(run, false)
	}
	async fn prepare_sources(&mut self, sources: &[RecordedRead]) -> Result<()> {
		self.access
			.prepare_read_sources(sources)
			.await
			.map_err(Into::into)
	}
	async fn run_base_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		self.access.run_base_visible(run).await.map_err(Into::into)
	}
	async fn run_dependency_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		if self.access.read_lookups.dependency_passed(run.id, false) {
			return Ok(true);
		}
		let visible = if !run.is_local(&self.access.node_id)
			|| crate::apps::knowledge::repositories::memory_reads::has_reads(
				&mut **self.access.tx,
				run.id,
			)
			.await?
		{
			self.run_base_visible(run).await?
		} else {
			aidash_application::authorization::visibility::local_dependency_without_memory_visible(
				&mut crate::bootstrap::run_visibility_scope(self.access),
				run,
			)
			.await?
		};
		if visible {
			self.access.read_lookups.remember_dependency(run.id, false);
		}
		Ok(visible)
	}
	async fn classified_run_dependency_visible(
		&mut self,
		run: &RunMetadata,
		pending: &mut Vec<Uuid>,
	) -> Result<bool> {
		if !run.is_local(&self.access.node_id) {
			return aidash_application::authorization::visibility::classified_base_visible(
				self, run, pending,
			)
			.await;
		}
		let has_reads = crate::apps::knowledge::repositories::memory_reads::has_reads(
			&mut **self.access.tx,
			run.id,
		)
		.await?;
		aidash_application::authorization::visibility::classified_local_dependency_visible(
			self, run, pending, has_reads,
		)
		.await
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
		if let Some(task) = self.access.read_lookups.task(id, workspace) {
			return Ok(task);
		}
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
		if let Some(message) = self.access.read_lookups.message(id, workspace) {
			return Ok(message);
		}
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
	async fn source_attachment_message(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Result<Option<Message>> {
		if let Some(message) = self.access.read_lookups.attachment(id, workspace) {
			return match message {
				Some(id) => self.source_message(id, workspace).await,
				None => Ok(None),
			};
		}
		let message_id: Option<Uuid> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("message_id"))
				.from(Alias::new("channel_attachments"))
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
				.and_where(Expr::col("message_id").is_not_null())
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **self.access.tx)
		.await?;
		match message_id {
			Some(id) => self.source_message(id, workspace).await,
			None => Ok(None),
		}
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
		if let Some(conversation) = self.access.read_lookups.conversation(id, workspace) {
			return Ok(conversation);
		}
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
	async fn output_producers(
		&mut self,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<Option<Vec<Uuid>>> {
		Ok(Some(self.producers(workspace, kind, id).await?))
	}
	async fn registry_entries(&mut self, run: Uuid) -> Result<Vec<EntityRef>> {
		crate::apps::identity::models::AuthorizationRunRegistryRead::for_run(
			self.access.tx.as_mut(),
			run,
		)
		.await
		.map_err(Into::into)
	}
	async fn open_run_reads(&mut self, run: Uuid) -> Result<bool> {
		self.access.open_run_reads(run).await.map_err(Into::into)
	}
	fn close_run_reads(&mut self) {
		self.access.close_run_reads();
	}
	async fn catalog_read(&mut self, _: Uuid, reference: &EntityRef) -> Result<()> {
		let node = self.access.node.clone();
		if self
			.access
			.read_lookups
			.catalog_read_passed(&node, reference, true)
		{
			return Ok(());
		}
		crate::apps::identity::services::catalog::read_recorded(self.access, reference)
			.await
			.map_err(aidash_application::Error::from)?;
		self.access
			.read_lookups
			.remember_catalog_read(&node, reference, true);
		Ok(())
	}
	async fn remote_reads(
		&mut self,
		run: Uuid,
	) -> Result<aidash_application::ports::federation::registry_reads::RemoteReads> {
		self.access.remote_reads(run).await.map_err(Into::into)
	}
	async fn semantic_reads(&mut self, run: Uuid) -> Result<bool> {
		self.access
			.semantic_reads_visible(run)
			.await
			.map_err(Into::into)
	}
	async fn semantic_invalidity(
		&mut self,
		run: Uuid,
		pending: &mut Vec<Uuid>,
	) -> Result<Option<aidash_domain::semantic::Failure>> {
		self.access
			.semantic_reads_invalidity(run, pending)
			.await
			.map_err(Into::into)
	}
	async fn sources(&mut self, run: Uuid) -> Result<Vec<RecordedRead>> {
		crate::apps::identity::models::AuthorizationRunRead::for_run(self.access.tx.as_mut(), run)
			.await
			.map_err(Into::into)
	}
	fn first_visit(&mut self, run: Uuid) -> bool {
		self.access.read_lookups.first_visit(run, true)
	}
	async fn prepare_sources(&mut self, sources: &[RecordedRead]) -> Result<()> {
		self.access
			.prepare_read_sources(sources)
			.await
			.map_err(Into::into)
	}
	async fn source_task(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Task>> {
		if let Some(task) = self.access.read_lookups.task(id, workspace) {
			return Ok(task);
		}
		crate::apps::workspaces::models::Task::read_in(self.access.tx.as_mut(), id, workspace)
			.await
			.map_err(Into::into)
	}
	async fn source_artifact(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Artifact>> {
		if let Some(artifact) = self.access.read_lookups.artifact(id, workspace) {
			return Ok(artifact);
		}
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
		if let Some(message) = self.access.read_lookups.message(id, workspace) {
			return Ok(message);
		}
		crate::apps::workspaces::models::Message::read_in(
			self.access.tx.as_mut(),
			id,
			workspace,
			false,
		)
		.await
		.map_err(Into::into)
	}
	async fn source_attachment_message(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Result<Option<Message>> {
		if let Some(message) = self.access.read_lookups.attachment(id, workspace) {
			return match message {
				Some(id) => self.source_message(id, workspace).await,
				None => Ok(None),
			};
		}
		let message_id: Option<Uuid> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("message_id"))
				.from(Alias::new("channel_attachments"))
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
				.and_where(Expr::col("message_id").is_not_null())
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **self.access.tx)
		.await?;
		match message_id {
			Some(id) => self.source_message(id, workspace).await,
			None => Ok(None),
		}
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
		if let Some(conversation) = self.access.read_lookups.conversation(id, workspace) {
			return Ok(conversation);
		}
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
	async fn run_dependency_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		if self.access.read_lookups.dependency_passed(run.id, true) {
			return Ok(true);
		}
		let visible = if !run.is_local(&self.access.node_id)
			|| crate::apps::knowledge::repositories::memory_reads::has_reads(
				&mut **self.access.tx,
				run.id,
			)
			.await?
		{
			self.run_base_visible(run).await?
		} else {
			aidash_application::authorization::visibility::local_dependency_without_memory_visible(
				&mut crate::bootstrap::native_run_visibility_scope(self.access),
				run,
			)
			.await?
		};
		if visible {
			self.access.read_lookups.remember_dependency(run.id, true);
		}
		Ok(visible)
	}
	async fn classified_run_dependency_visible(
		&mut self,
		run: &RunMetadata,
		pending: &mut Vec<Uuid>,
	) -> Result<bool> {
		let require_memory = !run.is_local(&self.access.node_id)
			|| crate::apps::knowledge::repositories::memory_reads::has_reads(
				&mut **self.access.tx,
				run.id,
			)
			.await?;
		aidash_application::authorization::visibility::classified_local_dependency_visible(
			self,
			run,
			pending,
			require_memory,
		)
		.await
	}
	async fn generation_visible(&mut self, job: &Request) -> Result<bool> {
		aidash_application::generation::visibility::local_visible(
			&mut crate::bootstrap::native_generation_visibility_scope(self.access),
			job,
		)
		.await
	}
}
