//! Completion Gate Checks (#112). A Check runs under the Run's bound shell
//! capability and current authority, on the exact files mounted in its
//! Working Area, and never publishes into that Area.
use crate::Result;
use aidash_domain::Run;
use async_trait::async_trait;
use uuid::Uuid;

/// The files mounted in a Run's Working Area, read for one gate decision.
pub struct MountedContent {
	/// [`aidash_domain::completion::content_digest`] of every mounted file.
	pub digest: String,
	/// The Area revision the digest was read at. A Check starts only at it.
	pub revision: i64,
	pub state: ContentState,
}

pub enum ContentState {
	/// Nothing is running in the Area; a Check can start.
	Ready,
	/// An operation is running in the Area; the candidate waits for it to end.
	Busy,
	/// The Area cannot run anything now, for example after an uncertain effect.
	Unavailable(String),
}

pub struct CheckRequest<'a> {
	/// Derived from the Check's subject and attempt, so a retried start after
	/// a process cut returns the operation the first start created.
	pub key: Uuid,
	pub command: &'a str,
	pub timeout_seconds: Option<u64>,
	/// The revision [`MountedContent`] was read at.
	pub revision: i64,
}

pub enum CheckStart {
	Started {
		operation_id: Uuid,
		/// The timeout the operation runs with, after node defaults and limits.
		timeout_seconds: u64,
	},
	/// The Check could not start; the reason is kept as its evidence.
	Unavailable(String),
	/// Authority or the capability's Binding refused the Check.
	Denied(String),
}

pub enum CheckStatus {
	Running,
	Finished {
		passed: bool,
		exit_code: Option<i64>,
		/// Tail of the Check's output.
		diagnostics: String,
	},
	/// The operation's outcome is unknown or was lost.
	Unavailable(String),
}

#[async_trait]
pub trait CompletionChecks: Send + Sync {
	/// `None` when the Run has no Working Area.
	async fn content(&self, run: &Run) -> Result<Option<MountedContent>>;
	async fn start(&self, run: &Run, request: &CheckRequest<'_>) -> Result<CheckStart>;
	async fn status(&self, run: &Run, operation: Uuid) -> Result<CheckStatus>;
}
