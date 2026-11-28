//! Read metadata for historical Working Area citations.
use crate::apps::workspaces::services::access::Lease;
use crate::{Error, Result, authorization::access::Access, store::Store};
use aidash_application::capabilities::sessions;
use aidash_domain::threads::channels::{ChannelCitation, CitedFileVersion};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// A citation as stored at cite time.
struct CitationRow {
	message_id: Uuid,
	area_id: Uuid,
	file_id: Uuid,
	remote_run_id: Option<Uuid>,
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
	path,
	digest,
	size,
	media_type,
	revision
});

/// Working Area file citations have been replaced by attachments and Drive links.
pub(crate) fn require_supported(citations: &[CitedFileVersion]) -> Result<()> {
	if !citations.is_empty() {
		return Err(Error::Invalid(
			"Working Area file citations are retired; use attachments or Drive links".into(),
		));
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
				available: false,
				reason: Some("released".into()),
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
