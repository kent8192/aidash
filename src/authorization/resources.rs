//! Resource attributes come from stored rows and their authoritative workspace.
use super::{access::Access, policy::Resource};
use crate::{
	Error, Result,
	domain::{Artifact, Event, Message, Task},
};
use sea_orm::sea_query::{
	Alias, Asterisk, Condition, Expr, OnConflict, Order, PostgresQueryBuilder, Query,
};
use serde_json::json;
use uuid::Uuid;

impl Access {
	pub(crate) async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		let workspace = self.workspace(task.workspace_id).await?;
		let mut attributes = workspace.attributes;
		attributes["created_by"] = json!(task.created_by);
		attributes["task_id"] = json!(task.id);
		Ok(self.resource("task", task.id, attributes))
	}
	pub(crate) async fn task_read(&mut self, id: Uuid) -> Result<Task> {
		let task: Task = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("tasks"))
				.cond_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_optional(&mut **self.tx)
		.await?
		.ok_or(Error::Forbidden)?;
		let resource = self.task_resource(&task).await?;
		self.require(&resource, "task.read").await?;
		if !self
			.output_visible(task.workspace_id, "task", task.id)
			.await?
		{
			return Err(Error::Forbidden);
		}
		Ok(task)
	}
	pub(crate) async fn task_visible(&mut self, task: &Task) -> Result<bool> {
		let resource = self.task_resource(task).await?;
		Ok(self.decide(&resource, "task.read").await?
			&& self
				.output_visible(task.workspace_id, "task", task.id)
				.await?)
	}
	pub(crate) async fn task_summary_visible(
		&mut self,
		workspace: &Resource,
		workspace_id: Uuid,
		task_id: Uuid,
		created_by: &str,
	) -> Result<bool> {
		let mut attributes = workspace.attributes.clone();
		attributes["created_by"] = json!(created_by);
		attributes["task_id"] = json!(task_id);
		let resource = self.resource("task", task_id, attributes);
		Ok(self.decide(&resource, "task.read").await?
			&& self.output_visible(workspace_id, "task", task_id).await?)
	}
	pub(crate) async fn track_task_reads(&mut self, workspace: Uuid, tasks: &[Uuid]) -> Result<()> {
		if tasks.is_empty() {
			return Ok(());
		}
		let (scope, table, column) = match (self.read_run, self.read_grant) {
			(Some(run), None) => (run, "authorization_run_reads", "run_id"),
			(None, Some(grant)) => (grant, "authorization_remote_grant_reads", "grant_id"),
			(None, None) => return Ok(()),
			(Some(_), Some(_)) => return Err(Error::Forbidden),
		};
		let kinds = vec!["task".to_owned(); tasks.len()];
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new(table))
				.columns([
					Alias::new(column),
					Alias::new("workspace_id"),
					Alias::new("resource_kind"),
					Alias::new("resource_id"),
				])
				.select_from(
					Query::select()
						.exprs([
							Expr::cust("$1"),
							Expr::cust("$2"),
							Expr::cust("unnest($3::text[])"),
							Expr::cust("unnest($4::uuid[])"),
						])
						.to_owned(),
				)
				.map_err(|error| Error::Invalid(error.to_string()))?
				.on_conflict(OnConflict::new().do_nothing().to_owned())
				.to_string(PostgresQueryBuilder),
		)
		.bind(scope)
		.bind(workspace)
		.bind(kinds)
		.bind(tasks)
		.execute(&self.pool)
		.await?;
		Ok(())
	}
	pub(crate) async fn related_tasks(
		&mut self,
		workspace: Uuid,
		input: &crate::domain::NewTask,
	) -> Result<()> {
		for id in input.dependencies.iter().chain(input.parent_id.iter()) {
			if self.task_read(*id).await?.workspace_id != workspace {
				return Err(Error::Forbidden);
			}
		}
		Ok(())
	}
	pub(crate) async fn artifact_resource(&mut self, artifact: &Artifact) -> Result<Resource> {
		let workspace = self.workspace(artifact.workspace_id).await?;
		let mut attributes = workspace.attributes;
		attributes["created_by"] = json!(artifact.created_by);
		attributes["task_id"] = json!(artifact.task_id);
		attributes["kind"] = json!(artifact.kind);
		Ok(self.resource("artifact", artifact.id, attributes))
	}
	pub(crate) async fn artifact_visible(&mut self, artifact: &Artifact) -> Result<bool> {
		let resource = self.artifact_resource(artifact).await?;
		if !self.decide(&resource, "artifact.read").await? {
			return Ok(false);
		}
		let task: Option<Task> = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("tasks"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(artifact.task_id)
		.bind(artifact.workspace_id)
		.fetch_optional(&mut **self.tx)
		.await?;
		match task {
			Some(task) if self.task_visible(&task).await? => {}
			_ => return Ok(false),
		}
		self.output_visible(artifact.workspace_id, "artifact", artifact.id)
			.await
	}
	async fn output_visible(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<bool> {
		let grants: Vec<Uuid> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("grant_id"))
				.from(Alias::new("authorization_remote_outputs"))
				.and_where(Expr::cust(
					"workspace_id=$1 AND resource_kind=$2 AND resource_id=$3",
				))
				.order_by(Alias::new("grant_id"), Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.bind(workspace)
		.bind(kind)
		.bind(id)
		.fetch_all(&mut **self.tx)
		.await?;
		for grant in grants {
			let key = (grant, format!("remote:{}", self.authority_context()));
			if self.checking_reads.insert(key.clone()) {
				let allowed = Box::pin(self.grant_reads_visible(grant)).await;
				self.checking_reads.remove(&key);
				if !allowed? {
					return Ok(false);
				}
			}
		}
		let producers: Vec<Uuid> = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("run_id")),
				))
				.from(sea_orm::sea_query::Alias::new("authorization_run_outputs"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"workspace_id = $1 AND resource_kind = $2 AND resource_id = $3",
				))
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("run_id"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(workspace)
		.bind(kind)
		.bind(id)
		.fetch_all(&mut **self.tx)
		.await?;
		for producer in producers {
			if !Box::pin(self.run_reads_visible(producer)).await? {
				return Ok(false);
			}
		}
		Ok(true)
	}
	pub(crate) async fn memory_resource(&mut self, run: &crate::domain::Run) -> Result<Resource> {
		let workspace = self.workspace(run.workspace_id).await?;
		let mut attributes = workspace.attributes;
		attributes["created_by"] = json!(crate::domain::qualified_agent(
			&run.home_node,
			&run.agent_id,
			&run.agent_version
		));
		attributes["version"] = json!(run.agent_version);
		Ok(self.resource("memory", &run.agent_id, attributes))
	}
	pub(crate) async fn artifact_creation_resource(
		&mut self,
		task: Uuid,
		creator: &str,
	) -> Result<Resource> {
		let task = self.task_read(task).await?;
		let mut resource = self.task_resource(&task).await?;
		resource.kind = "artifact".into();
		resource.attributes["created_by"] = json!(creator);
		Ok(resource)
	}
	pub(crate) async fn message_resource(&mut self, message: &Message) -> Result<Resource> {
		let workspace = self.workspace(message.workspace_id).await?;
		let mut attributes = workspace.attributes;
		attributes["created_by"] = json!(message.sender);
		attributes["sender"] = json!(message.sender);
		Ok(self.resource("message", message.id, attributes))
	}
	pub(crate) async fn message_visible(&mut self, message: &Message) -> Result<bool> {
		let resource = self.message_resource(message).await?;
		Ok(self.decide(&resource, "message.read").await?
			&& self
				.output_visible(message.workspace_id, "message", message.id)
				.await?)
	}
	pub(crate) async fn resource_event_visible(&mut self, event: &Event) -> Result<Option<bool>> {
		let id = |value: &serde_json::Value| value.as_str().and_then(|s| s.parse::<Uuid>().ok());
		if event.kind.starts_with("task.") {
			let task_id = id(&event.data["task"]["id"])
				.or_else(|| id(&event.data["task_id"]))
				.or_else(|| id(&event.data["id"]));
			let Some(task_id) = task_id else {
				return Ok(Some(false));
			};
			let task: Option<Task> = sqlx::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("tasks"))
					.cond_where(
						Condition::all()
							.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
							.add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
					)
					.to_string(PostgresQueryBuilder),
			)
			.bind(task_id)
			.bind(event.workspace_id)
			.fetch_optional(&mut **self.tx)
			.await?;
			let Some(task) = task else {
				return Ok(Some(false));
			};
			if !self.task_visible(&task).await? {
				return Ok(Some(false));
			}
			if let Some(artifact_id) = id(&event.data["artifact"]["id"]) {
				return Ok(Some(
					self.artifact_id_visible(artifact_id, event.workspace_id)
						.await?,
				));
			}
			return Ok(Some(true));
		}
		if event.kind.starts_with("artifact.") {
			let Some(artifact_id) = id(&event.data["id"]) else {
				return Ok(Some(false));
			};
			return Ok(Some(
				self.artifact_id_visible(artifact_id, event.workspace_id)
					.await?,
			));
		}
		if event.kind == "message.created" || event.kind == "message.thread_opened" {
			if event.kind == "message.thread_opened" && id(&event.data["id"]).is_none() {
				return Ok(Some(false));
			}
			let messages: Vec<Message> = if let Some(message_id) = id(&event.data["id"]) {
				sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("messages"))
						.cond_where(
							Condition::all()
								.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
								.add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
						)
						.to_string(PostgresQueryBuilder),
				)
				.bind(message_id)
				.bind(event.workspace_id)
				.fetch_all(&mut **self.tx)
				.await?
			} else {
				// Older events contain no ID. Require every matching immutable
				// row; ambiguity must never allow a denied message to escape.
				sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("messages"))
						.cond_where(
							Condition::all()
								.add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
								.add(Expr::col(Alias::new("sender")).eq(Expr::cust("$2")))
								.add(Expr::col(Alias::new("content")).eq(Expr::cust("$3"))),
						)
						.to_string(PostgresQueryBuilder),
				)
				.bind(event.workspace_id)
				.bind(event.data["sender"].as_str())
				.bind(event.data["content"].as_str())
				.fetch_all(&mut **self.tx)
				.await?
			};
			if messages.is_empty() {
				return Ok(Some(false));
			}
			for message in messages {
				if !self.message_visible(&message).await? {
					return Ok(Some(false));
				}
			}
			return Ok(Some(true));
		}
		Ok(None)
	}
	async fn artifact_id_visible(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<bool> {
		let artifact: Option<Artifact> = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("artifacts"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(workspace)
		.fetch_optional(&mut **self.tx)
		.await?;
		match artifact {
			Some(artifact) => self.artifact_visible(&artifact).await,
			None => Ok(false),
		}
	}
}

impl Access {
	/// Commit membership before a provider/tool can copy these records into its
	/// journal. The worker still retains the enclosing live authority lease.
	pub(crate) async fn track_snapshot(
		&mut self,
		snapshot: &crate::domain::WorkspaceSnapshot,
	) -> Result<()> {
		let (scope, table, column) = match (self.read_run, self.read_grant) {
			(Some(run), None) => (run, "authorization_run_reads", "run_id"),
			(None, Some(grant)) => (grant, "authorization_remote_grant_reads", "grant_id"),
			(None, None) => return Ok(()),
			(Some(_), Some(_)) => return Err(Error::Forbidden),
		};
		let mut sources: std::collections::BTreeSet<(String, Uuid)> = snapshot
			.tasks
			.iter()
			.map(|r| ("task".into(), r.id))
			.chain(snapshot.artifacts.iter().map(|r| ("artifact".into(), r.id)))
			.chain(snapshot.messages.iter().map(|r| ("message".into(), r.id)))
			.collect();
		let id = |value: &serde_json::Value| value.as_str().and_then(|s| s.parse::<Uuid>().ok());
		if !snapshot.events.is_empty() {
			sources.insert(("workspace_events".into(), snapshot.workspace.id));
		}
		for event in &snapshot.events {
			let source = if event.kind.starts_with("conversation.") {
				id(&event.data["id"]).map(|id| ("conversation", id))
			} else if event.kind.starts_with("generation.") {
				id(&event.data["id"]).map(|id| ("generation", id))
			} else if event.kind == "run.created" {
				id(&event.data["id"]).map(|id| ("run", id))
			} else {
				id(&event.data["run_id"]).map(|id| ("run", id))
			};
			if let Some((kind, id)) = source
				&& (kind != "run" || Some(id) != self.read_run)
			{
				sources.insert((kind.into(), id));
			}
			if event.kind == "message.created" || event.kind == "message.thread_opened" {
				if let Some(id) = id(&event.data["id"]) {
					sources.insert(("message".into(), id));
				} else if event.kind == "message.created" {
					let ids: Vec<Uuid> = sqlx::query_scalar(
						&Query::select()
							.column(Alias::new("id"))
							.from(Alias::new("messages"))
							.cond_where(
								Condition::all()
									.add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
									.add(Expr::col(Alias::new("sender")).eq(Expr::cust("$2")))
									.add(Expr::col(Alias::new("content")).eq(Expr::cust("$3"))),
							)
							.to_string(PostgresQueryBuilder),
					)
					.bind(event.workspace_id)
					.bind(event.data["sender"].as_str())
					.bind(event.data["content"].as_str())
					.fetch_all(&mut **self.tx)
					.await?;
					sources.extend(ids.into_iter().map(|id| ("message".into(), id)));
				}
			}
		}
		let (kinds, ids): (Vec<_>, Vec<_>) = sources.into_iter().unzip();
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new(table))
				.columns([
					Alias::new(column),
					Alias::new("workspace_id"),
					Alias::new("resource_kind"),
					Alias::new("resource_id"),
				])
				.select_from(
					Query::select()
						.exprs([
							Expr::cust("$1"),
							Expr::cust("$2"),
							Expr::cust("unnest($3::text[])"),
							Expr::cust("unnest($4::uuid[])"),
						])
						.to_owned(),
				)
				.map_err(|error| Error::Invalid(error.to_string()))?
				.on_conflict(OnConflict::new().do_nothing().to_owned())
				.to_string(PostgresQueryBuilder),
		)
		.bind(scope)
		.bind(snapshot.workspace.id)
		.bind(kinds)
		.bind(ids)
		.execute(&self.pool)
		.await?;
		Ok(())
	}
	pub(crate) async fn track_registry(
		&mut self,
		entries: &[crate::registry::Entry],
	) -> Result<()> {
		let Some(run) = self.read_run else {
			return Ok(());
		};
		let ids: Vec<_> = entries.iter().map(|entry| entry.id.clone()).collect();
		let versions: Vec<_> = entries.iter().map(|entry| entry.version.clone()).collect();
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("authorization_run_registry_reads"))
				.columns([
					Alias::new("run_id"),
					Alias::new("entry_id"),
					Alias::new("entry_version"),
				])
				.select_from(
					Query::select()
						.exprs([
							Expr::cust("$1"),
							Expr::cust("unnest($2::text[])"),
							Expr::cust("unnest($3::text[])"),
						])
						.to_owned(),
				)
				.map_err(|error| Error::Invalid(error.to_string()))?
				.on_conflict(OnConflict::new().do_nothing().to_owned())
				.to_string(PostgresQueryBuilder),
		)
		.bind(run)
		.bind(ids)
		.bind(versions)
		.execute(&self.pool)
		.await?;
		Ok(())
	}

	async fn registry_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		let entries: Vec<(String, String)> = sqlx::query_as(
			&Query::select()
				.columns([Alias::new("entry_id"), Alias::new("entry_version")])
				.from(Alias::new("authorization_run_registry_reads"))
				.and_where(Expr::col(Alias::new("run_id")).eq(Expr::cust("$1")))
				.order_by(Alias::new("entry_id"), Order::Asc)
				.order_by(Alias::new("entry_version"), Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.bind(run)
		.fetch_all(&mut **self.tx)
		.await?;
		for (id, version) in entries {
			match super::catalog::entry(
				self,
				&crate::registry::EntityRef { id, version },
				"registry.read",
			)
			.await
			{
				Ok(_) => {}
				Err(Error::Forbidden) => return Ok(false),
				Err(error) => return Err(error),
			}
		}
		Ok(true)
	}

	/// Walk recorded run dependencies iteratively; cycles between observation
	/// journals must terminate without skipping any resource's current policy.
	pub(crate) async fn run_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		let key = (run, self.authority_context());
		if !self.checking_reads.insert(key.clone()) {
			return Ok(true);
		}
		let result = self.run_reads_visible_in(run).await;
		self.checking_reads.remove(&key);
		result
	}
	async fn run_reads_visible_in(&mut self, run: Uuid) -> Result<bool> {
		let mut pending = vec![run];
		let mut visited = std::collections::BTreeSet::new();
		while let Some(run) = pending.pop() {
			if !visited.insert(run) {
				continue;
			}
			if !self.registry_reads_visible(run).await?
				|| !self.remote_reads_visible(run).await?
				|| !self.semantic_reads_visible(run).await?
			{
				return Ok(false);
			}
			let sources: Vec<(Uuid, String, Uuid)> = sqlx::query_as(
				&Query::select()
					.column(Alias::new("workspace_id"))
					.column(Alias::new("resource_kind"))
					.column(Alias::new("resource_id"))
					.from(Alias::new("authorization_run_reads"))
					.cond_where(Expr::col(Alias::new("run_id")).eq(Expr::cust("$1")))
					.order_by(Alias::new("resource_kind"), Order::Asc)
					.order_by(Alias::new("resource_id"), Order::Asc)
					.to_string(PostgresQueryBuilder),
			)
			.bind(run)
			.fetch_all(&mut **self.tx)
			.await?;
			for (workspace, kind, id) in sources {
				let allowed = self
					.read_source_visible(workspace, &kind, id, &mut pending)
					.await?;
				if !allowed {
					return Ok(false);
				}
			}
		}
		Ok(true)
	}
	pub(crate) async fn grant_reads_visible(&mut self, grant: Uuid) -> Result<bool> {
		let sources: Vec<(Uuid, String, Uuid)> = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("workspace_id")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("resource_kind")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("resource_id")),
				))
				.from(sea_orm::sea_query::Alias::new(
					"authorization_remote_grant_reads",
				))
				.and_where(sea_orm::sea_query::Expr::cust("grant_id = $1"))
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("resource_kind"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("resource_id"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(grant)
		.fetch_all(&mut **self.tx)
		.await?;
		let mut pending = vec![];
		for (workspace, kind, id) in sources {
			if !self
				.read_source_visible(workspace, &kind, id, &mut pending)
				.await?
			{
				return Ok(false);
			}
		}
		for run in pending {
			if !self.run_reads_visible(run).await? {
				return Ok(false);
			}
		}
		Ok(true)
	}
	async fn read_source_visible(
		&mut self,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
		pending: &mut Vec<Uuid>,
	) -> Result<bool> {
		Ok(match kind {
			"workspace_events" => {
				let resource = self.workspace(workspace).await?;
				self.decide(&resource, "workspace.events").await?
			}
			"task" => {
				let source: Option<Task> = sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("tasks"))
						.cond_where(
							Condition::all()
								.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
								.add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
						)
						.to_string(PostgresQueryBuilder),
				)
				.bind(id)
				.bind(workspace)
				.fetch_optional(&mut **self.tx)
				.await?;
				match source {
					Some(source) => self.task_visible(&source).await?,
					None => false,
				}
			}
			"artifact" => self.artifact_id_visible(id, Some(workspace)).await?,
			"message" => {
				let source: Option<Message> = sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("messages"))
						.cond_where(
							Condition::all()
								.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
								.add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
						)
						.to_string(PostgresQueryBuilder),
				)
				.bind(id)
				.bind(workspace)
				.fetch_optional(&mut **self.tx)
				.await?;
				match source {
					Some(source) => self.message_visible(&source).await?,
					None => false,
				}
			}
			"run" => {
				let source: Option<crate::domain::Run> = sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("runs"))
						.cond_where(
							Condition::all()
								.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
								.add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
						)
						.to_string(PostgresQueryBuilder),
				)
				.bind(id)
				.bind(workspace)
				.fetch_optional(&mut **self.tx)
				.await?;
				match source {
					Some(source) => {
						pending.push(source.id);
						self.run_base_visible(&source).await?
					}
					None => false,
				}
			}
			"conversation" => {
				let source: Option<crate::domain::Conversation> = sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("conversations"))
						.cond_where(
							Condition::all()
								.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
								.add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
						)
						.to_string(PostgresQueryBuilder),
				)
				.bind(id)
				.bind(workspace)
				.fetch_optional(&mut **self.tx)
				.await?;
				match source {
					Some(source) => {
						let resource = self.conversation_resource(&source).await?;
						self.decide(&resource, "conversation.read").await?
					}
					None => false,
				}
			}
			"generation" => {
				let source: Option<crate::generation::Request> = sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("generation_requests"))
						.cond_where(
							Condition::all()
								.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
								.add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
						)
						.to_string(PostgresQueryBuilder),
				)
				.bind(id)
				.bind(workspace)
				.fetch_optional(&mut **self.tx)
				.await?;
				match source {
					Some(source) => source.visible(self).await?,
					None => false,
				}
			}
			_ => false,
		})
	}
}
