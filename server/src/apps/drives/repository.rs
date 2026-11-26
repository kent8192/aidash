//! Tenant-scoped Drive manifests. Mutations hold the Drive row through commit.
use crate::{Error, Result, authorization::access::Access};
use aidash_application::drive::{Change, PathState};
use aidash_domain::{
	drive::{self, Authority, File, PublishBase, Version, VersionRef},
	identity::tenant_administration::{NODE_ADMINISTER, NODE_USE},
	nodes::NodeName,
};
use chrono::{Duration, Utc};
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{QueryRow, execution::convert_values},
};
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, IntoValue, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SelectStatement, SimpleExpr, Value,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub(super) fn col(name: &str) -> Expr {
	Expr::col(Alias::new(name))
}

pub(super) async fn rows<T: DeserializeOwned>(
	tx: &mut dyn TransactionExecutor,
	query: &SelectStatement,
) -> Result<Vec<T>> {
	let (sql, values) = query.build(PostgresQueryBuilder);
	tx.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data).map_err(Into::into))
		.collect()
}

pub(super) async fn insert<const N: usize>(
	tx: &mut dyn TransactionExecutor,
	table: &str,
	columns: [&str; N],
	values: [Value; N],
) -> Result<()> {
	let (sql, values) = Query::insert()
		.into_table(Alias::new(table))
		.columns(columns.map(Alias::new))
		.values_panic(values)
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

pub(super) fn select(table: &str, tenant: &str) -> SelectStatement {
	Query::select()
		.expr(Expr::cust("*"))
		.from(Alias::new(table))
		.and_where(col("tenant").eq(Expr::value(tenant)))
		.to_owned()
}

pub(super) async fn update(
	tx: &mut dyn TransactionExecutor,
	table: &str,
	tenant: &str,
	id: Uuid,
	values: impl IntoIterator<Item = (&'static str, SimpleExpr)>,
) -> Result<()> {
	let mut query = Query::update();
	query
		.table(Alias::new(table))
		.and_where(col("tenant").eq(Expr::value(tenant)))
		.and_where(col("id").eq(Expr::value(id)));
	for (key, value) in values {
		query.value_expr(Alias::new(key), value);
	}
	let (sql, values) = query.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

pub(super) async fn delete(
	tx: &mut dyn TransactionExecutor,
	table: &str,
	tenant: &str,
	condition: SimpleExpr,
) -> Result<()> {
	let (sql, values) = Query::delete()
		.from_table(Alias::new(table))
		.and_where(col("tenant").eq(Expr::value(tenant)))
		.and_where(condition)
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Drive {
	pub id: Uuid,
	pub node: String,
	pub closed: bool,
	pub head: i64,
}

#[derive(Clone, Debug)]
pub(super) struct Grant {
	pub tenant: String,
	pub person: String,
	pub administrator: bool,
	pub may_request_work: bool,
}

impl Grant {
	pub fn authority(&self) -> Authority<'_> {
		Authority {
			person: &self.person,
			administrator: self.administrator,
			may_request_work: self.may_request_work,
		}
	}
}

pub(super) async fn grant(access: &mut Access, node: &NodeName) -> Result<Grant> {
	let resource = access.node_resource(node).await?;
	let administrator = access.decide(&resource, NODE_ADMINISTER).await?;
	let may_request_work = access.decide(&resource, NODE_USE).await?;
	Ok(Grant {
		tenant: access.identity.tenant.clone(),
		person: access.identity.subject.clone(),
		administrator,
		may_request_work,
	})
}

pub(super) async fn drive(
	access: &mut Access,
	node: &NodeName,
	write: bool,
) -> Result<(Drive, Grant)> {
	let grant = grant(access, node).await?;
	let drive = by_node(access.tx.as_mut(), &grant.tenant, node, write).await?;
	if !grant.authority().reads(drive.closed, "") {
		return Err(Error::Forbidden);
	}
	Ok((drive, grant))
}

pub(super) async fn by_node(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &NodeName,
	lock: bool,
) -> Result<Drive> {
	use super::models::DrivesRow;
	use reinhardt::db::orm::Model;
	let query = DrivesRow::objects()
		.filter(DrivesRow::field_tenant().eq(tenant.to_owned()))
		.filter(DrivesRow::field_node().eq(node.as_str().to_owned()));
	let rows = if lock {
		query.select_for_update().all_with_executor(tx).await
	} else {
		query.all_with_executor(tx).await
	};
	let row = rows
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
		.ok_or_else(|| Error::NotFound("Drive not found".into()))?;
	Ok(Drive {
		id: row.id,
		node: row.node,
		closed: row.closed,
		head: row.head,
	})
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Entry {
	pub id: Uuid,
	#[serde(flatten)]
	pub file: File,
	pub removed_in_version: Option<i64>,
	pub indexed: bool,
}

pub(super) fn snapshot(drive: &Drive, tenant: &str, version: i64) -> Result<SelectStatement> {
	if !(0..=drive.head).contains(&version) {
		return Err(Error::NotFound("Drive version not found".into()));
	}
	let mut query = select("drive_entries", tenant);
	query
		.and_where(col("drive_id").eq(Expr::value(drive.id)))
		.and_where(col("added_in_version").lte(Expr::value(version)))
		.cond_where(
			Condition::any()
				.add(col("removed_in_version").is_null())
				.add(col("removed_in_version").gt(Expr::value(version))),
		);
	Ok(query)
}

pub(super) fn readable(query: &mut SelectStatement, grant: &Grant) {
	if !grant.administrator {
		query.cond_where(Condition::any().add(col("path").not_like("private/%")).add(
			col("path").like(format!(
				"{}/%",
				escape_like(&format!("private/{}", grant.person))
			)),
		));
	}
}

pub(super) fn escape_like(path: &str) -> String {
	path.replace('\\', "\\\\")
		.replace('%', "\\%")
		.replace('_', "\\_")
}

pub(super) async fn file(
	access: &mut Access,
	drive: &Drive,
	grant: &Grant,
	path: &str,
	version: Option<i64>,
) -> Result<Entry> {
	let path = drive::path(path)?;
	if !grant.authority().reads(drive.closed, path) {
		return Err(Error::Forbidden);
	}
	let mut query = snapshot(drive, &grant.tenant, version.unwrap_or(drive.head))?;
	query.and_where(col("path").eq(Expr::value(path)));
	let entry = rows::<Entry>(access.tx.as_mut(), &query)
		.await?
		.pop()
		.ok_or_else(|| Error::NotFound("Drive file not found".into()))?;
	if !producer_readable(access, &grant.tenant, drive.id, entry.file.added_in_version).await? {
		return Err(Error::Forbidden);
	}
	Ok(entry)
}

pub(super) async fn versions(
	tx: &mut dyn TransactionExecutor,
	drive: &Drive,
	grant: &Grant,
	limit: u64,
) -> Result<Vec<Version>> {
	if !grant.administrator {
		// Joining through readable paths keeps private-only publications and
		// their author and Task identifiers out of this person's history.
		let mut entries = entries_query(drive, &grant.tenant);
		readable(&mut entries, grant);
		let entries = rows::<Entry>(tx, &entries).await?;
		let mut counts = BTreeMap::new();
		for entry in entries {
			*counts.entry(entry.file.added_in_version).or_insert(0_i64) += 1;
			if let Some(version) = entry.removed_in_version {
				*counts.entry(version).or_insert(0_i64) += 1;
			}
		}
		if counts.is_empty() {
			return Ok(vec![]);
		}
		let mut query = versions_query(drive, &grant.tenant, limit);
		query.and_where(col("version").is_in(counts.keys().rev().take(limit as usize).copied()));
		let mut versions = rows::<Version>(tx, &query).await?;
		for version in &mut versions {
			version.change_count = counts[&version.version];
		}
		return Ok(versions);
	}
	rows(tx, &versions_query(drive, &grant.tenant, limit)).await
}

fn versions_query(drive: &Drive, tenant: &str, limit: u64) -> SelectStatement {
	select("drive_versions", tenant)
		.and_where(col("drive_id").eq(Expr::value(drive.id)))
		.order_by(Alias::new("version"), Order::Desc)
		.limit(limit)
		.to_owned()
}

fn entries_query(drive: &Drive, tenant: &str) -> SelectStatement {
	select("drive_entries", tenant)
		.and_where(col("drive_id").eq(Expr::value(drive.id)))
		.to_owned()
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Publication {
	Published { version: VersionRef },
	Conflict { paths: Vec<String> },
}

#[derive(Clone, Copy)]
pub(super) struct Provenance<'a> {
	pub author: &'a str,
	pub requester: &'a str,
	pub task_id: Option<Uuid>,
	pub run_id: Option<Uuid>,
	pub operation_id: Option<Uuid>,
}

struct ManifestDelta {
	changes: Vec<Change>,
	current: BTreeMap<String, PathState>,
}

pub(super) async fn publish(
	tx: &mut dyn TransactionExecutor,
	drive: &Drive,
	grant: &Grant,
	base: &PublishBase,
	changes: Vec<Change>,
	provenance: Provenance<'_>,
) -> Result<Publication> {
	let mut paths = BTreeSet::new();
	for change in &changes {
		let path = change.path();
		if drive::path(path)? != path || !paths.insert(path) {
			return Err(Error::Invalid(
				"duplicate or noncanonical Drive path".into(),
			));
		}
		if !grant.authority().writes(path) {
			return Err(Error::Forbidden);
		}
	}
	let tenant = &grant.tenant;
	if let Some(operation) = provenance.operation_id {
		#[derive(Deserialize)]
		struct Receipt {
			version: i64,
		}
		let mut query = select("drive_versions", tenant);
		query
			.and_where(col("drive_id").eq(Expr::value(drive.id)))
			.and_where(col("operation_id").eq(Expr::value(operation)));
		if let Some(receipt) = rows::<Receipt>(tx, &query).await?.pop() {
			return Ok(Publication::Published {
				version: version_ref(tx, tenant, drive.id, receipt.version).await?,
			});
		}
	}
	let mut current = BTreeMap::new();
	let paths: Vec<_> = paths.into_iter().collect();
	for batch in paths.chunks(10_000) {
		let mut query = entries_query(drive, tenant);
		query
			.distinct_on([Alias::new("path")])
			.and_where(col("path").is_in(batch.iter().copied()))
			.order_by(Alias::new("path"), Order::Asc)
			.order_by(Alias::new("added_in_version"), Order::Desc);
		for entry in rows::<Entry>(tx, &query).await? {
			current.insert(
				entry.file.path.clone(),
				PathState {
					last_change: entry
						.removed_in_version
						.unwrap_or(entry.file.added_in_version),
					current: entry.removed_in_version.is_none().then_some(entry.file),
				},
			);
		}
	}
	let conflicts = aidash_application::drive::conflicts(drive.head, base, &current, &changes);
	if !conflicts.is_empty() {
		return Ok(Publication::Conflict { paths: conflicts });
	}
	let delta = ManifestDelta { changes, current };
	let version = write_manifest(tx, drive, tenant, delta, provenance).await?;
	Ok(Publication::Published { version })
}

async fn write_manifest(
	tx: &mut dyn TransactionExecutor,
	drive: &Drive,
	tenant: &str,
	delta: ManifestDelta,
	provenance: Provenance<'_>,
) -> Result<VersionRef> {
	let ManifestDelta { changes, current } = delta;
	let version = drive
		.head
		.checked_add(1)
		.ok_or_else(|| Error::Conflict("Drive version exhausted".into()))?;
	let mut removed = Vec::new();
	let mut changed = Vec::new();
	let mut puts = Vec::new();
	let mut trash = Vec::new();
	let mut needs_index = false;
	let now = Utc::now();
	for change in changes {
		let prior = current
			.get(change.path())
			.and_then(|state| state.current.as_ref());
		match change {
			Change::Put {
				path,
				digest,
				size,
				media_type,
				index,
			} => {
				// Leave the temporal entry in place for unchanged files unless
				// the requester explicitly asks for indexing in this publication.
				if !index
					&& prior
						.is_some_and(|file| file.digest == digest && file.media_type == media_type)
				{
					continue;
				}
				needs_index |= index;
				let id = Uuid::new_v4();
				changed.push(path.clone());
				if prior.is_some() {
					removed.push(path.clone());
				}
				puts.push([
					id.into(),
					tenant.to_owned().into(),
					drive.id.into(),
					path.into(),
					digest.into(),
					size.into(),
					media_type.into(),
					version.into(),
					index.into(),
				]);
			}
			Change::Trash { path } => {
				if prior.is_some() {
					removed.push(path.clone());
					changed.push(path.clone());
					trash.push(path);
				}
			}
		}
	}
	for batch in removed.chunks(10_000) {
		let (sql, values) = Query::update()
			.table(Alias::new("drive_entries"))
			.value(Alias::new("removed_in_version"), version)
			.and_where(col("tenant").eq(Expr::value(tenant)))
			.and_where(col("drive_id").eq(Expr::value(drive.id)))
			.and_where(col("removed_in_version").is_null())
			.and_where(col("path").is_in(batch.iter().map(String::as_str)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	let mut trash_rows = Vec::new();
	for batch in trash.chunks(10_000) {
		let mut query = entries_query(drive, tenant);
		query
			.and_where(col("removed_in_version").eq(Expr::value(version)))
			.and_where(col("path").is_in(batch.iter().map(String::as_str)));
		for entry in rows::<Entry>(tx, &query).await? {
			trash_rows.push([
				Uuid::new_v4().into(),
				tenant.to_owned().into(),
				drive.id.into(),
				entry.file.path.into(),
				entry.id.into(),
				version.into(),
				(now + Duration::days(drive::TRASH_DAYS)).into(),
			]);
		}
	}
	insert_batch(
		tx,
		"drive_entries",
		[
			"id",
			"tenant",
			"drive_id",
			"path",
			"digest",
			"size",
			"media_type",
			"added_in_version",
			"indexed",
		],
		puts,
	)
	.await?;
	insert_batch(
		tx,
		"drive_trash",
		[
			"id",
			"tenant",
			"drive_id",
			"path",
			"entry_id",
			"removed_in_version",
			"expires_at",
		],
		trash_rows,
	)
	.await?;
	insert(
		tx,
		"drive_versions",
		[
			"id",
			"tenant",
			"drive_id",
			"version",
			"author",
			"created_at",
			"change_count",
			"task_id",
			"run_id",
			"operation_id",
		],
		[
			Uuid::new_v4().into(),
			tenant.to_owned().into(),
			drive.id.into(),
			version.into(),
			provenance.author.to_owned().into(),
			now.into(),
			(changed.len() as i64).into(),
			provenance.task_id.into_value(),
			provenance.run_id.into_value(),
			provenance.operation_id.into_value(),
		],
	)
	.await?;
	if needs_index {
		insert(
			tx,
			"drive_index_requests",
			["id", "tenant", "drive_id", "version", "requester", "state"],
			[
				Uuid::new_v4().into(),
				tenant.to_owned().into(),
				drive.id.into(),
				version.into(),
				provenance.requester.to_owned().into(),
				"pending".into(),
			],
		)
		.await?;
	}
	update(
		tx,
		"drives",
		tenant,
		drive.id,
		[("head", Expr::value(version).into())],
	)
	.await?;
	Ok(VersionRef {
		drive_id: drive.id,
		version,
		paths: changed,
	})
}

async fn insert_batch<const N: usize>(
	tx: &mut dyn TransactionExecutor,
	table: &str,
	columns: [&str; N],
	rows: Vec<[Value; N]>,
) -> Result<()> {
	let mut rows = rows.into_iter().peekable();
	while rows.peek().is_some() {
		// PostgreSQL extended protocol has at most 65,535 parameters.
		let mut query = Query::insert();
		query
			.into_table(Alias::new(table))
			.columns(columns.map(Alias::new));
		for row in rows.by_ref().take(60_000 / N) {
			query.values_panic(row);
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	Ok(())
}

async fn version_ref(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
	version: i64,
) -> Result<VersionRef> {
	let mut query = select("drive_entries", tenant);
	query
		.and_where(col("drive_id").eq(Expr::value(id)))
		.cond_where(
			Condition::any()
				.add(col("added_in_version").eq(Expr::value(version)))
				.add(col("removed_in_version").eq(Expr::value(version))),
		);
	let paths = rows::<Entry>(tx, &query)
		.await?
		.into_iter()
		.map(|entry| entry.file.path)
		.collect::<BTreeSet<_>>()
		.into_iter()
		.collect();
	Ok(VersionRef {
		drive_id: id,
		version,
		paths,
	})
}

/// Read publications inside an existing Tenant transaction when completing a Task.
pub(crate) async fn published_versions(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	task: Uuid,
) -> Result<Vec<VersionRef>> {
	#[derive(Deserialize)]
	struct Published {
		drive_id: Uuid,
		version: i64,
	}
	let mut query = select("drive_versions", tenant);
	query
		.and_where(col("task_id").eq(Expr::value(task)))
		.order_by(Alias::new("created_at"), Order::Asc)
		.order_by(Alias::new("version"), Order::Asc);
	let mut publications = Vec::new();
	for row in rows::<Published>(tx, &query).await? {
		publications.push(version_ref(tx, tenant, row.drive_id, row.version).await?);
	}
	Ok(publications)
}

pub(super) async fn producer_readable(
	access: &mut Access,
	tenant: &str,
	drive: Uuid,
	version: i64,
) -> Result<bool> {
	#[derive(Deserialize)]
	struct Producer {
		run_id: Option<Uuid>,
	}
	let query = select("drive_versions", tenant)
		.and_where(col("drive_id").eq(Expr::value(drive)))
		.and_where(col("version").eq(Expr::value(version)))
		.to_owned();
	let row = rows::<Producer>(access.tx.as_mut(), &query).await?.pop();
	match row.and_then(|row| row.run_id) {
		Some(run) => access.run_reads_visible(run).await,
		None => Ok(true),
	}
}

/// The current reader's authority is checked again for every recorded entry.
pub(crate) async fn source_visible(access: &mut Access, file: Uuid) -> Result<bool> {
	#[derive(Deserialize)]
	struct Parent {
		drive_id: Uuid,
		path: String,
		added_in_version: i64,
	}
	let tenant = access.identity.tenant.clone();
	let query = select("drive_entries", &tenant)
		.and_where(col("id").eq(Expr::value(file)))
		.to_owned();
	let Some(entry) = rows::<Parent>(access.tx.as_mut(), &query).await?.pop() else {
		return Ok(false);
	};
	let query = select("drives", &tenant)
		.and_where(col("id").eq(Expr::value(entry.drive_id)))
		.to_owned();
	let Some(drive) = rows::<Drive>(access.tx.as_mut(), &query).await?.pop() else {
		return Ok(false);
	};
	let node = NodeName::parse(&drive.node)?;
	match grant(access, &node).await {
		Ok(grant) if grant.authority().reads(drive.closed, &entry.path) => {
			producer_readable(access, &tenant, drive.id, entry.added_in_version).await
		}
		Ok(_) => Ok(false),
		Err(Error::NotFound(_) | Error::Forbidden) => Ok(false),
		Err(error) => Err(error),
	}
}

pub(super) async fn record_reads(
	access: &mut Access,
	run: &aidash_domain::Run,
	entries: impl IntoIterator<Item = Uuid>,
) -> Result<()> {
	use reinhardt::query::OnConflict;
	let tenant = access.identity.tenant.clone();
	let mut entries = entries
		.into_iter()
		.collect::<BTreeSet<_>>()
		.into_iter()
		.peekable();
	while entries.peek().is_some() {
		let mut query = Query::insert();
		query
			.into_table(Alias::new("drive_run_reads"))
			.columns(["id", "tenant", "thread_id", "run_id", "entry_id"].map(Alias::new))
			.on_conflict(
				OnConflict::columns(["tenant", "run_id", "entry_id"].map(Alias::new))
					.do_nothing()
					.to_owned(),
			);
		for entry in entries.by_ref().take(1000) {
			query.values_panic::<_, Value>([
				Uuid::new_v4().into(),
				tenant.clone().into(),
				run.thread_id.into(),
				run.id.into(),
				entry.into(),
			]);
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		access
			.tx
			.as_mut()
			.execute(&sql, convert_values(values))
			.await?;
	}
	Ok(())
}

/// Construct with the caller's already admitted Tenant. Each read additionally
/// filters by that Tenant and the exact Task ID.
pub(crate) struct Deliverables<'a> {
	pub store: &'a crate::store::Store,
	pub tenant: &'a str,
}

#[async_trait::async_trait]
impl aidash_application::deliverables::DeliverablePort for Deliverables<'_> {
	async fn published_versions(
		&self,
		task: Uuid,
	) -> aidash_application::Result<Vec<aidash_application::deliverables::DriveVersionRef>> {
		let scope = aidash_application::tenancy::TenancyScope::tenant(self.tenant)?;
		Ok(self
			.store
			.pool
			.atomic(&scope, async |tx| {
				published_versions(tx, self.tenant, task).await
			})
			.await?
			.into_iter()
			.map(
				|version| aidash_application::deliverables::DriveVersionRef {
					drive_id: version.drive_id,
					version: version.version,
					paths: version.paths,
				},
			)
			.collect())
	}
}
