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
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt};
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) store: Option<&'a Store>,
	pub(crate) access: &'a mut Access,
	pub(crate) run: Option<&'a Run>,
	pub(crate) pending: Option<PendingObject>,
}
struct Reader(tokio::io::BufReader<tokio::fs::File>);
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
		let result: NativeResult<Vec<u8>> = async {
			self.0.seek(std::io::SeekFrom::Start(offset)).await?;
			let mut bytes = vec![];
			(&mut self.0).take(limit).read_to_end(&mut bytes).await?;
			Ok(bytes)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn fill_buf(&mut self) -> Result<&[u8]> {
		self.0
			.fill_buf()
			.await
			.map_err(NativeError::from)
			.map_err(Into::into)
	}
	fn consume(&mut self, count: usize) {
		self.0.consume(count);
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
		let registry = crate::registry::Registry::new(store.pool.clone(), &store.node_id)?;
		registry
			.get_for_run(
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
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let previous = {
				let query_bind_1 = area.id;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("manifest"))
						.from(Alias::new("core_areas"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&mut **access.tx)
				.await?
			};
			Ok(previous)
		}
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
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = area.id;
				let query_bind_2 = &area.manifest;
				let query_bind_3 = &area.constraints;
				let query_bind_4 = area.revision;
				crate::database::native::query(
					&Query::update()
						.table(Alias::new("core_areas"))
						.value_expr(
							Alias::new("manifest"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.value_expr(
							Alias::new("constraints"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							),
						)
						.value_expr(
							Alias::new("revision"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
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
	async fn open(&mut self, file: &MountedFile) -> Result<Box<dyn VerifiedFile>> {
		let row: FileEntry = file.clone().into();
		let file = self
			.store
			.ok_or_else(missing)?
			.capabilities
			.verified(self.access, &row)
			.await?;
		Ok(Box::new(Reader(tokio::io::BufReader::new(file))))
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
		let registry = crate::registry::Registry::new(store.pool.clone(), &store.node_id)?;
		crate::knowledge::load(&registry.db, entry)
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
}
