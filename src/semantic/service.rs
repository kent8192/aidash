use super::*;
use crate::{
	authorization::{
		access::Access,
		identity::{Actor, SubjectIdentity},
	},
	domain::{Artifact, Message},
	store::Store,
};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use std::collections::BTreeMap;

/// Only constructed from authenticated in-process identities; never accepted
/// from API input. Persisted jobs retain the initiator and delegation chain.
#[derive(Serialize, Deserialize)]
pub(crate) struct SavedAuthority {
	credential: Option<Uuid>,
	tenant: String,
	subject: String,
	subjects: Vec<String>,
}
pub(crate) enum Lease<'a> {
	Operator(Transaction<'static, Postgres>),
	Scoped(Box<Access>),
	Inherited(&'a mut Access),
}
impl Lease<'_> {
	pub(crate) async fn begin(store: &Store, actor: &Actor) -> Result<Self> {
		Ok(match actor {
			Actor::Operator => Self::Operator(store.pool.begin().await?),
			Actor::Subject(identity) => {
				Self::Scoped(Box::new(Access::begin(store, identity).await?))
			}
		})
	}
	pub(crate) async fn restore(store: &Store, authority: Value) -> Result<Self> {
		let authority: SavedAuthority = serde_json::from_value(authority)?;
		match authority.credential {
			None if authority.subject == "operator"
				&& authority.tenant.is_empty()
				&& authority.subjects.is_empty() =>
			{
				Self::begin(store, &Actor::Operator).await
			}
			Some(credential_id) => {
				let mut access = Access::begin(
					store,
					&SubjectIdentity {
						credential_id,
						tenant: authority.tenant,
						subject: authority.subject,
					},
				)
				.await?;
				if authority.subjects.is_empty()
					|| !authority.subjects.contains(&access.identity.subject)
				{
					return Err(Error::Forbidden);
				}
				access.subjects = authority.subjects;
				access.worker();
				Ok(Self::Scoped(Box::new(access)))
			}
			_ => Err(Error::Forbidden),
		}
	}
	pub(crate) fn tx(&mut self) -> &mut Transaction<'static, Postgres> {
		match self {
			Self::Operator(tx) => tx,
			Self::Scoped(a) => &mut a.tx,
			Self::Inherited(a) => &mut a.tx,
		}
	}
	fn access(&mut self) -> Option<&mut Access> {
		match self {
			Self::Operator(_) => None,
			Self::Scoped(a) => Some(a),
			Self::Inherited(a) => Some(a),
		}
	}
	pub(crate) fn durable(&mut self) {
		if let Some(access) = self.access() {
			access.durable_audit = true;
		}
	}
	pub(crate) fn saved(&mut self) -> Result<Value> {
		let saved = match self.access() {
			None => SavedAuthority {
				credential: None,
				tenant: String::new(),
				subject: "operator".into(),
				subjects: vec![],
			},
			Some(a) => SavedAuthority {
				credential: Some(a.identity.credential_id),
				tenant: a.identity.tenant.clone(),
				subject: a.identity.subject.clone(),
				subjects: a.subjects.clone(),
			},
		};
		Ok(serde_json::to_value(saved)?)
	}
	pub(crate) async fn finish<T>(self, result: Result<T>) -> Result<T> {
		match self {
			Self::Operator(tx) => {
				if result.is_ok() {
					tx.commit().await?;
				} else {
					tx.rollback().await?;
				}
				result
			}
			Self::Scoped(a) => a.finish(result).await,
			Self::Inherited(_) => result,
		}
	}
	pub(crate) async fn workspace(&mut self, workspace: Uuid, action: &str) -> Result<()> {
		if let Some(a) = self.access() {
			let resource = a.workspace(workspace).await?;
			a.require(&resource, "workspace.read").await?;
			a.require(&resource, action).await?;
		}
		Ok(())
	}
	pub(crate) async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool> {
		let scoped = self.access().is_some();
		if let Some(a) = self.access() {
			let workspace = a.workspace(entry.workspace_id).await?;
			let mut attributes = workspace.attributes;
			attributes["created_by"] = json!(entry.created_by);
			attributes["agent"] = json!(entry.agent);
			attributes["metadata"] = entry.metadata.clone();
			let resource = a.resource("semantic", entry.id, attributes.clone());
			if !a.decide(&resource, action).await? {
				return Ok(false);
			}
			let managed: Option<(String, String)> = sqlx::query_as(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("agent_id")),
					))
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new(
							"agent_version",
						)),
					))
					.from(sea_orm::sea_query::Alias::new("semantic_agent_memory"))
					.and_where(sea_orm::sea_query::Expr::cust("entry_id = $1"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(entry.id)
			.fetch_optional(&mut **a.tx)
			.await?;
			if let Some((id, version)) = managed {
				attributes["created_by"] = json!(entry.agent);
				attributes["version"] = json!(version);
				let original = a.resource("memory", id, attributes);
				if !a
					.decide(
						&original,
						if action == "semantic.read" {
							"memory.read"
						} else {
							"memory.write"
						},
					)
					.await?
				{
					return Ok(false);
				}
			}
		}
		// Mutation replays and history responses retain the same underlying
		// read requirements as retrieval, including after source revocation.
		if scoped && action == "semantic.read" && !entry.deleted {
			let source = serde_json::from_value(entry.source.clone())?;
			if !matches!(source, Source::Memory { .. })
				&& self.source(entry.workspace_id, &source).await?.is_none()
			{
				return Ok(false);
			}
		}
		Ok(true)
	}
	pub(crate) async fn source(
		&mut self,
		workspace: Uuid,
		source: &Source,
	) -> Result<Option<String>> {
		match source {
			Source::Memory { text } => Ok(Some(text.clone())),
			Source::Artifact { id } => {
				let row: Option<Artifact> = sqlx::query_as(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
						))
						.from(sea_orm::sea_query::Alias::new("artifacts"))
						.and_where(sea_orm::sea_query::Expr::cust(
							"id = $1 AND workspace_id = $2",
						))
						.lock(sea_orm::sea_query::LockType::Share)
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(id)
				.bind(workspace)
				.fetch_optional(&mut **self.tx())
				.await?;
				let Some(row) = row else { return Ok(None) };
				if let Some(a) = self.access()
					&& !a.artifact_visible(&row).await?
				{
					return Ok(None);
				}
				Ok(Some(serde_json::to_string(&row.content)?))
			}
			Source::Message { id } => {
				let row: Option<Message> = sqlx::query_as(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
						))
						.from(sea_orm::sea_query::Alias::new("messages"))
						.and_where(sea_orm::sea_query::Expr::cust(
							"id = $1 AND workspace_id = $2",
						))
						.lock(sea_orm::sea_query::LockType::Share)
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(id)
				.bind(workspace)
				.fetch_optional(&mut **self.tx())
				.await?;
				let Some(row) = row else { return Ok(None) };
				if let Some(a) = self.access()
					&& !a.message_visible(&row).await?
				{
					return Ok(None);
				}
				Ok(Some(row.content))
			}
		}
	}
}
pub(crate) async fn index(
	tx: &mut Transaction<'_, Postgres>,
	workspace: Uuid,
	exclusive: bool,
) -> Result<Index> {
	sqlx::query_as(&if exclusive {
		sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("semantic_indexes"))
			.and_where(sea_orm::sea_query::Expr::cust("workspace_id = $1"))
			.lock(sea_orm::sea_query::LockType::Update)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder)
	} else {
		sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("semantic_indexes"))
			.and_where(sea_orm::sea_query::Expr::cust("workspace_id = $1"))
			.lock(sea_orm::sea_query::LockType::Share)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder)
	})
	.bind(workspace)
	.fetch_optional(&mut **tx)
	.await?
	.ok_or_else(|| Error::NotFound("semantic index".into()))
}
pub(crate) async fn history(
	tx: &mut Transaction<'_, Postgres>,
	workspace: Uuid,
	entry: Option<Uuid>,
	revision: i64,
	state: &str,
	detail: &str,
) -> Result<()> {
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("semantic_history"))
			.columns([
				sea_orm::sea_query::Alias::new("workspace_id"),
				sea_orm::sea_query::Alias::new("entry_id"),
				sea_orm::sea_query::Alias::new("revision"),
				sea_orm::sea_query::Alias::new("state"),
				sea_orm::sea_query::Alias::new("detail"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("$3"),
				sea_orm::sea_query::Expr::cust("$4"),
				sea_orm::sea_query::Expr::cust("$5"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.bind(entry)
	.bind(revision)
	.bind(state)
	.bind(detail)
	.execute(&mut **tx)
	.await?;
	Ok(())
}
pub(crate) async fn schedule_point(
	tx: &mut Transaction<'_, Postgres>,
	entry: &Entry,
	collection: &str,
) -> Result<()> {
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("semantic_points"))
			.value(
				sea_orm::sea_query::Alias::new("retired"),
				sea_orm::sea_query::Expr::cust("TRUE"),
			)
			.value(
				sea_orm::sea_query::Alias::new("next_attempt"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
			)
			.and_where(sea_orm::sea_query::Expr::cust(
				"entry_id = $1 AND NOT retired",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(entry.id)
	.execute(&mut **tx)
	.await?;
	if !entry.deleted {
		sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("semantic_points"))
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("entry_id"),
					sea_orm::sea_query::Alias::new("collection"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
				])
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(entry.point_id)
		.bind(entry.id)
		.bind(collection)
		.execute(&mut **tx)
		.await?;
	}
	Ok(())
}
pub async fn configure(store: &Store, workspace: Uuid, input: ConfigureIndex) -> Result<Index> {
	input.spec.validate()?;
	if input.expected_revision < 0 || input.expected_revision == i64::MAX {
		return Err(Error::Invalid("invalid index revision".into()));
	}
	let mut tx = store.pool.begin().await?;
	let exists: Option<Uuid> = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id")),
			))
			.from(sea_orm::sea_query::Alias::new("workspaces"))
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.lock(sea_orm::sea_query::LockType::Update)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.fetch_optional(&mut *tx)
	.await?;
	if exists.is_none() {
		return Err(Error::NotFound("workspace".into()));
	}
	let tenant: String = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("tenant")),
			))
			.from(sea_orm::sea_query::Alias::new("authorization_workspaces"))
			.and_where(sea_orm::sea_query::Expr::cust("workspace_id = $1"))
			.lock(sea_orm::sea_query::LockType::Share)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.fetch_optional(&mut *tx)
	.await?
	.unwrap_or_default();
	let old: Option<Index> = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("semantic_indexes"))
			.and_where(sea_orm::sea_query::Expr::cust("workspace_id = $1"))
			.lock(sea_orm::sea_query::LockType::Update)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.fetch_optional(&mut *tx)
	.await?;
	let spec = serde_json::to_value(&input.spec)?;
	if old.as_ref().map_or(0, |i| i.revision) != input.expected_revision {
		if let Some(old) = old
			&& old.revision == input.expected_revision + 1
			&& old.spec == spec
		{
			tx.commit().await?;
			return Ok(old);
		}
		return Err(Error::Conflict("semantic index revision changed".into()));
	}
	let count: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("semantic_entries"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"workspace_id = $1 AND NOT deleted",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.fetch_one(&mut *tx)
	.await?;
	if count > input.spec.max_sources as i64 {
		return Err(Error::Conflict(
			"index limit is below the current source count".into(),
		));
	}
	let collection = format!("aidash_{}", Uuid::new_v4().simple());
	let current: Index = sqlx::query_as(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("semantic_indexes"))
			.columns([
				sea_orm::sea_query::Alias::new("workspace_id"),
				sea_orm::sea_query::Alias::new("tenant"),
				sea_orm::sea_query::Alias::new("revision"),
				sea_orm::sea_query::Alias::new("spec"),
				sea_orm::sea_query::Alias::new("collection"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("$3"),
				sea_orm::sea_query::Expr::cust("$4"),
				sea_orm::sea_query::Expr::cust("$5"),
			])
			.on_conflict(
				sea_orm::sea_query::OnConflict::columns([sea_orm::sea_query::Alias::new(
					"workspace_id",
				)])
				.value(
					sea_orm::sea_query::Alias::new("revision"),
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
						sea_orm::sea_query::Alias::new("excluded"),
						sea_orm::sea_query::Alias::new("revision"),
					))),
				)
				.value(
					sea_orm::sea_query::Alias::new("spec"),
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
						sea_orm::sea_query::Alias::new("excluded"),
						sea_orm::sea_query::Alias::new("spec"),
					))),
				)
				.value(
					sea_orm::sea_query::Alias::new("collection"),
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
						sea_orm::sea_query::Alias::new("excluded"),
						sea_orm::sea_query::Alias::new("collection"),
					))),
				)
				.value(
					sea_orm::sea_query::Alias::new("updated_at"),
					sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
				)
				.to_owned(),
			)
			.returning_all()
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.bind(tenant)
	.bind(input.expected_revision + 1)
	.bind(spec)
	.bind(&collection)
	.fetch_one(&mut *tx)
	.await?;
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("semantic_collections"))
			.value(
				sea_orm::sea_query::Alias::new("retired"),
				sea_orm::sea_query::Expr::cust("TRUE"),
			)
			.value(
				sea_orm::sea_query::Alias::new("next_attempt"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("workspace_id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.execute(&mut *tx)
	.await?;
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("semantic_collections"))
			.columns([
				sea_orm::sea_query::Alias::new("collection"),
				sea_orm::sea_query::Alias::new("workspace_id"),
				sea_orm::sea_query::Alias::new("vector"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("$3"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(&collection)
	.bind(workspace)
	.bind(serde_json::to_value(&input.spec.vector)?)
	.execute(&mut *tx)
	.await?;
	let entries: Vec<Entry> = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("semantic_entries"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"workspace_id = $1 AND NOT deleted",
			))
			.order_by_expr(
				sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
					sea_orm::sea_query::Alias::new("id"),
				)),
				sea_orm::sea_query::Order::Asc,
			)
			.lock(sea_orm::sea_query::LockType::Update)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.fetch_all(&mut *tx)
	.await?;
	for entry in entries {
		let entry: Entry = sqlx::query_as(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("semantic_entries"))
				.value(
					sea_orm::sea_query::Alias::new("point_id"),
					sea_orm::sea_query::Expr::cust("$2"),
				)
				.value(
					sea_orm::sea_query::Alias::new("index_revision"),
					sea_orm::sea_query::Expr::cust("$3"),
				)
				.value(
					sea_orm::sea_query::Alias::new("state"),
					sea_orm::sea_query::Expr::cust("'PENDING'"),
				)
				.value(
					sea_orm::sea_query::Alias::new("last_error"),
					sea_orm::sea_query::Expr::cust("NULL"),
				)
				.value(
					sea_orm::sea_query::Alias::new("attempts"),
					sea_orm::sea_query::Expr::cust("0"),
				)
				.value(
					sea_orm::sea_query::Alias::new("next_attempt"),
					sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
				)
				.value(
					sea_orm::sea_query::Alias::new("updated_at"),
					sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
				)
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(entry.id)
		.bind(Uuid::new_v4())
		.bind(current.revision)
		.fetch_one(&mut *tx)
		.await?;
		schedule_point(&mut tx, &entry, &collection).await?;
	}
	history(
		&mut tx,
		workspace,
		None,
		current.revision,
		"CONFIGURED",
		"new immutable index generation",
	)
	.await?;
	tx.commit().await?;
	Ok(current)
}
pub async fn get_index(store: &Store, actor: &Actor, workspace: Uuid) -> Result<Index> {
	let mut lease = Lease::begin(store, actor).await?;
	let result = async {
		lease.workspace(workspace, "semantic.read").await?;
		index(lease.tx(), workspace, false).await
	}
	.await;
	lease.finish(result).await
}
pub async fn put(store: &Store, actor: &Actor, workspace: Uuid, input: PutEntry) -> Result<Entry> {
	input.validate()?;
	let mut lease = Lease::begin(store, actor).await?;
	let result = if input.key.starts_with("agent-memory:") {
		Err(Error::Invalid(
			"agent memory slots are updated through memory_write".into(),
		))
	} else {
		put_in(&mut lease, workspace, input).await
	};
	lease.finish(result).await
}
pub(crate) async fn put_in(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	input: PutEntry,
) -> Result<Entry> {
	input.validate()?;
	lease.workspace(workspace, "semantic.write").await?;
	let index = index(lease.tx(), workspace, true).await?;
	let spec = index.configuration()?;
	let old: Option<Entry> = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("semantic_entries"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"workspace_id = $1 AND key = $2",
			))
			.lock(sea_orm::sea_query::LockType::Update)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.bind(&input.key)
	.fetch_optional(&mut **lease.tx())
	.await?;
	let saved = lease.saved()?;
	let mut entry = Entry {
		id: old.as_ref().map_or_else(Uuid::new_v4, |e| e.id),
		workspace_id: workspace,
		key: input.key,
		source: serde_json::to_value(&input.source)?,
		agent: input.agent,
		metadata: input.metadata,
		revision: input.expected_revision + 1,
		point_id: Uuid::new_v4(),
		index_revision: index.revision,
		deleted: false,
		state: "PENDING".into(),
		attempts: 0,
		last_error: None,
		created_by: saved["subject"].as_str().unwrap_or_default().into(),
		updated_at: Utc::now(),
	};
	if let Some(old) = &old {
		if !lease.permits(old, "semantic.write").await?
			|| !lease.permits(old, "semantic.read").await?
		{
			return Err(Error::Forbidden);
		}
		if old.deleted {
			return Err(Error::Conflict(
				"deleted semantic keys cannot be reused".into(),
			));
		}
		if !input
			.source
			.same_origin(&serde_json::from_value(old.source.clone())?)
		{
			return Err(Error::Conflict(
				"semantic source identity is immutable".into(),
			));
		}
		entry.created_by = old.created_by.clone();
		if (input.expected_revision == 0
			|| old.revision == input.expected_revision + 1
			|| old.revision == input.expected_revision)
			&& old.source == entry.source
			&& old.agent == entry.agent
			&& old.metadata == entry.metadata
		{
			return Ok(old.clone());
		}
		if old.revision != input.expected_revision {
			return Err(Error::Conflict("semantic source revision changed".into()));
		}
	} else {
		if input.expected_revision != 0 {
			return Err(Error::Conflict("semantic source does not exist".into()));
		}
		let count: i64 = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
				.from(sea_orm::sea_query::Alias::new("semantic_entries"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"workspace_id = $1 AND NOT deleted",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(workspace)
		.fetch_one(&mut **lease.tx())
		.await?;
		if count >= spec.max_sources as i64 {
			return Err(Error::Conflict("semantic source limit reached".into()));
		}
	}
	if !lease.permits(&entry, "semantic.write").await?
		|| !lease.permits(&entry, "semantic.read").await?
	{
		return Err(Error::Forbidden);
	}
	let text = lease
		.source(workspace, &input.source)
		.await?
		.ok_or(Error::Forbidden)?;
	validate_text(&text, spec.max_input_bytes)?;
	let entry: Entry = sqlx::query_as(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("semantic_entries"))
			.columns([
				sea_orm::sea_query::Alias::new("id"),
				sea_orm::sea_query::Alias::new("workspace_id"),
				sea_orm::sea_query::Alias::new("key"),
				sea_orm::sea_query::Alias::new("source"),
				sea_orm::sea_query::Alias::new("agent"),
				sea_orm::sea_query::Alias::new("metadata"),
				sea_orm::sea_query::Alias::new("revision"),
				sea_orm::sea_query::Alias::new("point_id"),
				sea_orm::sea_query::Alias::new("index_revision"),
				sea_orm::sea_query::Alias::new("deleted"),
				sea_orm::sea_query::Alias::new("state"),
				sea_orm::sea_query::Alias::new("created_by"),
				sea_orm::sea_query::Alias::new("authority"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("$3"),
				sea_orm::sea_query::Expr::cust("$4"),
				sea_orm::sea_query::Expr::cust("$5"),
				sea_orm::sea_query::Expr::cust("$6"),
				sea_orm::sea_query::Expr::cust("$7"),
				sea_orm::sea_query::Expr::cust("$8"),
				sea_orm::sea_query::Expr::cust("$9"),
				sea_orm::sea_query::Expr::cust("FALSE"),
				sea_orm::sea_query::Expr::cust("'PENDING'"),
				sea_orm::sea_query::Expr::cust("$10"),
				sea_orm::sea_query::Expr::cust("$11"),
			])
			.on_conflict(
				sea_orm::sea_query::OnConflict::columns([sea_orm::sea_query::Alias::new("id")])
					.value(
						sea_orm::sea_query::Alias::new("source"),
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
							sea_orm::sea_query::Alias::new("excluded"),
							sea_orm::sea_query::Alias::new("source"),
						))),
					)
					.value(
						sea_orm::sea_query::Alias::new("agent"),
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
							sea_orm::sea_query::Alias::new("excluded"),
							sea_orm::sea_query::Alias::new("agent"),
						))),
					)
					.value(
						sea_orm::sea_query::Alias::new("metadata"),
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
							sea_orm::sea_query::Alias::new("excluded"),
							sea_orm::sea_query::Alias::new("metadata"),
						))),
					)
					.value(
						sea_orm::sea_query::Alias::new("revision"),
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
							sea_orm::sea_query::Alias::new("excluded"),
							sea_orm::sea_query::Alias::new("revision"),
						))),
					)
					.value(
						sea_orm::sea_query::Alias::new("point_id"),
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
							sea_orm::sea_query::Alias::new("excluded"),
							sea_orm::sea_query::Alias::new("point_id"),
						))),
					)
					.value(
						sea_orm::sea_query::Alias::new("index_revision"),
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
							sea_orm::sea_query::Alias::new("excluded"),
							sea_orm::sea_query::Alias::new("index_revision"),
						))),
					)
					.value(
						sea_orm::sea_query::Alias::new("state"),
						sea_orm::sea_query::Expr::cust("'PENDING'"),
					)
					.value(
						sea_orm::sea_query::Alias::new("attempts"),
						sea_orm::sea_query::Expr::cust("0"),
					)
					.value(
						sea_orm::sea_query::Alias::new("last_error"),
						sea_orm::sea_query::Expr::cust("NULL"),
					)
					.value(
						sea_orm::sea_query::Alias::new("authority"),
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
							sea_orm::sea_query::Alias::new("excluded"),
							sea_orm::sea_query::Alias::new("authority"),
						))),
					)
					.value(
						sea_orm::sea_query::Alias::new("next_attempt"),
						sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
					)
					.value(
						sea_orm::sea_query::Alias::new("updated_at"),
						sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
					)
					.to_owned(),
			)
			.returning_all()
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(entry.id)
	.bind(workspace)
	.bind(entry.key)
	.bind(entry.source)
	.bind(entry.agent)
	.bind(entry.metadata)
	.bind(entry.revision)
	.bind(entry.point_id)
	.bind(index.revision)
	.bind(entry.created_by)
	.bind(saved)
	.fetch_one(&mut **lease.tx())
	.await?;
	schedule_point(lease.tx(), &entry, &index.collection).await?;
	history(
		lease.tx(),
		workspace,
		Some(entry.id),
		entry.revision,
		"PENDING",
		"source accepted",
	)
	.await?;
	Ok(entry)
}
pub(crate) fn validate_text(text: &str, max: usize) -> Result<()> {
	if text.trim().is_empty() || text.len() > max {
		Err(Error::Invalid(
			"semantic text is empty or exceeds the configured byte limit".into(),
		))
	} else {
		Ok(())
	}
}
pub async fn entries(store: &Store, actor: &Actor, workspace: Uuid) -> Result<Vec<Entry>> {
	let mut lease = Lease::begin(store, actor).await?;
	let result = async {
		lease.workspace(workspace, "semantic.read").await?;
		index(lease.tx(), workspace, false).await?;
		let rows: Vec<Entry> = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("semantic_entries"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"workspace_id = $1 AND NOT deleted",
				))
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("id"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(workspace)
		.fetch_all(&mut **lease.tx())
		.await?;
		let mut visible = vec![];
		for entry in rows {
			if lease.permits(&entry, "semantic.read").await?
				&& (entry.deleted
					|| lease
						.source(workspace, &serde_json::from_value(entry.source.clone())?)
						.await?
						.is_some())
			{
				visible.push(entry);
			}
		}
		Ok(visible)
	}
	.await;
	lease.finish(result).await
}
pub async fn change(
	store: &Store,
	actor: &Actor,
	workspace: Uuid,
	id: Uuid,
	revision: i64,
	delete: bool,
) -> Result<Entry> {
	if revision < 0 || revision == i64::MAX {
		return Err(Error::Invalid("invalid source revision".into()));
	}
	let mut lease = Lease::begin(store, actor).await?;
	let result=async {
        lease.workspace(workspace,if delete {"semantic.delete"} else {"semantic.write"}).await?;
        let index=index(lease.tx(),workspace,true).await?;
        let entry:Entry=sqlx::query_as(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))).from(sea_orm::sea_query::Alias::new("semantic_entries")).and_where(sea_orm::sea_query::Expr::cust("workspace_id = $1 AND id = $2")).lock(sea_orm::sea_query::LockType::Update).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(workspace).bind(id).fetch_optional(&mut **lease.tx()).await?.ok_or(Error::Forbidden)?;
        if !lease.permits(&entry,if delete {"semantic.delete"} else {"semantic.write"}).await? {return Err(Error::Forbidden);}
        if !lease.permits(&entry,"semantic.read").await? {return Err(Error::Forbidden);}
        if !delete && !entry.deleted && (0..i64::MAX).contains(&revision) && entry.revision==revision+1 {
            let replay:bool=sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("EXISTS(SELECT 1 FROM semantic_history WHERE entry_id = $1 AND revision = $2 AND state = 'PENDING' AND detail = 'reindex requested')")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(id).bind(entry.revision).fetch_one(&mut **lease.tx()).await?;
            if replay {return Ok(entry);}
        }
        if entry.deleted && delete && (entry.revision==revision || entry.revision==revision+1) {return Ok(entry);}
        if entry.deleted || entry.revision!=revision || revision==i64::MAX {return Err(Error::Conflict("semantic source revision changed or deleted".into()));}
        if !delete {
            let text=lease.source(workspace,&serde_json::from_value(entry.source.clone())?).await?.ok_or(Error::Forbidden)?;
            validate_text(&text,index.configuration()?.max_input_bytes)?;
        }
        if delete {
            let managed:Option<(String,String,String)>=sqlx::query_as(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("agent_id")))).expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("agent_version")))).expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("home_node")))).from(sea_orm::sea_query::Alias::new("semantic_agent_memory")).and_where(sea_orm::sea_query::Expr::cust("entry_id = $1")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(id).fetch_optional(&mut **lease.tx()).await?;
            if let Some((agent_id,agent_version,home))=managed {
                if let Some(a)=lease.access() {
                    let workspace=a.workspace(workspace).await?;
                    let mut attributes=workspace.attributes;
                    attributes["created_by"]=json!(crate::domain::qualified_agent(&store.node_id,&agent_id,&agent_version));
                    attributes["version"]=json!(agent_version);
                    let resource=a.resource("memory",&agent_id,attributes);
                    a.require(&resource,"memory.write").await?;
                }
                sqlx::query(&sea_orm::sea_query::Query::delete().from_table(sea_orm::sea_query::Alias::new("memory")).and_where(sea_orm::sea_query::Expr::cust("workspace_id = $1 AND agent_id = $2 AND agent_version = $3 AND home_node = $4")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(workspace).bind(agent_id).bind(agent_version).bind(home).execute(&mut **lease.tx()).await?;
            }
        }
        let saved=lease.saved()?;
        let entry:Entry=sqlx::query_as(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("semantic_entries")).value(sea_orm::sea_query::Alias::new("revision"), sea_orm::sea_query::Expr::cust("revision + 1")).value(sea_orm::sea_query::Alias::new("point_id"), sea_orm::sea_query::Expr::cust("$3")).value(sea_orm::sea_query::Alias::new("index_revision"), sea_orm::sea_query::Expr::cust("$4")).value(sea_orm::sea_query::Alias::new("deleted"), sea_orm::sea_query::Expr::cust("$5")).value(sea_orm::sea_query::Alias::new("state"), sea_orm::sea_query::Expr::cust("$6")).value(sea_orm::sea_query::Alias::new("source"), sea_orm::sea_query::Expr::cust("CASE WHEN $5 THEN JSONB_BUILD_OBJECT('kind', 'memory', 'text', '') ELSE source END")).value(sea_orm::sea_query::Alias::new("authority"), sea_orm::sea_query::Expr::cust("$7")).value(sea_orm::sea_query::Alias::new("attempts"), sea_orm::sea_query::Expr::cust("0")).value(sea_orm::sea_query::Alias::new("last_error"), sea_orm::sea_query::Expr::cust("NULL")).value(sea_orm::sea_query::Alias::new("next_attempt"), sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()")).value(sea_orm::sea_query::Alias::new("updated_at"), sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()")).and_where(sea_orm::sea_query::Expr::cust("id = $1 AND workspace_id = $2")).returning_all().to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .bind(id).bind(workspace).bind(Uuid::new_v4()).bind(index.revision).bind(delete).bind(if delete {"DELETED"} else {"PENDING"}).bind(saved).fetch_one(&mut **lease.tx()).await?;
        schedule_point(lease.tx(),&entry,&index.collection).await?;
        history(lease.tx(),workspace,Some(id),entry.revision,&entry.state,if delete {"source tombstoned"} else {"reindex requested"}).await?;
        Ok(entry)
    }.await;
	lease.finish(result).await
}
pub async fn search(
	store: &Store,
	actor: &Actor,
	workspace: Uuid,
	input: Search,
) -> Result<SearchResult> {
	let mut lease = Lease::begin(store, actor).await?;
	lease.durable();
	let result = search_in(store, &mut lease, workspace, input, None).await;
	lease.finish(result).await
}
pub(crate) async fn search_in(
	store: &Store,
	lease: &mut Lease<'_>,
	workspace: Uuid,
	input: Search,
	run: Option<Uuid>,
) -> Result<SearchResult> {
	lease.workspace(workspace, "semantic.search").await?;
	let index = index(lease.tx(), workspace, false).await?;
	let spec = index.configuration()?;
	if !spec.enabled {
		return Err(Error::Conflict("semantic index is disabled".into()));
	}
	validate_text(&input.query, spec.max_input_bytes)?;
	if input.limit == 0
		|| input.limit > spec.max_results
		|| input.max_tokens == 0
		|| input.max_tokens > spec.max_result_tokens
		|| !input.metadata.is_object()
		|| serde_json::to_vec(&input.metadata)?.len() > 4096
	{
		return Err(Error::Invalid(
			"invalid semantic search limits or filters".into(),
		));
	}
	let rows: Vec<Entry> = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("semantic_entries"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"workspace_id = $1 AND NOT deleted",
			))
			.order_by_expr(
				sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
					sea_orm::sea_query::Alias::new("id"),
				)),
				sea_orm::sea_query::Order::Asc,
			)
			.lock(sea_orm::sea_query::LockType::Share)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.fetch_all(&mut **lease.tx())
	.await?;
	if rows.len() > spec.max_sources {
		return Err(Error::Conflict("semantic source quota exceeded".into()));
	}
	let mut allowed = BTreeMap::new();
	let mut incomplete = false;
	for entry in rows {
		if entry.agent.is_some() && entry.agent != input.agent {
			continue;
		}
		if !input
			.metadata
			.as_object()
			.expect("validated object")
			.iter()
			.all(|(k, v)| entry.metadata.get(k) == Some(v))
		{
			continue;
		}
		if !lease.permits(&entry, "semantic.read").await? {
			continue;
		}
		let Some(text) = lease
			.source(workspace, &serde_json::from_value(entry.source.clone())?)
			.await?
		else {
			continue;
		};
		// Source content must still match the bytes used for this point; linked
		// artifacts/messages can change outside this module's revision counter.
		let digest: String = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"COALESCE(content_digest, '')",
				))
				.from(sea_orm::sea_query::Alias::new("semantic_points"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1 AND NOT retired"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(entry.point_id)
		.fetch_optional(&mut **lease.tx())
		.await?
		.unwrap_or_default();
		if entry.state != "READY"
			|| entry.index_revision != index.revision
			|| digest != content_digest(&text)
		{
			incomplete = true;
			continue;
		}
		allowed.insert(entry.point_id, (entry, text));
	}
	if incomplete {
		return Err(Error::Conflict(
			"semantic index is incomplete; inspect entries and retry after indexing".into(),
		));
	}
	let mut result = SearchResult {
		workspace_id: workspace,
		index_revision: index.revision,
		model: spec.embedding.model.clone(),
		model_version: spec.embedding.model_version.clone(),
		matches: vec![],
		estimated_tokens: 0,
		truncated: false,
	};
	result.estimated_tokens = result_tokens(&result)?;
	if result.estimated_tokens > input.max_tokens {
		return Err(Error::Invalid(
			"semantic token budget cannot hold provenance".into(),
		));
	}
	if allowed.is_empty() {
		return Ok(result);
	}
	if !backend::present(
		&store.semantic_client,
		&spec.vector,
		&index.collection,
		&allowed.keys().copied().collect::<Vec<_>>(),
	)
	.await
	.map_err(|_| Error::SemanticUnavailable)?
	{
		return Err(Error::SemanticUnavailable);
	}
	let vector = embed(
		store,
		lease,
		workspace,
		&spec.embedding,
		&input.query,
		crate::generation::embedding::Origin::Query(run),
	)
	.await?;
	let points = backend::query(
		&store.semantic_client,
		&spec.vector,
		&index.collection,
		&vector,
		backend::Filter {
			allowed: &allowed.keys().copied().collect::<Vec<_>>(),
			workspace,
			tenant: &index.tenant,
		},
		input.limit,
	)
	.await
	.map_err(|_| Error::SemanticUnavailable)?;
	let mut seen = std::collections::BTreeSet::new();
	for point in points {
		let Some((entry, text)) = allowed.get(&point.id) else {
			continue;
		};
		if !seen.insert(point.id)
			|| point.payload["entry_id"] != entry.id.to_string()
			|| point.payload["revision"] != entry.revision
			|| point.payload["index_revision"] != index.revision
			|| point.payload["tenant"] != index.tenant
			|| point.payload["workspace_id"] != workspace.to_string()
		{
			continue;
		}
		// Check again at delivery while the authority and source row leases are
		// still held. Backend payload never supplies content or permission.
		if !lease.permits(entry, "semantic.read").await?
			|| lease
				.source(workspace, &serde_json::from_value(entry.source.clone())?)
				.await?
				.as_ref() != Some(text)
		{
			continue;
		}
		let matched = Match {
			entry_id: entry.id,
			revision: entry.revision,
			source: provenance(&entry.source),
			agent: entry.agent.clone(),
			metadata: entry.metadata.clone(),
			text: text.clone(),
			score: point.score,
		};
		result.matches.push(matched);
		let tokens = result_tokens(&result)?;
		if tokens > input.max_tokens {
			result.matches.pop();
			result.truncated = true;
		} else {
			result.estimated_tokens = tokens;
		}
	}
	Ok(result)
}
fn result_tokens(result: &SearchResult) -> Result<usize> {
	// Allow for growth of the counter's own JSON representation.
	Ok(crate::context::estimated_tokens(&serde_json::to_string(result)?) + 16)
}
pub(crate) fn content_digest(text: &str) -> String {
	use sha2::{Digest, Sha256};
	format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn provenance(source: &Value) -> Value {
	if source["kind"] == "memory" {
		json!({"kind":"memory"})
	} else {
		source.clone()
	}
}
pub async fn history_list(store: &Store, actor: &Actor, workspace: Uuid) -> Result<Vec<History>> {
	let mut lease = Lease::begin(store, actor).await?;
	let result = async {
		lease.workspace(workspace, "semantic.read").await?;
		let mut visible = vec![];
		let mut cursor = i64::MAX;
		loop {
			let rows: Vec<History> = sqlx::query_as(
				&sea_orm::sea_query::Query::select()
					.column(sea_orm::sea_query::Asterisk)
					.from(sea_orm::sea_query::Alias::new("semantic_history"))
					.and_where(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new(
							"workspace_id",
						))
						.eq(sea_orm::sea_query::Expr::cust("$1")),
					)
					.and_where(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("sequence"))
							.lt(sea_orm::sea_query::Expr::cust("$2")),
					)
					.order_by(
						sea_orm::sea_query::Alias::new("sequence"),
						sea_orm::sea_query::Order::Desc,
					)
					.limit(200)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(workspace)
			.bind(cursor)
			.fetch_all(&mut **lease.tx())
			.await?;
			let exhausted = rows.len() < 200;
			for row in rows {
				cursor = row.sequence;
				if let Some(id) = row.entry_id {
					let entry: Entry = sqlx::query_as(
						&sea_orm::sea_query::Query::select()
							.expr(sea_orm::sea_query::SimpleExpr::from(
								sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
							))
							.from(sea_orm::sea_query::Alias::new("semantic_entries"))
							.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
							.to_string(sea_orm::sea_query::PostgresQueryBuilder),
					)
					.bind(id)
					.fetch_one(&mut **lease.tx())
					.await?;
					if !lease.permits(&entry, "semantic.read").await? {
						continue;
					}
				}
				visible.push(row);
				if visible.len() == 200 {
					break;
				}
			}
			if exhausted || visible.len() == 200 {
				break;
			}
		}
		Ok(visible)
	}
	.await;
	lease.finish(result).await
}

pub(crate) async fn context_in(
	store: &Store,
	lease: &mut Lease<'_>,
	run: &crate::domain::Run,
	query: &str,
	budget: usize,
) -> Result<Option<SearchResult>> {
	let configured: Option<Index> = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("semantic_indexes"))
			.and_where(sea_orm::sea_query::Expr::cust("workspace_id = $1"))
			.lock(sea_orm::sea_query::LockType::Share)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.workspace_id)
	.fetch_optional(&mut **lease.tx())
	.await?;
	let Some(configured) = configured else {
		return Ok(None);
	};
	let spec = configured.configuration()?;
	if !spec.enabled || !spec.auto_context {
		return Ok(None);
	}
	let minimum = SearchResult {
		workspace_id: run.workspace_id,
		index_revision: configured.revision,
		model: spec.embedding.model.clone(),
		model_version: spec.embedding.model_version.clone(),
		matches: vec![],
		estimated_tokens: 0,
		truncated: false,
	};
	if budget.min(spec.max_result_tokens) < result_tokens(&minimum)? {
		return Ok(None);
	}
	let mut query = query.to_owned();
	if query.len() > spec.max_input_bytes {
		let mut end = spec.max_input_bytes;
		while !query.is_char_boundary(end) {
			end -= 1;
		}
		query.truncate(end);
	}
	search_in(
		store,
		lease,
		run.workspace_id,
		Search {
			query,
			agent: Some(crate::domain::qualified_agent(
				&run.home_node,
				&run.agent_id,
				&run.agent_version,
			)),
			metadata: json!({}),
			limit: spec.max_results,
			max_tokens: budget.min(spec.max_result_tokens),
		},
		Some(run.id),
	)
	.await
	.map(Some)
}

impl Access {
	pub(crate) async fn semantic_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		let dependencies: Vec<(Uuid, i64)> = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("entry_id")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("revision")),
				))
				.from(sea_orm::sea_query::Alias::new("semantic_run_reads"))
				.and_where(sea_orm::sea_query::Expr::cust("run_id = $1"))
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("entry_id"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("revision"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run)
		.fetch_all(&mut **self.tx)
		.await?;
		let mut lease = Lease::Inherited(self);
		for (id, revision) in dependencies {
			let entry: Entry = sqlx::query_as(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
					))
					.from(sea_orm::sea_query::Alias::new("semantic_entries"))
					.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(id)
			.fetch_one(&mut **lease.tx())
			.await?;
			if entry.deleted
				|| entry.revision != revision
				|| !lease.permits(&entry, "semantic.read").await?
			{
				return Ok(false);
			}
			let Some(text) = lease
				.source(entry.workspace_id, &serde_json::from_value(entry.source)?)
				.await?
			else {
				return Ok(false);
			};
			let digest: Option<String> = sqlx::query_scalar(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new(
							"content_digest",
						)),
					))
					.from(sea_orm::sea_query::Alias::new("semantic_points"))
					.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(entry.point_id)
			.fetch_one(&mut **lease.tx())
			.await?;
			if digest.as_deref() != Some(&content_digest(&text)) {
				return Ok(false);
			}
		}
		Ok(true)
	}
}

pub(crate) async fn remember_in(
	store: &Store,
	lease: &mut Lease<'_>,
	run: &crate::domain::Run,
	data: &Value,
) -> Result<()> {
	let exists: bool = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust(
				"EXISTS(SELECT 1 FROM semantic_indexes WHERE workspace_id = $1)",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.workspace_id)
	.fetch_one(&mut **lease.tx())
	.await?;
	if exists {
		let agent =
			crate::domain::qualified_agent(&run.home_node, &run.agent_id, &run.agent_version);
		let key = format!("agent-memory:{}", content_digest(&agent));
		// Serialize revisions with the same index lock used by API mutations.
		index(lease.tx(), run.workspace_id, true).await?;
		let revision: Option<i64> = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("revision")),
				))
				.from(sea_orm::sea_query::Alias::new("semantic_entries"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"workspace_id = $1 AND key = $2",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.workspace_id)
		.bind(&key)
		.fetch_optional(&mut **lease.tx())
		.await?;
		let entry = put_in(
			lease,
			run.workspace_id,
			PutEntry {
				key,
				expected_revision: revision.unwrap_or(0),
				source: Source::Memory {
					text: serde_json::to_string(data)?,
				},
				agent: Some(agent),
				metadata: json!({"origin":"agent_memory"}),
			},
		)
		.await?;
		sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("semantic_agent_memory"))
				.columns([
					sea_orm::sea_query::Alias::new("entry_id"),
					sea_orm::sea_query::Alias::new("workspace_id"),
					sea_orm::sea_query::Alias::new("agent_id"),
					sea_orm::sea_query::Alias::new("agent_version"),
					sea_orm::sea_query::Alias::new("home_node"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("''"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::columns([sea_orm::sea_query::Alias::new(
						"entry_id",
					)])
					.do_nothing()
					.to_owned(),
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(entry.id)
		.bind(run.workspace_id)
		.bind(&run.agent_id)
		.bind(&run.agent_version)
		.execute(&mut **lease.tx())
		.await?;
	}
	store.remember_in(lease.tx(), run, data).await
}

/// Keep the authority lease through reservation, provider I/O and settlement.
pub(crate) async fn embed(
	store: &Store,
	lease: &mut Lease<'_>,
	workspace: Uuid,
	config: &EmbeddingConfig,
	text: &str,
	origin: crate::generation::embedding::Origin,
) -> Result<Vec<f32>> {
	let reservation = if let Some(access) = lease.access() {
		crate::generation::embedding::reserve(access, store, workspace, config, text, origin)
			.await?
	} else {
		None
	};
	let output = backend::embed(&store.semantic_client, config, text)
		.await
		.map_err(|_| Error::SemanticUnavailable)?;
	if let Some(reservation) = reservation {
		reservation.settle(output.tokens).await?;
	}
	Ok(output.vector)
}
