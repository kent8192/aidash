//! Reference ports retain current authority transactions, object guards and unchanged queries.
use super::capability_records::{domain, native};
use crate::apps::execution::capabilities::{
	serializers::{
		contracts::{Area, FileEntry},
		references::{Chunk as NativeChunk, Reference as NativeReference, Upload as NativeUpload},
	},
	services::{
		objects::PendingObject,
		operations, python,
		records::{self, Record as NativeRecord},
		service, sessions,
	},
};
use crate::{
	Error as NativeError, Result as NativeResult,
	authorization::{access::Access, identity::SubjectIdentity},
	store::Store,
};
use aidash_application::{
	Error, Result,
	ports::capabilities::references::{Limits, Mounts, ReferenceRepository, ReferenceScope},
};
use aidash_domain::capabilities::{
	operations::MountedFile,
	records::Record,
	references::lifecycle::{Chunk, Reference, Upload},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::{Value, json};
use uuid::Uuid;
pub(crate) struct Repository<'a> {
	pub(crate) store: &'a Store,
}
pub(crate) enum Authority<'a> {
	Owned(Box<Access>),
	Borrowed(&'a mut Access),
}
impl Authority<'_> {
	fn get(&self) -> &Access {
		match self {
			Self::Owned(access) => access,
			Self::Borrowed(access) => access,
		}
	}
	fn get_mut(&mut self) -> &mut Access {
		match self {
			Self::Owned(access) => access,
			Self::Borrowed(access) => access,
		}
	}
}
pub(crate) struct Scope<'a> {
	pub(crate) store: Option<&'a Store>,
	pub(crate) authority: Authority<'a>,
	pub(crate) area: Option<&'a mut Area>,
	pub(crate) pending: Option<PendingObject>,
}
fn missing() -> Error {
	Error::External("reference repository scope invariant".into())
}
impl From<NativeUpload> for Upload {
	fn from(input: NativeUpload) -> Self {
		Self {
			idempotency_key: input.idempotency_key,
			name: input.name,
			media_type: input.media_type,
			size: input.size,
			digest: input.digest,
		}
	}
}
impl From<NativeChunk> for Chunk {
	fn from(input: NativeChunk) -> Self {
		Self {
			offset: input.offset,
			data: input.data,
		}
	}
}
impl From<Reference> for NativeReference {
	fn from(value: Reference) -> Self {
		Self {
			reference_id: value.reference_id,
			revision: value.revision,
			state: value.state,
			name: value.name,
			media_type: value.media_type,
			size: value.size,
			digest: value.digest,
			uploaded_bytes: value.uploaded_bytes,
			original: value.original.map(Into::into),
			extraction: value.extraction.map(Into::into),
			extraction_state: value.extraction_state,
		}
	}
}
#[async_trait]
impl ReferenceRepository for Repository<'_> {
	async fn snapshot(&self, id: Uuid) -> Result<Record> {
		let result: NativeResult<Record> = async {
			let snapshot: NativeRecord = {
				let query_bind_1 = id;
				crate::database::native::query_as(
					&sessions::select("core_records")
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
				.fetch_one(&self.store.pool)
				.await?
			};
			Ok(domain(snapshot))
		}
		.await;
		result.map_err(Into::into)
	}
	async fn begin(&self, record: &Record) -> Result<Box<dyn ReferenceScope + '_>> {
		let identity = SubjectIdentity {
			http_session: None,
			credential_id: serde_json::from_value(
				record.data["identity"]["credential_id"].clone(),
			)?,
			tenant: record.tenant.clone(),
			subject: record.owner.clone(),
		};
		let access = Access::begin(self.store, &identity).await?;
		Ok(Box::new(Scope {
			store: Some(self.store),
			authority: Authority::Owned(Box::new(access)),
			area: None,
			pending: None,
		}))
	}
	async fn active(&self, after: Uuid) -> Result<Vec<Uuid>> {
		let result: NativeResult<Vec<Uuid>> = async {
			let ids: Vec<Uuid> = {
				let query_bind_1 = after;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("core_records"))
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("reference")),
						)
						.and_where(
							Expr::col(Alias::new("state"))
								.eq(reinhardt::query::Expr::value("extracting")),
						)
						.and_where(
							Expr::col(Alias::new("id")).gt(Expr::value(query_bind_1.to_owned())),
						)
						.order_by(Alias::new("id"), Order::Asc)
						.limit(8)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_all(&self.store.pool)
				.await?
			};
			Ok(ids)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn receipts(&self, after: Uuid) -> Result<Vec<(Uuid, Value)>> {
		let result: NativeResult<Vec<(Uuid, Value)>> = async {
			let receipts: Vec<(Uuid, Value)> = {
				let query_bind_1 = after;
				crate::database::native::query_as(
					&Query::select()
						.columns(["id", "data"].map(Alias::new))
						.from(Alias::new("core_records"))
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("reference")),
						)
						.and_where(Expr::cust("data->'receipt_pending' = 'true'::jsonb"))
						.and_where(
							Expr::col(Alias::new("id")).gt(Expr::value(query_bind_1.to_owned())),
						)
						.order_by(Alias::new("id"), Order::Asc)
						.limit(8)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["id", "data"])
				.fetch_all(&self.store.pool)
				.await?
			};
			Ok(receipts)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn acknowledge(&self, id: Uuid) -> Result<()> {
		let result: NativeResult<()> = async {
			{
				let query_bind_1 = id;
				crate::database::native::query(
					&Query::update()
						.table(Alias::new("core_records"))
						.value_expr(
							Alias::new("data"),
							Expr::cust("jsonb_set(data,'{receipt_pending}','false'::jsonb)"),
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
				.execute(&self.store.pool)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
		use aidash_application::ports::capabilities::runner::RunnerTransport as _;
		crate::bootstrap::operation_runner(self.store)?
			.request(method, path, body.as_ref())
			.await
	}
}
#[async_trait]
impl ReferenceScope for Scope<'_> {
	fn principal(&self) -> &str {
		&self.authority.get().identity.subject
	}
	fn identity(&self) -> Value {
		let identity = &self.authority.get().identity;
		json!({"credential_id":identity.credential_id,"tenant":identity.tenant,"subject":identity.subject})
	}
	fn limits(&self) -> Result<Limits> {
		let profile = &self.store.ok_or_else(missing)?.capabilities.0;
		Ok(Limits {
			admission: profile.admission,
			reference_bytes: profile.limits.reference_bytes,
			reference_files: profile.limits.reference_files,
			reference_text_bytes: profile.limits.reference_text_bytes,
			reference_pages: profile.limits.reference_pages,
			working_bytes: profile.working_bytes,
			output_bytes: profile.output_bytes,
			operation_seconds: profile.operation_seconds,
			staging_seconds: profile.staging_seconds,
			runner_image: profile.runner.as_ref().map(|runner| runner.image.clone()),
		})
	}
	fn mounts(&self) -> Result<Mounts> {
		let area = self.area.as_deref().ok_or_else(missing)?;
		Ok(Mounts {
			manifest: area.manifest.clone(),
			constraints: area.constraints.clone(),
		})
	}
	fn set_constraints(&mut self, value: Value) -> Result<()> {
		self.area.as_deref_mut().ok_or_else(missing)?.constraints = value;
		Ok(())
	}
	fn set_manifest(&mut self, value: Value) -> Result<()> {
		self.area.as_deref_mut().ok_or_else(missing)?.manifest = value;
		Ok(())
	}
	async fn load(&mut self, id: Uuid) -> Result<Record> {
		records::get(self.authority.get_mut(), id, "reference")
			.await
			.map(domain)
			.map_err(Into::into)
	}
	async fn require(&mut self, id: Uuid, owner: &str, action: &str) -> Result<()> {
		let access = self.authority.get_mut();
		access
			.require(
				&access.resource("reference", id, json!({"owner":owner})),
				action,
			)
			.await
			.map_err(Into::into)
	}
	async fn require_upload(&mut self) -> Result<()> {
		let access = self.authority.get_mut();
		access
			.require(
				&access.resource("reference", "new", json!({})),
				"reference.upload",
			)
			.await
			.map_err(Into::into)
	}
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>> {
		sessions::cached(self.authority.get_mut(), key, digest)
			.await
			.map_err(Into::into)
	}
	async fn cache(&mut self, key: Uuid, digest: &str, value: &Value) -> Result<()> {
		sessions::cache(self.authority.get_mut(), key, digest, value)
			.await
			.map_err(Into::into)
	}
	async fn insert(
		&mut self,
		id: Uuid,
		state: &str,
		data: Value,
		expires: Option<DateTime<Utc>>,
	) -> Result<Record> {
		records::insert(
			self.authority.get_mut(),
			id,
			None,
			"reference",
			state,
			data,
			expires,
		)
		.await
		.map(domain)
		.map_err(Into::into)
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		let mut current = native(record);
		records::update(self.authority.get_mut(), &mut current).await?;
		record.revision = current.revision;
		Ok(())
	}
	async fn put(&mut self, kind: &str, bytes: &[u8]) -> Result<(Uuid, String)> {
		self.store
			.ok_or_else(missing)?
			.capabilities
			.put(self.authority.get_mut(), None, kind, bytes)
			.await
			.map_err(Into::into)
	}
	async fn read(&mut self, file: &MountedFile) -> Result<Vec<u8>> {
		self.store
			.ok_or_else(missing)?
			.capabilities
			.read(self.authority.get_mut(), &FileEntry::from(file.clone()))
			.await
			.map_err(Into::into)
	}
	async fn read_chunk(&mut self, file: &MountedFile, offset: u64) -> Result<Vec<u8>> {
		self.store
			.ok_or_else(missing)?
			.capabilities
			.read_chunk(
				self.authority.get_mut(),
				&FileEntry::from(file.clone()),
				offset,
			)
			.await
			.map_err(Into::into)
	}
	async fn begin_original(&mut self, size: u64) -> Result<()> {
		self.pending = Some(
			self.store
				.ok_or_else(missing)?
				.capabilities
				.begin_object(self.authority.get_mut(), None, "reference_original", size)
				.await?,
		);
		Ok(())
	}
	async fn write_original(&mut self, bytes: &[u8]) -> Result<()> {
		self.pending
			.as_mut()
			.ok_or_else(missing)?
			.write_block(bytes)
			.await
			.map_err(Into::into)
	}
	async fn finish_original(&mut self, digest: &str) -> Result<(Uuid, String)> {
		self.pending
			.take()
			.ok_or_else(missing)?
			.finish(self.authority.get_mut(), Some(digest))
			.await
			.map_err(Into::into)
	}
	async fn release_python(&mut self, reason: &str) -> Result<()> {
		python::release(
			self.store.ok_or_else(missing)?,
			self.authority.get_mut(),
			self.area.as_deref_mut().ok_or_else(missing)?,
			reason,
		)
		.await
		.map_err(Into::into)
	}
	async fn publish(&mut self) -> Result<()> {
		service::publish(
			self.store.ok_or_else(missing)?,
			self.authority.get_mut(),
			self.area.as_deref_mut().ok_or_else(missing)?,
		)
		.await
		.map_err(Into::into)
	}
	async fn verified_health(&mut self) -> Result<Value> {
		operations::verified_health(self.store.ok_or_else(missing)?, false)
			.await
			.map_err(Into::into)
	}
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
		use aidash_application::ports::capabilities::runner::RunnerTransport as _;
		crate::bootstrap::operation_runner(self.store.ok_or_else(missing)?)?
			.request(method, path, body.as_ref())
			.await
	}
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		let Scope { authority, .. } = *self;
		match authority {
			Authority::Owned(access) => (*access)
				.finish(result.map_err(NativeError::from))
				.await
				.map_err(Into::into),
			Authority::Borrowed(_) => Err(missing()),
		}
	}
}
