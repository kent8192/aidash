//! Person-facing Drive browsing, lifecycle and multipart endpoints.
use super::{index, repository as db, transfer};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	federation::Federation,
	http::json::Json,
};
use aidash_application::drive::Change;
use aidash_domain::{
	drive::{self, BaseVersion, File, PublishBase, Version},
	nodes::NodeName,
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use reinhardt::query::{Alias, Expr, ExprTrait, LockType, Order};
use reinhardt::{Path, Query, Request, Response, get, http::ViewResult, post, put};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowseQuery {
	pub prefix: Option<String>,
	pub after: Option<String>,
	pub version: Option<i64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct Trash {
	pub id: Uuid,
	pub path: String,
	pub expires_at: DateTime<Utc>,
	#[serde(skip_serializing)]
	pub entry_id: Uuid,
}

#[derive(Serialize, JsonSchema)]
pub struct Browse {
	pub drive: db::Drive,
	pub files: Vec<File>,
	pub next_after: Option<String>,
	pub versions: Vec<Version>,
	pub trash: Vec<Trash>,
	pub can_write: bool,
	pub can_administer: bool,
	pub drive_bytes: u64,
	pub scratch_capacity_bytes: Option<u64>,
	pub scratch_bytes: Option<u64>,
	pub index_confirmations: Vec<index::Request>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Download {
	pub path: String,
	pub version: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mutation {
	Close {
		closed: bool,
	},
	Trash {
		path: String,
		base_version: BaseVersion,
	},
	Restore {
		id: Uuid,
		base_version: BaseVersion,
	},
	Move {
		destination: String,
		confirm: bool,
	},
	ConfirmIndex {
		id: Uuid,
	},
}

#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
pub enum MutationResult {
	Publication(db::Publication),
	Updated {
		updated: bool,
	},
	Moved {
		drive_id: Uuid,
		warning: &'static str,
	},
}

async fn subject(f: &Federation, actor: Actor) -> Result<Access> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	Access::begin(&f.store, &identity).await
}

#[get("/api/nodes/{node}/drive", name = "drive-browse", auth = "protected")]
pub async fn browse(
	#[inject] f: Federation,
	#[inject] actor: Actor,
	Path(node): Path<String>,
	Query(input): Query<BrowseQuery>,
) -> ViewResult<Response> {
	crate::http::json(browse_inner(&f, actor, node, input).await)
}

async fn browse_inner(
	f: &Federation,
	actor: Actor,
	node: String,
	input: BrowseQuery,
) -> Result<Browse> {
	let mut access = subject(f, actor).await?;
	let result = async {
		let (drive, grant) = db::drive(&mut access, &NodeName::parse(&node)?, false).await?;
		let mut query = db::snapshot(&drive, &grant.tenant, input.version.unwrap_or(drive.head))?;
		db::readable(&mut query, &grant);
		let mut meter = db::select("drive_entries", &grant.tenant);
		meter.and_where(db::col("drive_id").eq(Expr::value(drive.id)));
		db::readable(&mut meter, &grant);
		let mut digests = BTreeSet::new();
		let mut drive_bytes = 0_u64;
		for entry in db::rows::<db::Entry>(access.tx.as_mut(), &meter).await? {
			if digests.insert(entry.file.digest) {
				drive_bytes = drive_bytes.saturating_add(entry.file.size as u64);
			}
		}
		if let Some(prefix) = input.prefix {
			let prefix = prefix.trim_matches('/');
			if !prefix.is_empty() {
				aidash_domain::registry::rules::validate_path(prefix)?;
				query.and_where(db::col("path").like(format!("{}/%", db::escape_like(prefix))));
			}
		}
		if let Some(after) = input.after {
			query.and_where(db::col("path").gt(Expr::value(after)));
		}
		const PAGE: usize = 200;
		query
			.order_by(Alias::new("path"), Order::Asc)
			.limit((PAGE + 1) as u64);
		let mut files: Vec<_> = db::rows::<db::Entry>(access.tx.as_mut(), &query)
			.await?
			.into_iter()
			.map(|entry| entry.file)
			.collect();
		let next_after = if files.len() > PAGE {
			files.pop();
			files.last().map(|file| file.path.clone())
		} else {
			None
		};
		let versions = db::versions(access.tx.as_mut(), &drive, &grant, 100).await?;
		let mut trash = db::select("drive_trash", &grant.tenant);
		trash
			.and_where(db::col("drive_id").eq(Expr::value(drive.id)))
			.and_where(db::col("expires_at").gt(Expr::value(Utc::now())))
			.order_by(Alias::new("expires_at"), Order::Asc)
			.limit(200);
		db::readable(&mut trash, &grant);
		let trash = db::rows(access.tx.as_mut(), &trash).await?;
		let index_confirmations =
			index::confirmations(access.tx.as_mut(), drive.id, &grant).await?;
		let (scratch_capacity_bytes, scratch_spaces) =
			scratch_capacity(&mut access, &drive, &grant).await?;
		Ok((
			Browse {
				drive,
				files,
				next_after,
				versions,
				trash,
				drive_bytes,
				scratch_capacity_bytes,
				scratch_bytes: None,
				can_write: grant.may_request_work,
				can_administer: grant.administrator,
				index_confirmations,
			},
			scratch_spaces,
		))
	}
	.await;
	let (mut browse, spaces) = access.finish(result).await?;
	// Ask the Runner after releasing the authority transaction. Missing or
	// stopped Pods leave the live measurement unavailable.
	if !spaces.is_empty()
		&& let Ok(runner) = crate::bootstrap::operation_runner(&f.store)
	{
		use futures_util::{StreamExt, TryStreamExt, stream};
		browse.scratch_bytes =
			stream::iter(spaces.into_iter().map(|space| runner.scratch_bytes(space)))
				.buffer_unordered(4)
				.try_fold(0_u64, |sum, bytes| {
					std::future::ready(Ok(sum.saturating_add(bytes)))
				})
				.await
				.ok();
	}
	Ok(browse)
}

async fn scratch_capacity(
	access: &mut Access,
	drive: &db::Drive,
	grant: &db::Grant,
) -> Result<(Option<u64>, Vec<Uuid>)> {
	if !grant.may_request_work && !grant.administrator {
		return Ok((None, vec![]));
	}
	#[derive(Deserialize)]
	struct Space {
		id: Uuid,
	}
	#[derive(Deserialize)]
	struct Settings {
		settings: aidash_domain::scratch::ScratchSettings,
	}
	let query = db::select("scratch_spaces", &grant.tenant)
		.and_where(db::col("node").eq(Expr::value(&drive.node)))
		.and_where(db::col("state").eq(Expr::value("active")))
		.to_owned();
	let spaces = db::rows::<Space>(access.tx.as_mut(), &query).await?;
	if spaces.is_empty() {
		return Ok((Some(0), vec![]));
	}
	let spaces: Vec<_> = spaces.into_iter().map(|space| space.id).collect();
	let query = db::select("scratch_space_settings", &grant.tenant)
		.and_where(db::col("space_id").is_in(spaces.iter().copied()))
		.to_owned();
	let space = db::rows::<Settings>(access.tx.as_mut(), &query).await?;
	Ok((
		Some(space.into_iter().fold(0_u64, |bytes, row| {
			bytes.saturating_add(row.settings.capacity_bytes)
		})),
		spaces,
	))
}

#[get(
	"/api/nodes/{node}/drive/file",
	name = "drive-download",
	auth = "protected"
)]
pub async fn download(
	#[inject] f: Federation,
	#[inject] actor: Actor,
	Path(node): Path<String>,
	Query(input): Query<Download>,
) -> ViewResult<Response> {
	let result = async {
		let mut access = subject(&f, actor).await?;
		let result = async {
			let (drive, grant) = db::drive(&mut access, &NodeName::parse(&node)?, false).await?;
			let entry = db::file(&mut access, &drive, &grant, &input.path, input.version).await?;
			let reader = transfer::open(&f.store, &grant, &entry, None).await?;
			Ok((entry, reader))
		}
		.await;
		let (entry, reader) = access.finish(result).await?;
		let stream = futures_util::stream::try_unfold(reader, |mut reader| async move {
			let mut data = vec![0; 128 * 1024];
			let count = reader
				.read(&mut data)
				.await
				.map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
			data.truncate(count);
			Ok((count > 0).then(|| (Bytes::from(data), reader)))
		});
		let filename = entry.file.path.rsplit('/').next().unwrap_or("file");
		let filename: String = filename
			.bytes()
			.map(|byte| format!("%{byte:02X}"))
			.collect();
		Ok(Response::ok()
			.with_stream(stream)
			.with_header("Content-Type", "application/octet-stream")
			.with_header(
				"Content-Disposition",
				&format!("attachment; filename*=UTF-8''{filename}"),
			)
			.with_header("Content-Length", &entry.file.size.to_string())
			.with_header("Cache-Control", "private, no-store")
			.with_header("X-Content-Type-Options", "nosniff"))
	}
	.await;
	crate::http::response(result)
}

#[post("/api/nodes/{node}/drive", name = "drive-update", auth = "protected")]
pub async fn mutation(
	#[inject] f: Federation,
	#[inject] actor: Actor,
	Path(node): Path<String>,
	Json(input): Json<Mutation>,
) -> ViewResult<Response> {
	let result = async {
		let mut access = subject(&f, actor).await?;
		let result = mutate(&mut access, &NodeName::parse(&node)?, input).await;
		access.finish(result).await
	}
	.await;
	crate::http::json(result)
}

async fn mutate(access: &mut Access, node: &NodeName, input: Mutation) -> Result<MutationResult> {
	if let Mutation::Move {
		destination,
		confirm,
	} = input
	{
		return move_drive(access, node, &NodeName::parse(&destination)?, confirm).await;
	}
	let (drive, grant) = db::drive(access, node, true).await?;
	let (path, base_version, change, restored) = match input {
		Mutation::Close { closed } => {
			if !grant.administrator {
				return Err(Error::Forbidden);
			}
			db::update(
				access.tx.as_mut(),
				"drives",
				&grant.tenant,
				drive.id,
				[("closed", Expr::value(closed).into())],
			)
			.await?;
			return Ok(MutationResult::Updated { updated: true });
		}
		Mutation::ConfirmIndex { id } => {
			index::confirm(access.tx.as_mut(), drive.id, &grant, id).await?;
			return Ok(MutationResult::Updated { updated: true });
		}
		Mutation::Trash { path, base_version } => {
			let path = drive::path(&path)?.to_owned();
			db::file(access, &drive, &grant, &path, None).await?;
			(path.clone(), base_version, Change::Trash { path }, None)
		}
		Mutation::Restore { id, base_version } => {
			let mut query = db::select("drive_trash", &grant.tenant);
			query
				.and_where(db::col("drive_id").eq(Expr::value(drive.id)))
				.and_where(db::col("id").eq(Expr::value(id)))
				.and_where(db::col("expires_at").gt(Expr::value(Utc::now())));
			db::readable(&mut query, &grant);
			let trash = db::rows::<Trash>(access.tx.as_mut(), &query)
				.await?
				.pop()
				.ok_or_else(|| Error::NotFound("trash entry not found".into()))?;
			let query = db::select("drive_entries", &grant.tenant)
				.and_where(db::col("id").eq(Expr::value(trash.entry_id)))
				.to_owned();
			let entry = db::rows::<db::Entry>(access.tx.as_mut(), &query)
				.await?
				.pop()
				.ok_or_else(|| Error::NotFound("trash entry not found".into()))?;
			let File {
				path,
				digest,
				size,
				media_type,
				..
			} = entry.file;
			(
				path.clone(),
				base_version,
				Change::Put {
					path,
					digest,
					size,
					media_type,
					index: entry.indexed,
				},
				Some(id),
			)
		}
		Mutation::Move { .. } => unreachable!(),
	};
	let provenance = db::Provenance {
		author: &grant.person,
		requester: &grant.person,
		task_id: None,
		run_id: None,
		operation_id: None,
	};
	let result = db::publish(
		access.tx.as_mut(),
		&drive,
		&grant,
		&PublishBase::Paths(BTreeMap::from([(path, base_version)])),
		vec![change],
		provenance,
	)
	.await?;
	if let db::Publication::Published { .. } = &result
		&& let Some(id) = restored
	{
		db::delete(
			access.tx.as_mut(),
			"drive_trash",
			&grant.tenant,
			db::col("id").eq(Expr::value(id)),
		)
		.await?;
	}
	Ok(MutationResult::Publication(result))
}

async fn move_drive(
	access: &mut Access,
	source: &NodeName,
	destination: &NodeName,
	confirm: bool,
) -> Result<MutationResult> {
	use crate::apps::identity::{models::NodeDeletion, repositories::nodes};
	use aidash_domain::identity::tenant_administration::NODE_ADMINISTER;
	if !confirm || source == destination {
		return Err(Error::Invalid("confirm the Drive move to a different Node. Restoring the original Node will not bring the Drive back".into()));
	}
	let tenant = access.identity.tenant.clone();
	let node = nodes::get(access.tx.as_mut(), &tenant, source.as_str()).await?;
	let resource = access.resource(
		"node",
		source.as_str(),
		nodes::policy_attributes(&node.label, node.default),
	);
	access.require(&resource, NODE_ADMINISTER).await?;
	let target = access.node_resource(destination).await?;
	access.require(&target, NODE_ADMINISTER).await?;
	let deletion = db::select("node_deletions", &tenant)
		.and_where(db::col("node").eq(Expr::value(source.as_str())))
		.and_where(db::col("restored_at").is_null())
		.and_where(db::col("purged_at").is_null())
		.lock(LockType::Update)
		.to_owned();
	let deletion = db::rows::<NodeDeletion>(access.tx.as_mut(), &deletion)
		.await?
		.pop()
		.ok_or_else(|| Error::Conflict("Drive move requires an open Node deletion".into()))?;
	if deletion.purge_step.is_some() {
		return Err(Error::Conflict("Node purge has already started".into()));
	}
	let incoming = db::by_node(access.tx.as_mut(), &tenant, source, true).await?;
	let current = db::by_node(access.tx.as_mut(), &tenant, destination, true).await?;
	let uploads = db::select("drive_uploads", &tenant)
		.and_where(db::col("drive_id").eq(Expr::value(current.id)))
		.limit(1)
		.to_owned();
	if current.head != 0
		|| !db::rows::<serde_json::Value>(access.tx.as_mut(), &uploads)
			.await?
			.is_empty()
	{
		return Err(Error::Conflict(
			"destination Node Drive already contains work".into(),
		));
	}
	db::delete(
		access.tx.as_mut(),
		"drives",
		&tenant,
		db::col("id").eq(Expr::value(current.id)),
	)
	.await?;
	db::update(
		access.tx.as_mut(),
		"drives",
		&tenant,
		incoming.id,
		[("node", Expr::value(destination.as_str()).into())],
	)
	.await?;
	db::insert(
		access.tx.as_mut(),
		"drives",
		["id", "tenant", "node", "closed", "head"],
		[
			Uuid::new_v4().into(),
			tenant.into(),
			source.as_str().into(),
			incoming.closed.into(),
			0_i64.into(),
		],
	)
	.await?;
	Ok(MutationResult::Moved {
		drive_id: incoming.id,
		warning: "Restoring the original Node will not bring this Drive back.",
	})
}

#[post(
	"/api/nodes/{node}/drive/uploads",
	name = "drive-upload-start",
	auth = "protected"
)]
pub async fn upload_start(
	#[inject] f: Federation,
	#[inject] actor: Actor,
	Path(node): Path<String>,
	Json(spec): Json<transfer::UploadSpec>,
) -> ViewResult<Response> {
	let result = async {
		let mut access = subject(&f, actor).await?;
		let result = async {
			let (drive, grant) = db::drive(&mut access, &NodeName::parse(&node)?, true).await?;
			transfer::start(&mut access, &drive, &grant, spec).await
		}
		.await;
		access.finish(result).await
	}
	.await;
	crate::http::json(result)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PartQuery {
	pub number: i64,
}

#[put(
	"/api/nodes/{node}/drive/uploads/{id}/chunks",
	name = "drive-upload-part",
	auth = "protected"
)]
pub async fn upload_part(
	#[inject] f: Federation,
	#[inject] actor: Actor,
	Path((node, id)): Path<(String, Uuid)>,
	Query(part): Query<PartQuery>,
	request: Request,
) -> ViewResult<Response> {
	let result = async {
		let mut access = subject(&f, actor).await?;
		let (drive, grant) = db::drive(&mut access, &NodeName::parse(&node)?, false).await?;
		transfer::put_part(
			&f.store,
			access,
			&drive,
			&grant,
			id,
			part.number,
			request.body(),
		)
		.await
	}
	.await;
	crate::http::json(result)
}

#[post(
	"/api/nodes/{node}/drive/uploads/{id}/finish",
	name = "drive-upload-finish",
	auth = "protected"
)]
pub async fn upload_finish(
	#[inject] f: Federation,
	#[inject] actor: Actor,
	Path((node, id)): Path<(String, Uuid)>,
) -> ViewResult<Response> {
	let result = async {
		let mut access = subject(&f, actor).await?;
		let (drive, grant) = db::drive(&mut access, &NodeName::parse(&node)?, true).await?;
		transfer::finish(&f.store, access, &drive, &grant, id).await
	}
	.await;
	crate::http::json(result)
}

pub(crate) fn openapi(
	contracts: &mut crate::config::openapi::Contracts,
	document: &mut reinhardt::rest::openapi::OpenApiSchema,
) -> Result<()> {
	let json = "application/json";
	contracts.path(document, browse, &["String"])?;
	contracts.query::<_, BrowseQuery>(document, browse)?;
	contracts.response::<_, Browse>(document, browse, 200, json)?;
	contracts.path(document, download, &["String"])?;
	contracts.query::<_, Download>(document, download)?;
	contracts.binary_response(document, download, 200, "application/octet-stream")?;
	contracts.path(document, mutation, &["String"])?;
	contracts.request::<_, Mutation>(document, mutation)?;
	contracts.response::<_, MutationResult>(document, mutation, 200, json)?;
	contracts.path(document, upload_start, &["String"])?;
	contracts.request::<_, transfer::UploadSpec>(document, upload_start)?;
	contracts.response::<_, transfer::UploadReceipt>(document, upload_start, 200, json)?;
	contracts.path(document, upload_part, &["String", "Uuid"])?;
	contracts.query::<_, PartQuery>(document, upload_part)?;
	contracts.binary_request(document, upload_part)?;
	contracts.response::<_, transfer::UploadReceipt>(document, upload_part, 200, json)?;
	contracts.path(document, upload_finish, &["String", "Uuid"])?;
	contracts.response::<_, db::Publication>(document, upload_finish, 200, json)?;
	Ok(())
}
