//! Current File versions are disclosed only through their Area's read gate.
use crate::apps::execution::capabilities::{
	serializers::contracts::{FileEntry, FileScope},
	services::sessions,
};
use crate::{Error, Result, authorization::access::Access};
use aidash_domain::semantic::Source;
use uuid::Uuid;

pub(crate) async fn current(
	access: &mut Access,
	workspace: Uuid,
	source: &Source,
) -> Result<Option<FileEntry>> {
	let Source::WorkingImage {
		area_id,
		generation,
		file_id,
		path,
	} = source
	else {
		return Err(Error::Forbidden);
	};
	let area = match sessions::load(access, *area_id).await {
		Ok(area) => area,
		Err(Error::Forbidden | Error::NotFound(_)) => return Ok(None),
		Err(error) => return Err(error),
	};
	if area.workspace_id != workspace || area.generation != *generation {
		return Ok(None);
	}
	let files: Vec<FileEntry> = serde_json::from_value(area.manifest)?;
	Ok(files.into_iter().find(|file| {
		file.file_id == *file_id
			&& file.path == *path
			&& matches!(file.scope, FileScope::Working | FileScope::Received)
	}))
}
