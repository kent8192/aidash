//! File adapters retain verified handles, pending-object guards and native publication statements.
use crate::apps::execution::capabilities::{
	serializers::contracts::{
		Area as NativeArea, FileEntry, FileScope as NativeFileScope,
		Materialize as NativeMaterialize, MaterializeSource as NativeMaterializeSource,
	},
	services::{
		approvals, objects::PendingObject, operations, packages, patch, python, sessions, sharing,
		skills,
	},
};
use crate::{
	Error as NativeError, Result as NativeResult,
	authorization::access::Access,
	domain::Run,
	registry::{EntityRef, Entry},
	store::Store,
};
use aidash_application::{
	Error, Result,
	ports::capabilities::files::{Action, FileScopePort, Limits, VerifiedFile},
};
use aidash_domain::capabilities::files::{Materialize, MaterializeSource};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		operations::{FileScope, MountedFile},
		sessions::Area,
	},
	policy::Resource,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, IntoValue as _, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) store: Option<&'a Store>,
	pub(crate) access: &'a mut Access,
	pub(crate) run: Option<&'a Run>,
	pub(crate) pending: Option<PendingObject>,
}
struct Reader {
	inner: tokio::io::BufReader<aidash_application::ports::blob_store::BlobReader>,
	store: std::sync::Arc<dyn aidash_application::ports::blob_store::BlobStore>,
	tenant: String,
	digest: aidash_domain::capabilities::Digest,
	size: u64,
}
/// Atomically retain one immutable revision and every logical object it names.
pub(crate) async fn record_revision(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	area: &Area,
	manifest: &Value,
) -> NativeResult<()> {
	let inserted = crate::database::native::query(
		&Query::insert()
			.into_table(Alias::new("core_manifest_revisions"))
			.columns(["tenant", "area_id", "revision", "manifest"].map(Alias::new))
			.values(vec![
				tenant.to_owned().into(),
				area.id.into(),
				area.revision.into(),
				manifest.clone().into_value(),
			])
			.map_err(NativeError::Invalid)?
			.on_conflict(
				OnConflict::columns(["tenant", "area_id", "revision"].map(Alias::new))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?
	.rows_affected();
	let stored: Value = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("manifest"))
			.from(Alias::new("core_manifest_revisions"))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
			.and_where(Expr::col(Alias::new("area_id")).eq(Expr::value(area.id)))
			.and_where(Expr::col(Alias::new("revision")).eq(Expr::value(area.revision)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **tx)
	.await?;
	if stored != *manifest {
		return Err(NativeError::Conflict("AREA_REVISION_CHANGED".into()));
	}
	// Revision and references are inserted in the same transaction. An existing
	// revision therefore already owns its references, including an empty one.
	if inserted == 0 {
		return Ok(());
	}
	let mut entries: Vec<FileEntry> = serde_json::from_value(manifest.clone())?;
	entries.sort_by(|a, b| a.digest.cmp(&b.digest).then(a.file_id.cmp(&b.file_id)));
	// Bound statement size independently of the maximum Working Area size.
	const REFERENCE_BATCH_SIZE: usize = 256;
	for batch in entries.chunks(REFERENCE_BATCH_SIZE) {
		let mut insert = Query::insert();
		insert
			.into_table(Alias::new("core_blob_references"))
			.columns(
				[
					"id",
					"tenant",
					"digest",
					"holder_kind",
					"holder_id",
					"object_id",
					"area_id",
					"revision",
				]
				.map(Alias::new),
			);
		for file in batch {
			let holder = format!("{}:{}:{}", area.id, area.revision, file.file_id);
			insert
				.values(vec![
					Uuid::new_v4().into(),
					tenant.to_owned().into(),
					file.digest.clone().into(),
					"revision".into(),
					holder.into(),
					file.file_id.into(),
					area.id.into(),
					area.revision.into(),
				])
				.map_err(NativeError::Invalid)?;
		}
		insert.on_conflict(
			OnConflict::columns(["tenant", "digest", "holder_kind", "holder_id"].map(Alias::new))
				.do_nothing()
				.to_owned(),
		);
		crate::database::native::query(&insert.to_string(PostgresQueryBuilder))
			.execute(&mut **tx)
			.await?;
	}
	Ok(())
}

fn missing() -> Error {
	Error::External("file repository scope invariant".into())
}
impl From<NativeMaterialize> for Materialize {
	fn from(v: NativeMaterialize) -> Self {
		Self {
			idempotency_key: v.idempotency_key,
			expected_revision: v.expected_revision,
			path: v.path,
			source: match v.source {
				NativeMaterializeSource::File {
					file_id,
					expected_digest,
				} => MaterializeSource::File {
					file_id,
					expected_digest,
				},
				NativeMaterializeSource::Message { message_id } => {
					MaterializeSource::Message { message_id }
				}
				NativeMaterializeSource::ReferenceText { index } => {
					MaterializeSource::ReferenceText { index }
				}
			},
		}
	}
}
#[async_trait]
impl VerifiedFile for Reader {
	async fn read_range(&mut self, offset: u64, limit: u64) -> Result<Vec<u8>> {
		if offset > self.size {
			return Err(Error::Invalid("INVALID_READ_RANGE".into()));
		}
		let end = offset.saturating_add(limit).min(self.size);
		if end == offset {
			return Ok(Vec::new());
		}
		let mut reader = self
			.store
			.get(&self.tenant, self.digest, Some(offset..end))
			.await?
			.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
		let mut bytes = vec![];
		reader
			.read_to_end(&mut bytes)
			.await
			.map_err(|e| Error::External(e.to_string()))?;
		if bytes.len() as u64 != end - offset {
			return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
		}
		Ok(bytes)
	}
	async fn fill_buf(&mut self) -> Result<&[u8]> {
		self.inner
			.fill_buf()
			.await
			.map_err(NativeError::from)
			.map_err(Into::into)
	}
	fn consume(&mut self, count: usize) {
		self.inner.consume(count);
	}
}
#[async_trait]
impl FileScopePort for Scope<'_> {
	fn local_node(&self) -> &str {
		&self.access.node_id
	}

	fn binding_snapshot(&self) -> Result<&aidash_domain::registry::bindings::BindingSnapshot> {
		self.run
			.and_then(|run| run.context.binding_snapshot.as_deref())
			.ok_or_else(|| Error::Invalid("Run has no admitted Binding snapshot".into()))
	}

	fn limits(&self) -> Result<Limits> {
		let p = &self.store.ok_or_else(missing)?.capabilities.0;
		Ok(Limits {
			working_bytes: p.working_bytes,
			read_bytes: p.limits.read_bytes,
			search_matches: p.limits.search_matches,
			search_bytes: p.limits.search_bytes,
			search_seconds: p.limits.search_seconds,
		})
	}
	async fn retained_bytes(&mut self, area: &Area) -> Result<u64> {
		super::retention::retained_bytes_of(&mut *self.access, area.id)
			.await
			.map_err(Into::into)
	}
	fn policy_revision(&self) -> i64 {
		self.access.snapshot.revision
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		crate::authorization::catalog::entry(self.access, reference, action)
			.await
			.map_err(Into::into)
	}
	async fn check_pinned(&mut self, entry: &Entry) -> Result<()> {
		crate::marketplace::check_pinned(self.access, entry)
			.await
			.map_err(Into::into)
	}
	async fn effective(&mut self, reference: &EntityRef) -> Result<Entry> {
		let store = self.store.ok_or_else(missing)?;
		let registry = crate::registry::Registry::new(store.pool.clone(), &store.deployment)?;
		let tenant = aidash_domain::tenancy::TenantName::new(self.access.identity.tenant.clone())?;
		registry
			.get_for_run(
				&tenant,
				self.run.ok_or_else(missing)?,
				&reference.id,
				&reference.version,
			)
			.await
			.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn serialize_sharing(&mut self) -> Result<()> {
		sharing::serialize(self.access).await.map_err(Into::into)
	}
	async fn for_run(&mut self, run: &RunMetadata) -> Result<Area> {
		aidash_application::capabilities::sessions::for_run(
			&mut crate::bootstrap::session_scope(None, self.access),
			run,
		)
		.await
	}
	async fn subnode_area(
		&mut self,
		run: &RunMetadata,
		workspace: Uuid,
		area: Uuid,
	) -> Result<Area> {
		use crate::apps::execution::models::subnodes::SubnodeRecord;
		use reinhardt::db::orm::Model;
		if self.access.read_run != Some(run.id) {
			return Err(Error::Forbidden);
		}
		let matches = SubnodeRecord::objects()
			.filter(SubnodeRecord::field_tenant().eq(self.access.identity.tenant.clone()))
			.filter(SubnodeRecord::field_workspace_id().eq(workspace))
			.filter(SubnodeRecord::field_creator_run_id().eq(run.id))
			.all_with_executor(self.access.tx.native_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(NativeError::from)?;
		if matches.is_empty() {
			return Err(Error::Forbidden);
		}
		// Narrow the inherited Workspace only for this read. Session authorization
		// still checks file.read and every source constraint against the creator.
		let previous = std::mem::replace(
			&mut self.access.context,
			serde_json::json!({"workspace_id": workspace}),
		);
		let result = aidash_application::capabilities::sessions::load(
			&mut crate::bootstrap::session_scope(None, self.access),
			area,
		)
		.await;
		self.access.context = previous;
		let area = result?;
		if area.workspace_id != workspace {
			return Err(Error::Forbidden);
		}
		Ok(area)
	}
	async fn authorize(&mut self, area: &Area, action: &str) -> Result<()> {
		aidash_application::capabilities::sessions::authorize(
			&mut crate::bootstrap::session_scope(None, self.access),
			area,
			action,
		)
		.await
	}
	async fn current_run(&mut self, area: &Area) -> Result<Option<Uuid>> {
		aidash_application::capabilities::sessions::status(
			&mut crate::bootstrap::session_scope(None, self.access),
			area,
		)
		.await
		.map(|s| s.active_run_id)
	}
	async fn require_current(&mut self, area: &Area, run: &RunMetadata) -> Result<()> {
		aidash_application::capabilities::sessions::require_current_run(
			&mut crate::bootstrap::session_scope(None, self.access),
			area,
			run,
		)
		.await
	}
	async fn execute(
		&mut self,
		action: Action<'_>,
		area: &mut Area,
		input: Value,
		key: &str,
	) -> Result<Value> {
		let store = self.store.ok_or_else(missing)?;
		let run = self.run.ok_or_else(missing)?;
		let mut native: NativeArea = area.clone().into();
		let result: NativeResult<Value> = async {
			Ok(match action {
				Action::Package => {
					packages::prepare(
						store,
						self.access,
						run,
						&mut native,
						serde_json::from_value(input)
							.map_err(|e| NativeError::Invalid(e.to_string()))?,
					)
					.await?
				}
				Action::Python => {
					python::prepare(
						store,
						self.access,
						run,
						&mut native,
						serde_json::from_value(input)
							.map_err(|e| NativeError::Invalid(e.to_string()))?,
						key,
					)
					.await?
				}
				Action::Outbound => {
					approvals::prepare(
						store,
						self.access,
						run,
						&native,
						serde_json::from_value(input)
							.map_err(|e| NativeError::Invalid(e.to_string()))?,
					)
					.await?
				}
				Action::Share => {
					sharing::share(
						store,
						self.access,
						run,
						&native,
						serde_json::from_value(input)
							.map_err(|e| NativeError::Invalid(e.to_string()))?,
					)
					.await?
				}
				Action::Skill(name) => skills::invoke(store, self.access, run, name, input).await?,
				Action::Patch => {
					patch::apply(
						store,
						self.access,
						run,
						&mut native,
						serde_json::from_value(input)
							.map_err(|e| NativeError::Invalid(e.to_string()))?,
					)
					.await?
				}
				Action::Shell => {
					operations::prepare(
						store,
						self.access,
						run,
						&mut native,
						serde_json::from_value(input)
							.map_err(|e| NativeError::Invalid(e.to_string()))?,
						key,
					)
					.await?
				}
				Action::Verify => {
					operations::prepare_kind(
						store,
						self.access,
						run,
						&mut native,
						serde_json::from_value(input)
							.map_err(|e| NativeError::Invalid(e.to_string()))?,
						aidash_domain::capabilities::operations::VERIFICATION,
						serde_json::json!({}),
					)
					.await?
				}
				Action::Control { kind, cancel } => {
					operations::poll(
						store,
						self.access,
						run,
						&native,
						serde_json::from_value(input)
							.map_err(|e| NativeError::Invalid(e.to_string()))?,
						kind,
						cancel,
					)
					.await?
				}
			})
		}
		.await;
		*area = native.into();
		result.map_err(Into::into)
	}
	async fn output(&mut self, area: &Area, id: Uuid) -> Result<Option<(String, i64, String)>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let row = {
				let query_bind_1 = id;
				let query_bind_2 = area.id;
				let query_bind_3 = &area.tenant;
				crate::database::native::query_as(
					&Query::select()
						.columns(["digest", "size", "kind"].map(Alias::new))
						.from(Alias::new("core_objects"))
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("area_id"))
								.eq(Expr::value(query_bind_2.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("tenant"))
								.eq(Expr::value(query_bind_3.to_owned())),
						)
						.and_where(Expr::col(Alias::new("kind")).is_in([
							"output",
							"display",
							"network_output",
						]))
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["digest", "size", "kind"])
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(row)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn previous_manifest(&mut self, area: &Area) -> Result<Value> {
		let result: NativeResult<Value> = async {
			let access = &mut *self.access;
			let previous: Option<Value> = crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("manifest"))
					.from(Alias::new("core_manifest_revisions"))
					.and_where(
						Expr::col(Alias::new("tenant")).eq(Expr::value(&access.identity.tenant)),
					)
					.and_where(Expr::col(Alias::new("area_id")).eq(Expr::value(area.id)))
					.and_where(Expr::col(Alias::new("revision")).eq(Expr::value(area.revision)))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **access.tx)
			.await?;
			let previous = previous.unwrap_or_else(|| serde_json::json!([]));
			let tenant = access.identity.tenant.clone();
			record_revision(access.tx.native_mut(), &tenant, area, &previous).await?;
			Ok(previous)
		}
		.await;
		result.map_err(Into::into)
	}

	async fn previous_generation(&mut self, area: &Area) -> Result<i64> {
		let result: NativeResult<i64> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("generation"))
				.from(Alias::new("core_areas"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(area.id)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **self.access.tx)
		.await;
		result.map_err(Into::into)
	}
	async fn supersede(&mut self, area: &Area, file: Uuid) -> Result<()> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = file;
				let query_bind_2 = area.id;
				crate::database::native::query(
					&Query::update()
						.table(Alias::new("core_objects"))
						.value(Alias::new("kind"), "superseded_working")
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("area_id"))
								.eq(Expr::value(query_bind_2.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("working")),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **access.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn persist_manifest(&mut self, area: &Area) -> Result<()> {
		let result: NativeResult<()> = async {
			let access = &mut *self.access;
			let tenant = access.identity.tenant.clone();
			record_revision(access.tx.native_mut(), &tenant, area, &area.manifest).await?;
			crate::database::native::query(
				&Query::update()
					.table(Alias::new("core_areas"))
					.value(Alias::new("constraints"), area.constraints.clone())
					.value(Alias::new("revision"), area.revision)
					.and_where(Expr::col(Alias::new("id")).eq(Expr::value(area.id)))
					.and_where(
						Expr::col(Alias::new("tenant")).eq(Expr::value(&access.identity.tenant)),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **access.tx)
			.await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}

	async fn unusable_references(
		&mut self,
		ids: &[Uuid],
	) -> Result<std::collections::BTreeSet<Uuid>> {
		crate::apps::execution::capabilities::services::records::unusable_references(
			self.access,
			ids,
		)
		.await
		.map_err(Into::into)
	}

	async fn open(&mut self, file: &MountedFile) -> Result<Box<dyn VerifiedFile>> {
		let row: FileEntry = file.clone().into();
		let file = self
			.store
			.ok_or_else(missing)?
			.capabilities
			.verified(self.access, &row)
			.await?;
		let store = self
			.store
			.ok_or_else(missing)?
			.capabilities
			.blob_store()
			.await?;
		let digest = row
			.digest
			.parse()
			.map_err(|_| Error::Conflict("OBJECT_INTEGRITY".into()))?;
		Ok(Box::new(Reader {
			inner: tokio::io::BufReader::new(file),
			store,
			tenant: self.access.identity.tenant.clone(),
			digest,
			size: row.size,
		}))
	}
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>> {
		sessions::cached(self.access, key, digest)
			.await
			.map_err(Into::into)
	}
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()> {
		sessions::cache(self.access, key, digest, result)
			.await
			.map_err(Into::into)
	}
	async fn begin_pending(&mut self, area: Uuid, size: u64) -> Result<()> {
		self.pending = Some(
			self.store
				.ok_or_else(missing)?
				.capabilities
				.begin_object(self.access, Some(area), "working", size)
				.await?,
		);
		Ok(())
	}
	async fn read_chunk(&mut self, file: &MountedFile, offset: u64) -> Result<Vec<u8>> {
		let row: FileEntry = file.clone().into();
		self.store
			.ok_or_else(missing)?
			.capabilities
			.read_chunk(self.access, &row, offset)
			.await
			.map_err(Into::into)
	}
	async fn write_pending(&mut self, bytes: &[u8]) -> Result<()> {
		self.pending
			.as_mut()
			.ok_or_else(missing)?
			.write_block(bytes)
			.await
			.map_err(Into::into)
	}
	async fn finish_pending(&mut self, expected: &str) -> Result<(Uuid, String)> {
		self.pending
			.take()
			.ok_or_else(missing)?
			.finish(self.access, Some(expected))
			.await
			.map_err(Into::into)
	}
	async fn message(&mut self, workspace: Uuid, id: Uuid) -> Result<Value> {
		self.access
			.workspace_record(workspace, "message", id)
			.await
			.map_err(Into::into)
	}
	async fn documents(&mut self, entry: &Entry) -> Result<Value> {
		let store = self.store.ok_or_else(missing)?;
		let registry = crate::registry::Registry::new(store.pool.clone(), &store.deployment)?;
		let tenant = aidash_domain::tenancy::TenantName::new(self.access.identity.tenant.clone())?;
		crate::knowledge::load(&registry, &tenant, entry)
			.await
			.map_err(Into::into)
	}
	async fn text_file(
		&mut self,
		area: Uuid,
		path: String,
		text: &str,
		scope: FileScope,
		provenance: Value,
	) -> Result<MountedFile> {
		let scope = match scope {
			FileScope::Working => NativeFileScope::Working,
			FileScope::References => NativeFileScope::References,
			FileScope::Received => NativeFileScope::Received,
		};
		self.store
			.ok_or_else(missing)?
			.capabilities
			.text_file(self.access, area, path, text, scope, provenance)
			.await
			.map(Into::into)
			.map_err(Into::into)
	}
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()> {
		self.store
			.ok_or_else(missing)?
			.event(&mut self.access.tx, Some(workspace), kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn image_publication(
		&mut self,
		area: &Area,
		changes: &[aidash_domain::semantic::indexing::ImageChange],
	) -> Result<()> {
		let access = &mut *self.access;
		if area.tenant != access.identity.tenant {
			return Err(Error::Forbidden);
		}
		crate::apps::knowledge::repositories::image_publications::lock_workspace(
			&mut **access.tx,
			area.workspace_id,
		)
		.await?;
		let authority = if access.identity.subject == area.owner {
			serde_json::json!({"credential":access.identity.credential_id,"tenant":access.identity.tenant,"subject":access.identity.subject,"subjects":access.subjects})
		} else {
			// Do not use another actor's credential to index the owner's Area.
			serde_json::json!({"credential":null,"tenant":area.tenant,"subject":area.owner,"subjects":[]})
		};
		let changes = serde_json::to_value(changes)?;
		// The Workspace lock assigns publication sequence in commit order.
		let statement = Query::insert()
			.into_table(Alias::new("semantic_image_publications"))
			.columns(
				[
					"tenant",
					"workspace_id",
					"area_id",
					"generation",
					"changes",
					"authority",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(area.tenant.clone()))
					.expr(Expr::value(area.workspace_id))
					.expr(Expr::value(area.id))
					.expr(Expr::value(area.generation))
					.expr(Expr::value(changes))
					.expr(Expr::value(authority))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&statement)
			.execute(&mut **access.tx)
			.await
			.map_err(Error::from)?;
		Ok(())
	}
}
