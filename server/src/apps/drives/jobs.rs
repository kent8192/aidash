//! Scratch Space publication plans and resumable, digest-addressed transfers.
use super::{repository as db, tools::MAINTENANCE, transfer};
use crate::{Error, Result, authorization::access::Access, store::Store};
use aidash_application::{
	drive::Change,
	ports::capabilities::{
		execution_pod::{
			ExecutionPodError, ExecutionPodErrorCode, FileManifestRequest, FileReadRequest,
			ManifestFile,
		},
		pod_operations::PodContext,
	},
	tenancy::TenancyScope,
};
use aidash_domain::{
	Run, RunMetadata,
	capabilities::files::FileSelector,
	drive::{self, IndexSelection, Publish, PublishBase},
	nodes::NodeName,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{StreamExt, stream::FuturesUnordered};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{Alias, Expr, ExprTrait, LockType, Order, Query, SimpleExpr};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
struct Plan {
	node: String,
	base: PublishBase,
	changes: Vec<Change>,
	files: Vec<ManifestFile>,
	requester: String,
}

#[derive(Debug, Deserialize)]
struct Job {
	id: Uuid,
	tenant: String,
	drive_id: Uuid,
	run_id: Uuid,
	task_id: Uuid,
	author: String,
	plan: Plan,
	state: String,
	result: Option<String>,
}

pub(super) async fn publish(
	store: &Store,
	mut access: Access,
	run: &Run,
	input: Publish,
	key: &str,
) -> Result<Value> {
	let tenant = access.identity.tenant.clone();
	let operation = Uuid::new_v5(&run.id, format!("drive_publish:{key}").as_bytes());
	let scope = TenancyScope::tenant(&tenant)?;
	let node = publishing_node(&mut access, &run.metadata()).await?;
	let (drive, grant) = db::drive(&mut access, &node, false).await?;
	if !grant.may_request_work {
		return access.finish(Err(Error::Forbidden)).await;
	}
	if let Some(existing) = load(access.tx.as_mut(), &tenant, operation).await? {
		let value = status_value(access.tx.as_mut(), &existing).await;
		return access.finish(value).await;
	}
	let requester = requester(access.tx.as_mut(), &tenant, run.task_id, &grant.person).await?;
	let context = pod(store, &mut access, &run.metadata()).await?;
	access.finish(Ok(())).await?;

	let selected = manifest(&context, &input.sources).await?;
	let to_index = match &input.index {
		IndexSelection::Selector(selector) => {
			manifest(&context, selector).await?.into_keys().collect()
		}
		IndexSelection::None(_) => BTreeSet::new(),
	};
	let destination = input.destination.trim_matches('/');
	if !destination.is_empty() {
		aidash_domain::registry::rules::validate_path(destination)?;
	}
	let private_destination = destination.starts_with("private/");
	let mut access = recheck(store, run).await?;
	let (fresh, grant) = db::drive(&mut access, &node, true).await?;
	if fresh.id != drive.id || !grant.may_request_work {
		return access.finish(Err(Error::Forbidden)).await;
	}
	if let Some(existing) = load(access.tx.as_mut(), &tenant, operation).await? {
		let value = status_value(access.tx.as_mut(), &existing).await;
		return access.finish(value).await;
	}
	let mut changes = Vec::new();
	let mut paths = BTreeSet::new();
	for file in selected.values() {
		let path = if destination.is_empty() {
			file.path.clone()
		} else {
			format!("{destination}/{}", file.path)
		};
		let path = drive::path(&path)?.to_owned();
		if !grant.authority().writes(&path)
			|| drive::private_owner(&path).is_some() && !private_destination
		{
			return access.finish(Err(Error::Forbidden)).await;
		}
		let size = i64::try_from(file.size)
			.map_err(|_| Error::Invalid("Drive file size is out of range".into()))?;
		let media_type = mime_guess::from_path(&path)
			.first_or_octet_stream()
			.to_string();
		paths.insert(path.clone());
		changes.push(Change::Put {
			path,
			digest: file.sha256.clone(),
			size,
			media_type,
			index: to_index.contains(&file.path),
		});
	}
	if input.sources.sync {
		let mut query = db::snapshot(&fresh, &tenant, fresh.head)?;
		db::readable(&mut query, &grant);
		for entry in db::rows::<db::Entry>(access.tx.as_mut(), &query).await? {
			let path = entry.file.path;
			let relative = if destination.is_empty() {
				Some(path.as_str())
			} else {
				path.strip_prefix(destination)
					.and_then(|path| path.strip_prefix('/'))
			};
			if !paths.contains(&path)
				&& (drive::private_owner(&path).is_none() || private_destination)
				&& grant.authority().writes(&path)
				&& relative
					.is_some_and(|path| input.sources.paths.iter().any(|root| under(path, root)))
			{
				changes.push(Change::Trash { path });
			}
		}
	}
	let plan = Plan {
		node: node.to_string(),
		base: normalize_base(input.base_version)?,
		changes,
		files: selected.into_values().collect(),
		requester,
	};
	let bytes = plan
		.files
		.iter()
		.map(|file| file.size)
		.fold(0_u64, u64::saturating_add);
	let estimated_seconds = bytes
		.div_ceil(50_000_000)
		.saturating_add((plan.files.len() as u64).div_ceil(1000));
	let result = db::insert(
		access.tx.as_mut(),
		"drive_jobs",
		[
			"id",
			"tenant",
			"drive_id",
			"run_id",
			"task_id",
			"author",
			"plan",
			"state",
			"created_at",
		],
		[
			operation.into(),
			tenant.clone().into(),
			drive.id.into(),
			run.id.into(),
			run.task_id.into(),
			grant.person.into(),
			serde_json::to_value(&plan)?.into(),
			"pending".into(),
			chrono::Utc::now().into(),
		],
	)
	.await;
	access.finish(result).await?;
	if estimated_seconds <= 60 {
		let mut tx = store.pool.begin(&scope).await?;
		let job = load(tx.as_mut(), &tenant, operation)
			.await?
			.ok_or_else(|| Error::NotFound("Drive job disappeared".into()))?;
		tx.commit().await?;
		if let Some(result) = process(store, job).await? {
			return Ok(serde_json::to_value(result)?);
		}
	}
	Ok(json!({"status":"pending","job_id":operation,"estimated_seconds":estimated_seconds}))
}

fn normalize_base(base: PublishBase) -> Result<PublishBase> {
	let PublishBase::Paths(paths) = base else {
		return Ok(base);
	};
	let mut normalized = BTreeMap::new();
	for (path, version) in paths {
		if normalized
			.insert(drive::path(&path)?.to_owned(), version)
			.is_some()
		{
			return Err(Error::Invalid("duplicate Drive base version path".into()));
		}
	}
	Ok(PublishBase::Paths(normalized))
}

fn under(path: &str, root: &str) -> bool {
	let root = root.trim_matches('/');
	root == "."
		|| root.is_empty()
		|| path == root
		|| path
			.strip_prefix(root)
			.is_some_and(|suffix| suffix.starts_with('/'))
}

async fn publishing_node(access: &mut Access, run: &RunMetadata) -> Result<NodeName> {
	#[derive(Deserialize)]
	struct Subnode {
		node: String,
		state: String,
		creator_run_id: Uuid,
	}
	#[derive(Deserialize)]
	struct Creator {
		root_subject: String,
	}
	let tenant = access.identity.tenant.clone();
	let query = db::select("subnodes", &tenant)
		.and_where(db::col("thread_id").eq(Expr::value(run.thread_id)))
		.lock(LockType::Share)
		.to_owned();
	let Some(row) = db::rows::<Subnode>(access.tx.as_mut(), &query).await?.pop() else {
		return Ok(run.node.clone());
	};
	let query = db::select("authorization_execution", &tenant)
		.and_where(db::col("run_id").eq(Expr::value(row.creator_run_id)))
		.to_owned();
	let creator = db::rows::<Creator>(access.tx.as_mut(), &query)
		.await?
		.pop()
		.ok_or(Error::Forbidden)?;
	if row.state != "active" || creator.root_subject != access.identity.subject {
		return Err(Error::Forbidden);
	}
	Ok(NodeName::parse(&row.node)?)
}

async fn requester(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	task: Uuid,
	person: &str,
) -> Result<String> {
	#[derive(Deserialize)]
	struct Parent {
		duty_id: Option<Uuid>,
	}
	#[derive(Deserialize)]
	struct Duty {
		requester: String,
	}
	let query = db::select("tasks", tenant)
		.and_where(db::col("id").eq(Expr::value(task)))
		.to_owned();
	let Some(duty) = db::rows::<Parent>(tx, &query)
		.await?
		.pop()
		.and_then(|row| row.duty_id)
	else {
		return Ok(person.to_owned());
	};
	let query = db::select("duties", tenant)
		.and_where(db::col("id").eq(Expr::value(duty)))
		.to_owned();
	Ok(db::rows::<Duty>(tx, &query)
		.await?
		.pop()
		.map(|row| row.requester)
		.unwrap_or_else(|| person.to_owned()))
}

async fn recheck(store: &Store, run: &Run) -> Result<Access> {
	crate::apps::identity::services::execution::access_for_run(store, &run.metadata(), false)
		.await?
		.ok_or(Error::Forbidden)
}

async fn pod(store: &Store, access: &mut Access, run: &RunMetadata) -> Result<PodContext> {
	crate::apps::execution::repositories::operations::pods::tool_context(store, access, run)
		.await?
		.ok_or_else(|| Error::Conflict("Drive publishing requires an Execution Pod".into()))
}

async fn manifest(
	context: &PodContext,
	selector: &FileSelector,
) -> Result<BTreeMap<String, ManifestFile>> {
	if selector.paths.is_empty() {
		return Err(Error::Invalid(
			"Drive selector must name one or more paths".into(),
		));
	}
	let mut files = BTreeMap::new();
	for root in &selector.paths {
		let root = if matches!(root.as_str(), "" | "/" | ".") {
			"."
		} else {
			root.trim_end_matches('/')
		};
		if root != "." {
			aidash_domain::registry::rules::validate_path(root)?;
		}
		let mut request = FileManifestRequest {
			scratch_space_id: context.scratch_space_id().to_owned(),
			root: root.into(),
			ignore_files: selector.ignore_files.clone(),
			include: selector.include.clone(),
			exclude: selector.exclude.clone(),
			max_files: 800,
			after: Some(String::new()),
			missing_ok: selector.sync,
		};
		loop {
			let result = match context.runtime.file_manifest(&request).await {
				Err(ExecutionPodError::Remote {
					code: ExecutionPodErrorCode::PodUnavailable,
					..
				}) => {
					aidash_application::capabilities::pod::ensure(context)
						.await
						.map_err(aidash_application::capabilities::pod::tool_error)?;
					context.runtime.file_manifest(&request).await
				}
				result => result,
			}
			.map_err(aidash_application::capabilities::pod::tool_error)?;
			let next = result.files.last().map(|file| file.path.clone());
			for file in result.files {
				files.insert(file.path.clone(), file);
			}
			if !result.truncated {
				break;
			}
			if next.is_none() || next == request.after {
				return Err(Error::External(
					"Execution Pod manifest cursor did not advance".into(),
				));
			}
			request.after = next;
		}
	}
	Ok(files)
}

async fn load(tx: &mut dyn TransactionExecutor, tenant: &str, id: Uuid) -> Result<Option<Job>> {
	let query = db::select("drive_jobs", tenant)
		.and_where(db::col("id").eq(Expr::value(id)))
		.to_owned();
	Ok(db::rows::<Job>(tx, &query).await?.pop())
}

pub(super) async fn status(access: &mut Access, run: &Run, id: Uuid) -> Result<Value> {
	let tenant = access.identity.tenant.clone();
	let job = load(access.tx.as_mut(), &tenant, id)
		.await?
		.ok_or_else(|| Error::NotFound("Drive publication job not found".into()))?;
	if job.task_id != run.task_id || job.author != access.identity.subject {
		return Err(Error::Forbidden);
	}
	let (drive, _) = db::drive(access, &NodeName::parse(&job.plan.node)?, false).await?;
	if drive.id != job.drive_id {
		return Err(Error::Forbidden);
	}
	status_value(access.tx.as_mut(), &job).await
}

async fn status_value(tx: &mut dyn TransactionExecutor, job: &Job) -> Result<Value> {
	if let Some(result) = &job.result {
		return Ok(serde_json::from_str(result)?);
	}
	let query = db::select("drive_job_blobs", &job.tenant)
		.and_where(db::col("job_id").eq(Expr::value(job.id)))
		.to_owned();
	let complete = db::rows::<Value>(tx, &query).await?.len();
	let total = job
		.plan
		.files
		.iter()
		.map(|file| &file.sha256)
		.collect::<BTreeSet<_>>()
		.len();
	Ok(json!({"status":job.state,"job_id":job.id,"copied":complete,"total":total}))
}

pub(super) async fn tick(store: &Store) -> Result<()> {
	let query = Query::select()
		.expr(Expr::cust("*"))
		.from(Alias::new("drive_jobs"))
		.and_where(db::col("state").eq(Expr::value("pending")))
		.order_by(Alias::new("created_at"), Order::Asc)
		.limit(8)
		.to_owned();
	let mut tx = store.pool.begin(&MAINTENANCE).await?;
	let jobs = db::rows::<Job>(tx.as_mut(), &query).await?;
	tx.commit().await?;
	for job in jobs {
		let id = job.id;
		let tenant = job.tenant.clone();
		if let Err(error) = process(store, job).await {
			if matches!(
				error,
				Error::Forbidden | Error::Conflict(_) | Error::Invalid(_) | Error::NotFound(_)
			) {
				mark_failed(store, &tenant, id, &error).await?;
			} else {
				tracing::warn!(job = %id, %error, "Drive publication will retry");
			}
		}
	}
	Ok(())
}

async fn mark_failed(store: &Store, tenant: &str, id: Uuid, error: &Error) -> Result<()> {
	let mut tx = store
		.control_pool
		.begin(&TenancyScope::tenant(tenant)?)
		.await?;
	if !claim(tx.as_mut(), id).await? {
		return Ok(());
	}
	let query = db::select("drive_jobs", tenant)
		.and_where(db::col("id").eq(Expr::value(id)))
		.and_where(db::col("state").eq(Expr::value("pending")))
		.lock(LockType::Update)
		.to_owned();
	if db::rows::<Job>(tx.as_mut(), &query).await?.is_empty() {
		return Ok(());
	}
	let result = json!({"status":"failed","job_id":id,"error":error.to_string()}).to_string();
	db::update(
		tx.as_mut(),
		"drive_jobs",
		tenant,
		id,
		[
			("state", Expr::value("failed").into()),
			("result", Expr::value(result).into()),
		],
	)
	.await?;
	db::delete(
		tx.as_mut(),
		"drive_job_blobs",
		tenant,
		db::col("job_id").eq(Expr::value(id)),
	)
	.await?;
	tx.commit().await?;
	Ok(())
}

async fn claim(tx: &mut dyn TransactionExecutor, job: Uuid) -> Result<bool> {
	#[derive(Deserialize)]
	struct Lock {
		acquired: bool,
	}
	let query = Query::select()
		.expr_as(
			SimpleExpr::CustomWithExpr(
				"pg_try_advisory_xact_lock(hashtextextended(?, 0))".into(),
				vec![Expr::value(format!("drive-job:{job}")).into()],
			),
			Alias::new("acquired"),
		)
		.to_owned();
	Ok(db::rows::<Lock>(tx, &query)
		.await?
		.iter()
		.any(|lock| lock.acquired))
}

fn process(
	store: &Store,
	job: Job,
) -> futures_util::future::BoxFuture<'_, Result<Option<db::Publication>>> {
	Box::pin(async move {
		let scope = TenancyScope::tenant(&job.tenant)?;
		let mut lock = store.control_pool.begin(&scope).await?;
		if !claim(lock.as_mut(), job.id).await? {
			return Ok(None);
		}
		let Some(job) = load(lock.as_mut(), &job.tenant, job.id)
			.await?
			.filter(|job| job.state == "pending")
		else {
			return Ok(None);
		};
		let run = store.run(&scope, job.run_id).await?;
		let mut access = recheck(store, &run).await?;
		let node = publishing_node(&mut access, &run.metadata()).await?;
		if node.as_str() != job.plan.node || access.identity.subject != job.author {
			return access.finish(Err(Error::Forbidden)).await;
		}
		let (drive, grant) = db::drive(&mut access, &node, false).await?;
		if drive.id != job.drive_id || !grant.may_request_work {
			return access.finish(Err(Error::Forbidden)).await;
		}
		let context = pod(store, &mut access, &run.metadata()).await?;
		access.finish(Ok(())).await?;
		let mut digests = BTreeSet::new();
		let files: Vec<_> = job
			.plan
			.files
			.iter()
			.filter(|file| digests.insert(file.sha256.clone()))
			.collect();
		// Each upload may also borrow a reservation and a BlobStore commit
		// connection. Keep the shared pool available for completion and cleanup.
		for batch in files.chunks(8) {
			let mut uploads = FuturesUnordered::new();
			for file in batch {
				uploads.push(stage(store, &scope, &job, &context, file));
			}
			while let Some(result) = uploads.next().await {
				result?;
			}
		}
		let mut access = recheck(store, &run).await?;
		let current_node = publishing_node(&mut access, &run.metadata()).await?;
		let (drive, grant) = db::drive(&mut access, &current_node, true).await?;
		let result = async {
			if drive.id != job.drive_id || grant.person != job.author {
				return Err(Error::Forbidden);
			}
			let result = db::publish(
				access.tx.as_mut(),
				&drive,
				&grant,
				&job.plan.base,
				job.plan.changes,
				db::Provenance {
					author: &job.author,
					requester: &job.plan.requester,
					task_id: Some(job.task_id),
					run_id: Some(job.run_id),
					operation_id: Some(job.id),
				},
			)
			.await?;
			db::update(
				access.tx.as_mut(),
				"drive_jobs",
				&job.tenant,
				job.id,
				[
					("state", Expr::value("finished").into()),
					(
						"result",
						Expr::value(serde_json::to_string(&result)?).into(),
					),
				],
			)
			.await?;
			db::delete(
				access.tx.as_mut(),
				"drive_job_blobs",
				&job.tenant,
				db::col("job_id").eq(Expr::value(job.id)),
			)
			.await?;
			Ok(Some(result))
		}
		.await;
		let result = access.finish(result).await?;
		lock.commit().await?;
		Ok(result)
	})
}

async fn stage(
	store: &Store,
	scope: &TenancyScope,
	job: &Job,
	context: &PodContext,
	file: &ManifestFile,
) -> Result<()> {
	let mut tx = store.scratch_upload_pool.begin(scope).await?;
	let query = db::select("drive_job_blobs", &job.tenant)
		.and_where(db::col("job_id").eq(Expr::value(job.id)))
		.and_where(db::col("digest").eq(Expr::value(&file.sha256)))
		.to_owned();
	if !db::rows::<Value>(tx.as_mut(), &query).await?.is_empty() {
		return Ok(());
	}
	let query = db::select("core_blobs", &job.tenant)
		.and_where(db::col("digest").eq(Expr::value(&file.sha256)))
		.lock(LockType::Share)
		.to_owned();
	let mut sealed = None;
	if db::rows::<Value>(tx.as_mut(), &query).await?.is_empty() {
		let mut pending = store
			.capabilities
			.begin_object_for(&mut tx, &job.tenant, None, "drive", file.size, true)
			.await?;
		let mut offset = 0;
		while offset < file.size {
			let request = FileReadRequest {
				scratch_space_id: context.scratch_space_id().to_owned(),
				path: file.path.clone(),
				offset: Some(offset),
				limit: transfer::PART_BYTES as u32,
			};
			let chunk = context
				.runtime
				.file_read(&request)
				.await
				.map_err(aidash_application::capabilities::pod::tool_error)?;
			let bytes = STANDARD
				.decode(chunk.data)
				.map_err(|_| Error::External("invalid Pod file data".into()))?;
			if bytes.is_empty() || chunk.size != file.size {
				return Err(Error::Conflict(format!(
					"Scratch Space file changed: {}",
					file.path
				)));
			}
			pending.write_block(&bytes).await?;
			offset += bytes.len() as u64;
		}
		sealed = Some(pending.seal(Some(&file.sha256)).await?);
	}
	db::insert(
		tx.as_mut(),
		"drive_job_blobs",
		["id", "tenant", "job_id", "digest"],
		[
			Uuid::new_v4().into(),
			job.tenant.clone().into(),
			job.id.into(),
			file.sha256.clone().into(),
		],
	)
	.await?;
	tx.commit().await?;
	drop(sealed);
	Ok(())
}
