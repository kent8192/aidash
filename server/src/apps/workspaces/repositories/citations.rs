//! Message citations retain their File versions and record what was cited.
use crate::apps::workspaces::services::access::Lease;
use crate::{Error, Result, authorization::access::Access, store::Store};
use aidash_application::capabilities::sessions;
use aidash_domain::capabilities::{operations::MountedFile, retention::Holder, sessions::Area};
use aidash_domain::workspaces::channels::{ChannelCitation, CitedFileVersion};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, IntoValue as _, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _,
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// A citation as stored at cite time.
struct CitationRow {
	message_id: Uuid,
	area_id: Uuid,
	file_id: Uuid,
	remote_run_id: Option<Uuid>,
	remote_expires_at: Option<chrono::DateTime<chrono::Utc>>,
	path: String,
	digest: String,
	size: i64,
	media_type: String,
	revision: i64,
}
crate::native_record!(CitationRow {
	message_id,
	area_id,
	file_id,
	remote_run_id,
	remote_expires_at,
	path,
	digest,
	size,
	media_type,
	revision
});

/// One File version identity, used to compare a retry with the stored citations.
struct Cited {
	area_id: Uuid,
	file_id: Uuid,
	remote_run_id: Option<Uuid>,
}
crate::native_record!(Cited {
	area_id,
	file_id,
	remote_run_id
});

struct NewCitation {
	message_id: Uuid,
	file_id: Uuid,
	remote_run_id: Option<Uuid>,
	remote_expires_at: Option<chrono::DateTime<chrono::Utc>>,
	tenant: String,
	area_id: Uuid,
	revision: i64,
	path: String,
	digest: String,
	size: u64,
	media_type: String,
}

fn manifest(area: &Area) -> Result<Vec<MountedFile>> {
	Ok(serde_json::from_value(area.manifest.clone())?)
}

pub(crate) async fn object_digest(
	access: &mut Access,
	tenant: &str,
	area: Uuid,
	file: Uuid,
) -> Result<String> {
	let digest: Option<String> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("digest"))
			.from(Alias::new("core_objects"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(file)))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.to_owned())))
			.and_where(Expr::col(Alias::new("area_id")).eq(Expr::value(area)))
			.and_where(Expr::col(Alias::new("kind")).is_in(["working", "superseded_working"]))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **access.tx)
	.await?;
	digest.ok_or_else(|| Error::Conflict("FILE_VERSION_UNAVAILABLE".into()))
}

/// Remote references hidden when a thread is removed, for best-effort release
/// after the local deletion transaction commits.
pub(crate) async fn remote_for_thread(
	access: &mut Access,
	workspace: Uuid,
	thread: Uuid,
) -> Result<Vec<(Uuid, Uuid, Uuid)>> {
	let root: Option<Uuid> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("root_message_id"))
			.from(Alias::new("channel_threads"))
			.and_where(Expr::col("id").eq(Expr::value(thread)))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **access.tx)
	.await?;
	let Some(root) = root else { return Ok(vec![]) };
	let mut ids: Vec<Uuid> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("message_id"))
			.from(Alias::new("channel_message_context"))
			.and_where(Expr::col("thread_id").eq(Expr::value(thread)))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&mut **access.tx)
	.await?;
	ids.push(root);
	let rows: Vec<(Uuid, Uuid, Uuid)> = crate::database::native::query_as(
		&Query::select()
			.column(Alias::new("message_id"))
			.column(Alias::new("remote_run_id"))
			.column(Alias::new("file_id"))
			.from(Alias::new("message_file_versions"))
			.and_where(Expr::col("remote_run_id").is_not_null())
			.and_where(Expr::col("message_id").is_in(ids.iter().copied().map(Expr::value)))
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["message_id", "remote_run_id", "file_id"])
	.fetch_all(&mut **access.tx)
	.await?;
	Ok(rows)
}

async fn existing(lease: &mut Lease, message: Uuid) -> Result<Vec<Cited>> {
	let tx = lease.tx();
	crate::database::native::query_as(
		&Query::select()
			.column(Alias::new("area_id"))
			.column(Alias::new("file_id"))
			.column(Alias::new("remote_run_id"))
			.from(Alias::new("message_file_versions"))
			.and_where(Expr::col(Alias::new("message_id")).eq(Expr::value(message)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *tx)
	.await
}

fn same_set(stored: &[Cited], requested: &[CitedFileVersion]) -> bool {
	stored.len() == requested.len()
		&& requested.iter().all(|citation| {
			stored.iter().any(|row| {
				row.area_id == citation.area_id
					&& row.file_id == citation.file_id
					&& row.remote_run_id == citation.run_id
			})
		})
}

/// Retain every cited version for the message and record the citation.
///
/// The author's download authority over each Working Area is checked in the
/// same transaction. A quota refusal reaches the caller as
/// `FILE_VERSION_QUOTA_EXCEEDED`, which fails the whole message write.
pub(crate) async fn record(
	store: &Store,
	federation: Option<&crate::federation::Federation>,
	lease: &mut Lease,
	message: Uuid,
	citations: &[CitedFileVersion],
) -> Result<()> {
	if citations.is_empty() {
		return Ok(());
	}
	let stored = existing(lease, message).await?;
	if !stored.is_empty() {
		return if same_set(&stored, citations) {
			Ok(())
		} else {
			Err(Error::Conflict(
				"message idempotency key reused for different File versions".into(),
			))
		};
	}
	let access = lease.access_mut();
	let mut rows = vec![];
	for citation in citations {
		if let Some(run_id) = citation.run_id {
			let f = federation.ok_or(Error::Forbidden)?;
			let retained =
				crate::apps::execution::capabilities::services::federation_previews::retain_remote(
					f,
					crate::authorization::identity::Actor::Subject(access.identity.clone()),
					aidash_domain::federation::preview::RetainRequest {
						run_id,
						area_id: citation.area_id,
						file_id: citation.file_id,
						message_id: message,
					},
				)
				.await?;
			rows.push(NewCitation {
				message_id: message,
				file_id: citation.file_id,
				remote_run_id: Some(run_id),
				remote_expires_at: Some(retained.expires_at),
				tenant: access.identity.tenant.clone(),
				area_id: citation.area_id,
				revision: retained.revision,
				path: retained.file.path,
				digest: retained.file.digest,
				size: retained.file.size,
				media_type: retained.file.media_type,
			});
			continue;
		}
		let area = {
			let mut scope = crate::bootstrap::session_scope(Some(store), access);
			sessions::load(&mut scope, citation.area_id).await?
		};
		let cited_path = manifest(&area)?
			.iter()
			.find(|file| file.file_id == citation.file_id)
			.map(|file| file.path.clone());
		let hold = crate::apps::execution::capabilities::services::retention::retain(
			store,
			access,
			citation.area_id,
			citation.file_id,
			Holder::Message {
				message_id: message,
			},
		)
		.await
		.map_err(|error| match error {
			Error::Conflict(message) if message.starts_with("QUOTA_EXCEEDED") => {
				Error::Conflict(format!(
					"FILE_VERSION_QUOTA_EXCEEDED: File version {} ({}) of Working Area {} cannot be retained; send the message without the citation",
					citation.file_id,
					cited_path.as_deref().unwrap_or("path unavailable"),
					citation.area_id,
				))
			}
			other => other,
		})?;
		let file = hold
			.files
			.iter()
			.find(|file| file.file_id == citation.file_id)
			.ok_or_else(|| Error::Conflict("FILE_VERSION_CHANGED".into()))?;
		let digest = match cited_path {
			Some(_) => manifest(&area)?
				.iter()
				.find(|mounted| mounted.file_id == citation.file_id)
				.map(|mounted| mounted.digest.clone())
				.ok_or_else(|| Error::Conflict("FILE_VERSION_CHANGED".into()))?,
			None => object_digest(access, &area.tenant, citation.area_id, citation.file_id).await?,
		};
		rows.push(NewCitation {
			message_id: message,
			file_id: citation.file_id,
			remote_run_id: None,
			remote_expires_at: None,
			tenant: area.tenant.clone(),
			area_id: citation.area_id,
			revision: hold.revision,
			path: file.path.clone(),
			digest,
			size: file.size,
			media_type: file.media_type.clone(),
		});
	}
	let tx = lease.tx();
	for row in rows {
		let id = Uuid::new_v4();
		crate::database::native::query(
			&Query::insert()
				.into_table(Alias::new("message_file_versions"))
				.columns(
					[
						"id",
						"message_id",
						"file_id",
						"remote_run_id",
						"remote_expires_at",
						"tenant",
						"area_id",
						"revision",
						"path",
						"digest",
						"size",
						"media_type",
						"created_at",
					]
					.map(Alias::new),
				)
				.values(vec![
					id.into(),
					row.message_id.into(),
					row.file_id.into(),
					row.remote_run_id.into_value(),
					row.remote_expires_at.into_value(),
					row.tenant.into(),
					row.area_id.into(),
					row.revision.into(),
					row.path.into(),
					row.digest.into(),
					i64::try_from(row.size)
						.map_err(|_| Error::Invalid("FILE_LIMIT".into()))?
						.into(),
					row.media_type.into(),
					chrono::Utc::now().into(),
				])
				.map_err(Error::Invalid)?
				.on_conflict(
					OnConflict::columns([Alias::new("message_id"), Alias::new("file_id")])
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
	}
	Ok(())
}

/// The citations of these messages, redacted for the current viewer.
pub(crate) async fn for_messages(
	store: &Store,
	federation: Option<&crate::federation::Federation>,
	lease: &mut Lease,
	ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<ChannelCitation>>> {
	if ids.is_empty() {
		return Ok(HashMap::new());
	}
	let rows: Vec<CitationRow> = {
		let tx = lease.tx();
		crate::database::native::query_as(
			&Query::select()
				.column(Alias::new("message_id"))
				.column(Alias::new("area_id"))
				.column(Alias::new("file_id"))
				.column(Alias::new("remote_run_id"))
				.column(Alias::new("remote_expires_at"))
				.column(Alias::new("path"))
				.column(Alias::new("digest"))
				.column(Alias::new("size"))
				.column(Alias::new("media_type"))
				.column(Alias::new("revision"))
				.from(Alias::new("message_file_versions"))
				.and_where(
					Expr::col(Alias::new("message_id"))
						.is_in(ids.iter().map(|id| Expr::value(*id))),
				)
				.order_by(Alias::new("message_id"), reinhardt::query::Order::Asc)
				.order_by(Alias::new("path"), reinhardt::query::Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&mut *tx)
		.await?
	};
	if rows.is_empty() {
		return Ok(HashMap::new());
	}
	let held = {
		let messages: Vec<String> = ids.iter().map(Uuid::to_string).collect();
		let tx = lease.tx();
		let rows: Vec<(Uuid, String)> = crate::database::native::query_as(
			&Query::select()
				.column((Alias::new("f"), Alias::new("file_id")))
				.column((Alias::new("h"), Alias::new("holder_id")))
				.from_as(Alias::new("core_retention_files"), Alias::new("f"))
				.join(
					reinhardt::query::JoinType::InnerJoin,
					reinhardt::query::TableRef::table_alias(
						Alias::new("core_retention_holds"),
						Alias::new("h"),
					),
					Expr::col((Alias::new("h"), Alias::new("id")))
						.equals((Alias::new("f"), Alias::new("hold_id"))),
				)
				.and_where(
					Expr::col((Alias::new("h"), Alias::new("holder_kind")))
						.eq(Expr::value("message")),
				)
				.and_where(
					Expr::col((Alias::new("h"), Alias::new("holder_id")))
						.is_in(messages.iter().cloned()),
				)
				.and_where(Expr::cust(
					"(h.expires_at IS NULL OR h.expires_at > CURRENT_TIMESTAMP)",
				))
				.to_string(PostgresQueryBuilder),
		)
		.columns(&["file_id", "holder_id"])
		.fetch_all(&mut *tx)
		.await?;
		rows.into_iter()
			.filter_map(|(file, holder)| holder.parse::<Uuid>().ok().map(|message| (message, file)))
			.collect::<HashSet<_>>()
	};
	let readable = {
		let areas: HashSet<Uuid> = rows
			.iter()
			.filter(|row| row.remote_run_id.is_none())
			.map(|row| row.area_id)
			.collect();
		let runs: HashSet<Uuid> = rows.iter().filter_map(|row| row.remote_run_id).collect();
		{
			let access = lease.access_mut();
			let mut readable = HashSet::new();
			for area in areas {
				let mut scope = crate::bootstrap::session_scope(Some(store), access);
				if sessions::load(&mut scope, area).await.is_ok() {
					readable.insert(area);
				}
			}
			for run_id in runs {
				if let Some(federation) = federation
						&& crate::apps::execution::capabilities::services::federation_previews::source_viewer(
							federation, access, run_id,
						)
						.await
						.is_ok()
					{
						readable.insert(run_id);
					}
			}
			readable
		}
	};
	let mut out: HashMap<Uuid, Vec<ChannelCitation>> = HashMap::new();
	for row in rows {
		let readable = row.remote_run_id.map_or_else(
			|| readable.contains(&row.area_id),
			|run| readable.contains(&run),
		);
		let available = readable
			&& if row.remote_run_id.is_some() {
				row.remote_expires_at
					.is_some_and(|expires| expires > chrono::Utc::now())
			} else {
				held.contains(&(row.message_id, row.file_id))
			};
		let citation = if readable {
			ChannelCitation {
				area_id: row.area_id,
				file_id: row.file_id,
				run_id: row.remote_run_id,
				path: row.path,
				digest: Some(row.digest),
				size: u64::try_from(row.size).ok(),
				media_type: Some(row.media_type),
				revision: Some(row.revision),
				available,
				reason: (!available).then(|| "released".into()),
			}
		} else {
			ChannelCitation {
				area_id: row.area_id,
				file_id: row.file_id,
				run_id: None,
				path: row.path,
				digest: None,
				size: None,
				media_type: None,
				revision: None,
				available: false,
				reason: Some("forbidden".into()),
			}
		};
		out.entry(row.message_id).or_default().push(citation);
	}
	Ok(out)
}
