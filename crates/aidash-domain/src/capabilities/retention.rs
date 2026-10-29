//! A reference keeps a File version by hold; a Retained version is never a copy.
//!
//! The rules here decide which File versions a hold covers and how many bytes a
//! Working Area keeps beyond its current manifest. They are deliberately free of
//! storage, clocks and transactions so that reclamation and quota admission can
//! share them.
use crate::capabilities::operations::MountedFile;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

/// What refers to a File version, and therefore keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Holder {
	/// A thread message that cites the File version.
	Message { message_id: Uuid },
	/// A running Live preview that pinned the revision it mounts.
	LivePreview { live_preview_id: Uuid },
	/// A peer Deployment retaining on behalf of a delegated Run's subject.
	Federation {
		peer: String,
		reference_id: String,
		expires_at: DateTime<Utc>,
	},
}

impl Holder {
	/// The persisted holder kind.
	pub fn kind(&self) -> &'static str {
		match self {
			Self::Message { .. } => "message",
			Self::LivePreview { .. } => "live_preview",
			Self::Federation { .. } => "federation",
		}
	}

	/// The holder's identity inside its kind: a message, a Live preview or a
	/// peer's reference id. Two holds with the same identity are the same hold.
	pub fn id(&self) -> String {
		match self {
			Self::Message { message_id } => message_id.to_string(),
			Self::LivePreview { live_preview_id } => live_preview_id.to_string(),
			Self::Federation { reference_id, .. } => reference_id.clone(),
		}
	}

	/// The peer that asked for a Federation hold, if any.
	pub fn peer(&self) -> Option<&str> {
		match self {
			Self::Federation { peer, .. } => Some(peer),
			_ => None,
		}
	}

	/// When the hold stops keeping its File versions by itself.
	pub fn expires_at(&self) -> Option<DateTime<Utc>> {
		match self {
			Self::Federation { expires_at, .. } => Some(*expires_at),
			_ => None,
		}
	}

	/// Whether the hold has already lapsed at `now`. A released hold is deleted,
	/// not expired; only Federation holds expire on their own.
	pub fn expired(&self, now: DateTime<Utc>) -> bool {
		self.expires_at().is_some_and(|at| at <= now)
	}

	/// Whether `self` is the same holder as `other`, ignoring a Federation hold's
	/// expiring timestamp, which the caller may recompute on a retry.
	pub fn same(&self, other: &Self) -> bool {
		self.kind() == other.kind() && self.id() == other.id() && self.peer() == other.peer()
	}
}

/// One File version a hold keeps, with the metadata a preview or a download
/// needs after the version left the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedFile {
	pub file_id: Uuid,
	pub path: String,
	pub size: u64,
	pub media_type: String,
}

impl From<&MountedFile> for RetainedFile {
	fn from(file: &MountedFile) -> Self {
		Self {
			file_id: file.file_id,
			path: file.path.clone(),
			size: file.size,
			media_type: file.media_type.clone(),
		}
	}
}

/// A reference to one File version, or to the whole manifest of an HTML File
/// version at its revision, that keeps every File version it names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionHold {
	pub tenant: String,
	pub area_id: Uuid,
	/// The Working Area revision the held File versions belong to.
	pub revision: i64,
	pub holder: Holder,
	pub files: Vec<RetainedFile>,
}

impl RetentionHold {
	/// Whether this hold keeps `file_id`.
	pub fn covers(&self, file_id: Uuid) -> bool {
		self.files.iter().any(|file| file.file_id == file_id)
	}
}

/// Whether an exact revision of this File version carries a page whose relative
/// references must resolve inside that same revision.
pub fn is_html(media_type: &str, path: &str) -> bool {
	let media_type = media_type.split(';').next().unwrap_or_default().trim();
	if media_type.eq_ignore_ascii_case("text/html") {
		return true;
	}
	let path = path.to_ascii_lowercase();
	path.ends_with(".html") || path.ends_with(".htm")
}

/// The File versions a reference to `entry` retains: the version alone, or,
/// for HTML, every File version of the manifest at that revision so that the
/// page's relative references still resolve. `manifest` is the Working Area
/// manifest the entry belongs to.
pub fn retained_files(entry: &MountedFile, manifest: &[MountedFile]) -> Vec<RetainedFile> {
	if is_html(&entry.media_type, &entry.path) {
		manifest.iter().map(RetainedFile::from).collect()
	} else {
		vec![RetainedFile::from(entry)]
	}
}

/// Sum of the sizes of held File versions that have left the current manifest,
/// each object counted once across holds, ignoring holders that already lapsed.
///
/// Bytes of a version still in `current` are not retained bytes: the object is
/// the current one and its size already counts in the Working Area. They start
/// counting only when a later publication supersedes that object.
pub fn retained_bytes(holds: &[RetentionHold], current: &[MountedFile], now: DateTime<Utc>) -> u64 {
	let current: HashSet<Uuid> = current.iter().map(|file| file.file_id).collect();
	let mut counted = HashSet::new();
	let mut total = 0_u64;
	for hold in holds {
		if hold.holder.expired(now) {
			continue;
		}
		for file in &hold.files {
			if current.contains(&file.file_id) || !counted.insert(file.file_id) {
				continue;
			}
			total = total.saturating_add(file.size);
		}
	}
	total
}

/// Whether the Tenant may add retained bytes at `used_bytes`, after discounting
/// the superseded objects no hold covers: reclamation will free those.
pub fn permits_retention(used_bytes: u64, reclaimable_bytes: u64, limit: u64) -> bool {
	used_bytes.saturating_sub(reclaimable_bytes) <= limit
}

/// Whether a Working Area's manifest plus the bytes its holds keep beyond the
/// manifest still fit the Working Area's byte budget.
pub fn area_permits_retention(manifest_bytes: u64, retained_bytes: u64, limit: u64) -> bool {
	manifest_bytes.saturating_add(retained_bytes) <= limit
}

#[cfg(test)]
mod tests;
