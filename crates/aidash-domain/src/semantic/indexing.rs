//! Indexing decisions use current database authority and durable point identities.
use super::{EmbeddingConfig, Source, VectorConfig};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct IndexingPlan {
	pub collection: String,
	pub tenant: String,
	pub revision: i64,
	pub spec: Value,
}

/// Default limits for image ingestion in new and existing index configurations.
pub fn default_max_image_bytes() -> usize {
	8 * 1024 * 1024
}
pub fn default_max_images() -> usize {
	256
}

/// The reason an image cannot currently enter its Workspace index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedReason {
	Modality,
	TooLarge,
	Limit,
}
impl UnsupportedReason {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Modality => "modality",
			Self::TooLarge => "too_large",
			Self::Limit => "limit",
		}
	}
}

/// File publication changes carry only immutable pointers and metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageChange {
	Removed {
		file_id: Uuid,
		path: String,
	},
	Published {
		file_id: Uuid,
		path: String,
		media_type: String,
		size: u64,
	},
	/// A pre-feature File whose bytes must not be inspected without its owner.
	LegacyCandidate {
		file_id: Uuid,
		path: String,
		position: usize,
	},
	/// The owner-authorized signature check has finished for a candidate.
	LegacyResolved {
		file_id: Uuid,
		path: String,
		position: usize,
		media_type: Option<String>,
		size: u64,
	},
}

impl ImageChange {
	pub fn file_id(&self) -> Uuid {
		match self {
			Self::Removed { file_id, .. }
			| Self::Published { file_id, .. }
			| Self::LegacyCandidate { file_id, .. }
			| Self::LegacyResolved { file_id, .. } => *file_id,
		}
	}
	pub fn path(&self) -> &str {
		match self {
			Self::Removed { path, .. }
			| Self::Published { path, .. }
			| Self::LegacyCandidate { path, .. }
			| Self::LegacyResolved { path, .. } => path,
		}
	}
}

/// Retire old versions before assigning slots to new versions, in manifest order.
/// Unchanged images are intentionally absent, so deletion does not refill a slot.
pub fn image_changes(
	previous: &[crate::capabilities::operations::MountedFile],
	current: &[crate::capabilities::operations::MountedFile],
) -> Vec<ImageChange> {
	use crate::capabilities::operations::{FileScope, MountedFile};
	use std::collections::HashSet;
	fn image(file: &MountedFile) -> bool {
		matches!(file.scope, FileScope::Working | FileScope::Received)
			&& file.media_type.starts_with("image/")
	}
	fn identity(file: &MountedFile) -> (Uuid, &str, &str, u64, &str) {
		(
			file.file_id,
			&file.path,
			&file.media_type,
			file.size,
			&file.digest,
		)
	}
	let old: HashSet<_> = previous
		.iter()
		.filter(|file| image(file))
		.map(identity)
		.collect();
	let new: HashSet<_> = current
		.iter()
		.filter(|file| image(file))
		.map(identity)
		.collect();
	previous
		.iter()
		.filter(|file| {
			(image(file) && !new.contains(&identity(file)))
				|| (matches!(file.scope, FileScope::Working | FileScope::Received)
					&& file.media_type == "application/octet-stream"
					&& !current.iter().any(|next| identity(next) == identity(file)))
		})
		.map(|file| ImageChange::Removed {
			file_id: file.file_id,
			path: file.path.clone(),
		})
		.chain(
			current
				.iter()
				.filter(|file| image(file) && !old.contains(&identity(file)))
				.map(|file| ImageChange::Published {
					file_id: file.file_id,
					path: file.path.clone(),
					media_type: file.media_type.clone(),
					size: file.size,
				}),
		)
		.collect()
}

/// One current File version and its position in the Workspace image quota.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageSlot {
	pub area_id: Uuid,
	pub generation: i64,
	pub file_id: Uuid,
	pub path: String,
	pub media_type: String,
	pub size: u64,
	pub publication: i64,
	pub position: usize,
	pub assigned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyCandidate {
	pub area_id: Uuid,
	pub generation: i64,
	pub file_id: Uuid,
	pub path: String,
	pub position: usize,
	#[serde(default)]
	pub initially_limited: bool,
}

/// Quota decisions are durable; freeing one does not select a waiting image.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageSlots {
	pub entries: Vec<ImageSlot>,
	#[serde(default)]
	pub legacy_candidates: Vec<LegacyCandidate>,
	/// Last publication included in a requested rebuild, if it is queued.
	#[serde(default)]
	pub rebuild_through: Option<i64>,
}

impl ImageSlots {
	pub fn request_rebuild(&mut self, through: Option<i64>) {
		self.rebuild_through = through;
	}

	/// Reconsider quota exactly after the publication at the rebuild boundary.
	pub fn finish_publication(&mut self, sequence: i64, max: usize) -> bool {
		if self.rebuild_through == Some(sequence) {
			self.rebuild_through = None;
			self.rebuild(max);
			true
		} else {
			false
		}
	}
	pub fn assigned(&self, area: Uuid, file: Uuid) -> bool {
		self.entries
			.iter()
			.any(|slot| slot.area_id == area && slot.file_id == file && slot.assigned)
	}

	fn limit_candidates(&mut self, max: usize, reconsider: bool) {
		for candidate in &mut self.legacy_candidates {
			let rank = (0, candidate.area_id, candidate.position, candidate.file_id);
			let preceding = self
				.entries
				.iter()
				.filter(|slot| {
					slot.assigned
						&& (slot.publication, slot.area_id, slot.position, slot.file_id) < rank
				})
				.count();
			if reconsider {
				candidate.initially_limited = preceding >= max;
			} else if preceding >= max {
				candidate.initially_limited = true;
			}
		}
	}

	/// Apply a committed publication once, retaining its relative manifest order.
	pub fn apply(
		&mut self,
		area: Uuid,
		generation: i64,
		publication: i64,
		changes: &[ImageChange],
		max: usize,
	) {
		for (position, change) in changes.iter().enumerate() {
			let admits = !matches!(change, ImageChange::Removed { .. });
			match change {
				ImageChange::Removed { file_id, path } => {
					self.entries.retain(|slot| {
						slot.area_id != area || slot.file_id != *file_id || slot.path != *path
					});
					self.legacy_candidates.retain(|candidate| {
						candidate.area_id != area
							|| candidate.file_id != *file_id
							|| candidate.path != *path
					});
				}
				ImageChange::LegacyCandidate {
					file_id,
					path,
					position,
				} => {
					if !self.legacy_candidates.iter().any(|candidate| {
						candidate.area_id == area
							&& candidate.generation == generation
							&& candidate.file_id == *file_id
							&& candidate.path == *path
					}) {
						self.legacy_candidates.push(LegacyCandidate {
							area_id: area,
							generation,
							file_id: *file_id,
							path: path.clone(),
							position: *position,
							initially_limited: false,
						});
					}
				}
				ImageChange::LegacyResolved {
					file_id,
					path,
					position,
					media_type,
					size,
				} => {
					let known =
						self.legacy_candidates
							.iter()
							.find(|candidate| {
								candidate.area_id == area
									&& candidate.generation == generation
									&& candidate.file_id == *file_id
									&& candidate.path == *path && candidate.position == *position
							})
							.cloned();
					self.legacy_candidates.retain(|candidate| {
						!(candidate.area_id == area
							&& candidate.generation == generation
							&& candidate.file_id == *file_id
							&& candidate.path == *path)
					});
					if let Some(candidate) = known
						&& let Some(media_type) = media_type
					{
						let rank = (0, area, *position, *file_id);
						let count = self.entries.iter().filter(|slot| slot.assigned).count();
						let latest = self
							.entries
							.iter()
							.enumerate()
							.filter(|(_, slot)| slot.assigned)
							.max_by_key(|(_, slot)| {
								(slot.publication, slot.area_id, slot.position, slot.file_id)
							})
							.map(|(index, slot)| {
								(
									index,
									(slot.publication, slot.area_id, slot.position, slot.file_id),
								)
							});
						let assigned = if candidate.initially_limited {
							false
						} else if count < max {
							true
						} else if let Some((index, old_rank)) = latest
							&& rank < old_rank
						{
							self.entries[index].assigned = false;
							true
						} else {
							false
						};
						self.entries.push(ImageSlot {
							area_id: area,
							generation,
							file_id: *file_id,
							path: path.clone(),
							media_type: media_type.clone(),
							size: *size,
							publication: 0,
							position: *position,
							assigned,
						});
					}
				}
				ImageChange::Published {
					file_id,
					path,
					media_type,
					size,
				} => {
					if self.entries.iter().any(|slot| {
						slot.area_id == area
							&& slot.generation == generation
							&& slot.file_id == *file_id
							&& slot.path == *path
					}) {
						continue;
					}
					self.entries
						.retain(|slot| slot.area_id != area || slot.path != *path);
					let assigned = self.entries.iter().filter(|slot| slot.assigned).count() < max;
					self.entries.push(ImageSlot {
						area_id: area,
						generation,
						file_id: *file_id,
						path: path.clone(),
						media_type: media_type.clone(),
						size: *size,
						publication,
						position,
						assigned,
					});
				}
			}
			if admits {
				self.limit_candidates(max, false);
			}
		}
	}

	/// A rebuild is the explicit point at which waiting images are reconsidered.
	pub fn rebuild(&mut self, max: usize) {
		self.entries
			.sort_by_key(|slot| (slot.publication, slot.area_id, slot.position, slot.file_id));
		for (position, slot) in self.entries.iter_mut().enumerate() {
			slot.assigned = position < max;
		}
		self.limit_candidates(max, true);
	}
}

/// Decode the same persisted specification after the locked authority fence.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexingSpec {
	pub embedding: EmbeddingConfig,
	pub vector: VectorConfig,
	pub enabled: bool,
	pub auto_context: bool,
	pub max_sources: usize,
	pub max_results: usize,
	pub max_result_tokens: usize,
	pub max_input_bytes: usize,
	#[serde(default = "default_max_image_bytes")]
	pub max_image_bytes: usize,
	#[serde(default = "default_max_images")]
	pub max_images: usize,
}

#[derive(Debug, Clone)]
pub struct IndexingEntry {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub source: Value,
	pub revision: i64,
	pub point_id: Uuid,
	pub state: String,
	pub attempts: i32,
}

pub fn source(entry: &IndexingEntry) -> serde_json::Result<Source> {
	serde_json::from_value(entry.source.clone())
}

pub fn validate_text(text: &str, max: usize) -> Result<()> {
	if text.trim().is_empty() || text.len() > max {
		Err(Error::Invalid(
			"semantic text is empty or exceeds the configured byte limit".into(),
		))
	} else {
		Ok(())
	}
}

pub fn retry(attempts: i32) -> (i32, f64) {
	let attempts = attempts.saturating_add(1);
	let delay = 2_i64.pow(attempts.min(8) as u32).min(300);
	(attempts, delay as f64)
}

pub fn content_digest(text: &str) -> String {
	content_digest_bytes(text.as_bytes())
}

pub fn content_digest_bytes(bytes: &[u8]) -> String {
	use sha2::{Digest, Sha256};
	format!("{:x}", Sha256::digest(bytes))
}

pub fn needs_new_point(old: &Option<(Option<String>, bool)>, digest: &str) -> bool {
	old.as_ref().is_some_and(|(old_digest, retired)| {
		*retired || old_digest.as_deref().is_some_and(|old| old != digest)
	})
}

pub fn has_current_point(
	entry: &IndexingEntry,
	old: &Option<(Option<String>, bool)>,
	digest: &str,
) -> bool {
	entry.state == "READY"
		&& old
			.as_ref()
			.is_some_and(|(old_digest, retired)| old_digest.as_deref() == Some(digest) && !*retired)
}

#[cfg(test)]
mod tests;
