//! References published with the final message of a Task.

use crate::{Error, Result, goal::Judgment};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DriveVersionRef {
	pub drive_id: Uuid,
	pub version: i64,
	pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutsideVersion {
	pub system: String,
	pub item: String,
	pub version: String,
	pub url: Option<String>,
}

impl OutsideVersion {
	pub fn validate(&self) -> Result<()> {
		if [&self.system, &self.item, &self.version]
			.iter()
			.any(|value| value.trim().is_empty() || value.len() > 512)
			|| self.url.as_ref().is_some_and(|url| url.len() > 2048)
		{
			return Err(Error::Invalid(
				"Invalid Outside Version field length".into(),
			));
		}
		Ok(())
	}
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Deliverable {
	pub drive_versions: Vec<DriveVersionRef>,
	pub outside_versions: Vec<OutsideVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Completion {
	pub message_id: Uuid,
	pub text: String,
	pub verdict: Option<Judgment>,
	/// Thread Attachment IDs owned by this message.
	pub attachments: Vec<Uuid>,
	pub deliverable: Deliverable,
}

impl Completion {
	pub fn message_id(task: Uuid, attempt: &str) -> Uuid {
		Uuid::new_v5(&task, attempt.as_bytes())
	}

	pub fn new(task: Uuid, attempt: &str, text: String, verdict: Option<Judgment>) -> Self {
		Self {
			message_id: Self::message_id(task, attempt),
			text,
			verdict,
			attachments: Vec::new(),
			deliverable: Deliverable::default(),
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompletionDetails {
	pub task_id: Uuid,
	pub verdict: Option<Judgment>,
	pub attachments: Vec<Uuid>,
	pub deliverable: Deliverable,
}

pub const MAX_COMPLETION_ATTACHMENTS: usize = 256;
pub const MAX_COMPLETION_ATTACHMENT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// The working model may return this object instead of an unstructured final answer.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
	pub text: String,
	pub attachments: Vec<String>,
}

impl Answer {
	pub fn parse(text: &str) -> Result<Self> {
		let answer = serde_json::from_str::<Self>(text).unwrap_or_else(|_| Self {
			text: text.to_owned(),
			attachments: Vec::new(),
		});
		let mut unique = std::collections::BTreeSet::new();
		if answer.attachments.len() > MAX_COMPLETION_ATTACHMENTS {
			return Err(Error::Invalid(
				"A Completion Message can attach at most 256 files".into(),
			));
		}
		for path in &answer.attachments {
			crate::registry::rules::validate_path(path)?;
			if !unique.insert(path) {
				return Err(Error::Invalid(
					"Completion attachment paths must be unique".into(),
				));
			}
		}
		Ok(answer)
	}
}

impl crate::Run {
	pub fn completion_key(&self) -> String {
		match self.context.completion_attempt {
			0 => format!("{}:complete", self.id),
			attempt => format!("{}:complete:{attempt}", self.id),
		}
	}
}
