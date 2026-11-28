//! Paths, authority and immutable version boundaries for Node Drives.
use crate::{Error, Result, capabilities::files::FileSelector, registry::rules::validate_path};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const TRASH_DAYS: i64 = 30;

pub fn path(value: &str) -> Result<&str> {
	let value = value.strip_prefix('/').unwrap_or(value);
	validate_path(value)?;
	if value
		.split('/')
		.any(|part| part.is_empty() || part == "." || part == "..")
		|| value.chars().any(char::is_control)
		|| value == "private"
		|| value.starts_with("private/") && value.split('/').count() < 3
	{
		return Err(Error::Invalid("invalid Drive file path".into()));
	}
	Ok(value)
}

pub fn private_owner(path: &str) -> Option<&str> {
	path.strip_prefix("private/")?.split('/').next()
}

#[derive(Debug, Clone, Copy)]
pub struct Authority<'a> {
	pub person: &'a str,
	pub administrator: bool,
	pub may_request_work: bool,
}

impl Authority<'_> {
	pub fn reads(self, closed: bool, path: &str) -> bool {
		(!closed || self.administrator || self.may_request_work)
			&& private_owner(path).is_none_or(|owner| self.administrator || self.person == owner)
	}

	pub fn writes(self, path: &str) -> bool {
		self.may_request_work
			&& private_owner(path).is_none_or(|owner| self.administrator || self.person == owner)
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "DriveNewFile")]
pub enum NewFile {
	New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "DriveBaseVersion")]
pub enum BaseVersion {
	Version(i64),
	New(NewFile),
}

impl BaseVersion {
	pub fn matches(self, latest: Option<i64>, present: bool) -> bool {
		match self {
			Self::New(_) => !present,
			Self::Version(base) => base > 0 && latest.is_some_and(|latest| latest <= base),
		}
	}
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum PublishBase {
	Paths(BTreeMap<String, BaseVersion>),
	New(NewFile),
}

impl PublishBase {
	pub fn for_path(&self, path: &str) -> Option<BaseVersion> {
		match self {
			Self::Paths(paths) => paths.get(path).copied(),
			Self::New(new) => Some(BaseVersion::New(*new)),
		}
	}
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoIndex {
	None,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum IndexSelection {
	Selector(FileSelector),
	None(NoIndex),
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Publish {
	pub sources: FileSelector,
	pub destination: String,
	pub base_version: PublishBase,
	pub index: IndexSelection,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "DriveVersionRef")]
pub struct VersionRef {
	pub drive_id: Uuid,
	pub version: i64,
	pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileLocation {
	pub node: String,
	pub path: String,
	pub version: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "DriveFile")]
pub struct File {
	pub path: String,
	pub digest: String,
	pub size: i64,
	pub media_type: String,
	pub added_in_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "DriveVersion")]
pub struct Version {
	pub version: i64,
	pub author: String,
	pub created_at: DateTime<Utc>,
	pub change_count: i64,
	pub task_id: Option<Uuid>,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn closed_drive_membership_does_not_grant_another_persons_private_folder() {
		let member = Authority {
			person: "alice",
			administrator: false,
			may_request_work: true,
		};
		assert!(member.reads(true, "reports/output.txt"));
		assert!(member.reads(true, "private/alice/output.txt"));
		assert!(!member.reads(true, "private/bob/output.txt"));
		assert!(!member.writes("private/bob/output.txt"));
		let reader = Authority {
			may_request_work: false,
			..member
		};
		assert!(reader.reads(false, "reports/output.txt"));
		assert!(!reader.reads(true, "reports/output.txt"));
		assert!(!reader.writes("reports/output.txt"));
	}

	#[test]
	fn an_unrelated_publication_does_not_conflict_but_deletion_does() {
		let base = BaseVersion::Version(4);
		assert!(base.matches(Some(3), true));
		assert!(!base.matches(Some(5), false));
		assert!(!BaseVersion::New(NewFile::New).matches(Some(5), true));
	}
}
