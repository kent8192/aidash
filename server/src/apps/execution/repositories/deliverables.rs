//! Outside Versions belong to the executing Task and survive worker restarts.

use super::store::Store;
use crate::{Error, Result, database::native};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::{Run, deliverable::OutsideVersion};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

const MAX_OUTSIDE_VERSIONS: usize = 64;
type Notes = BTreeMap<String, OutsideVersion>;

fn read_notes(row: &native::Row) -> Result<Notes> {
	Ok(row
		.try_get::<Option<Value>>("outside_versions")?
		.map(serde_json::from_value)
		.transpose()?
		.unwrap_or_default())
}

impl Store {
	pub(crate) async fn record_outside_version(
		&self,
		scope: &TenancyScope,
		run: &Run,
		key: &str,
		input: &OutsideVersion,
	) -> Result<()> {
		input.validate()?;
		let mut tx = self.pool.begin(scope).await?;
		let rows = native::query(
			&Query::select()
				.columns(["id", "outside_versions"].map(Alias::new))
				.from(Alias::new("task_executions"))
				.and_where(Expr::col("task_id").eq(Expr::value(run.task_id)))
				.order_by(Alias::new("id"), Order::Asc)
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&mut *tx)
		.await?;
		let mut total = 0;
		let mut current = None;
		for row in rows {
			let notes = read_notes(&row)?;
			total += notes.len();
			if row.try_get::<Uuid>("id")? == run.id {
				if let Some(previous) = notes.get(key) {
					return if previous == input {
						Ok(())
					} else {
						Err(Error::Conflict("Outside Version key reused".into()))
					};
				}
				current = Some(notes);
			}
		}
		if total >= MAX_OUTSIDE_VERSIONS {
			return Err(Error::Invalid(
				"A Task can record at most 64 Outside Versions".into(),
			));
		}
		let mut notes = current.ok_or(Error::Forbidden)?;
		notes.insert(key.to_owned(), input.clone());
		native::query(
			&Query::update()
				.table(Alias::new("task_executions"))
				.value_expr(
					Alias::new("outside_versions"),
					Expr::value(serde_json::to_value(notes)?),
				)
				.and_where(Expr::col("id").eq(Expr::value(run.id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		Ok(())
	}

	pub(crate) async fn outside_versions(
		&self,
		scope: &TenancyScope,
		task: Uuid,
	) -> Result<Vec<OutsideVersion>> {
		let rows = native::query(
			&Query::select()
				.column(Alias::new("outside_versions"))
				.from(Alias::new("task_executions"))
				.and_where(Expr::col("task_id").eq(Expr::value(task)))
				.order_by(Alias::new("id"), Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(self.pool.in_scope(scope))
		.await?;
		let mut versions = Vec::new();
		for row in rows {
			versions.extend(read_notes(&row)?.into_values());
		}
		Ok(versions)
	}
}

pub(crate) use crate::apps::drives::repository::Deliverables as DriveVersions;

pub(crate) struct Attachment {
	pub path: String,
	pub id: Uuid,
	pub filename: String,
	pub media_type: String,
	pub size: u64,
	pub digest: String,
	pub object: super::capability_objects::SealedObject,
}

pub(crate) async fn stage_attachments(
	store: &Store,
	tenant: &str,
	pod: &aidash_application::ports::capabilities::pod_operations::PodContext,
	message: Uuid,
	paths: &[String],
) -> Result<(Vec<Attachment>, native::Transaction)> {
	use aidash_application::ports::capabilities::execution_pod::{
		FileManifestRequest, FileReadRequest,
	};
	use aidash_domain::deliverable::{MAX_COMPLETION_ATTACHMENT_BYTES, MAX_COMPLETION_ATTACHMENTS};
	use base64::{Engine as _, engine::general_purpose::STANDARD};

	if paths.len() > MAX_COMPLETION_ATTACHMENTS {
		return Err(Error::Invalid(
			"A Completion Message can attach at most 256 files".into(),
		));
	}
	let _slot = store
		.scratch_upload_slots
		.acquire()
		.await
		.map_err(|error| Error::External(error.to_string()))?;
	let mut tx = store
		.scratch_upload_pool
		.begin(&TenancyScope::tenant(tenant)?)
		.await?;
	let mut staged = Vec::with_capacity(paths.len());
	let mut total = 0_u64;
	for path in paths {
		aidash_domain::registry::rules::validate_path(path)?;
		let manifest = pod
			.runtime
			.file_manifest(&FileManifestRequest {
				scratch_space_id: pod.scratch_space_id().to_owned(),
				root: path.clone(),
				ignore_files: Vec::new(),
				exclude: Vec::new(),
				include: Vec::new(),
				max_files: 1,
				after: None,
				missing_ok: false,
			})
			.await
			.map_err(|error| Error::External(error.to_string()))?;
		let file = manifest
			.files
			.into_iter()
			.find(|file| file.path == *path)
			.ok_or_else(|| Error::Invalid(format!("{path} is not a regular Scratch Space file")))?;
		total = total.saturating_add(file.size);
		if total > MAX_COMPLETION_ATTACHMENT_BYTES {
			return Err(Error::Invalid(
				"Completion Message attachments exceed 4 GiB".into(),
			));
		}
		let id = Uuid::new_v5(&message, path.as_bytes());
		let mut upload = store
			.capabilities
			.begin_object_for(&mut tx, tenant, None, "thread_attachment", file.size, false)
			.await?;
		let mut offset = 0;
		let mut media_type = "application/octet-stream";
		while offset < file.size {
			let chunk = pod
				.runtime
				.file_read(&FileReadRequest {
					scratch_space_id: pod.scratch_space_id().to_owned(),
					path: path.clone(),
					offset: Some(offset),
					limit: (file.size - offset).min(64 * 1024) as u32,
				})
				.await
				.map_err(|error| Error::External(error.to_string()))?;
			let bytes = STANDARD
				.decode(chunk.data)
				.map_err(|_| Error::External("Invalid Runner file encoding".into()))?;
			if bytes.is_empty()
				|| chunk.size != file.size
				|| bytes.len() as u64 > file.size - offset
			{
				return Err(Error::Conflict(format!(
					"{path} changed during attachment upload"
				)));
			}
			if offset == 0 {
				media_type = aidash_domain::provider::detect_image(&bytes).unwrap_or(media_type);
			}
			upload.write_block(&bytes).await?;
			offset += bytes.len() as u64;
		}
		staged.push(Attachment {
			path: path.clone(),
			id,
			filename: path.rsplit('/').next().unwrap_or(path).to_owned(),
			media_type: media_type.to_owned(),
			size: file.size,
			digest: file.sha256.clone(),
			object: upload.seal(Some(&file.sha256)).await?,
		});
	}
	Ok((staged, tx))
}
