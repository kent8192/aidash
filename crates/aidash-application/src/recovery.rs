//! Failure classifications shared by worker ports and native adapters.
use crate::authorization::refusal_note::Note;
use aidash_domain::{
	context::recovery::Failure as ContextFailure,
	semantic::Failure as SemanticFailure,
	tool::{AuthorityRefusal, RefusalCheck},
};

/// Why a Run's authority stopped it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityFailure {
	/// The external identity provider's status could not be read.
	IdentityUnavailable,
	/// Authority was denied. The refusal names the tool (or stage) and the check; `None` is a
	/// refusal nothing has named yet, which [`AuthorityFailure::named`] fills in where the Run's
	/// state is known.
	Denied(Option<AuthorityRefusal>),
}

impl AuthorityFailure {
	/// The sentence a paused Run shows as its `error`.
	pub fn reason(&self) -> String {
		match self {
			Self::IdentityUnavailable => "identity status unavailable".into(),
			Self::Denied(Some(refusal)) => refusal.to_string(),
			Self::Denied(None) => "an authority check refused this step".into(),
		}
	}

	/// Name an unnamed refusal after `stage` and the check recorded in `note`; without a note the
	/// check is [`RefusalCheck::Unnamed`]. Every other failure passes through.
	pub fn named(self, stage: &str, note: Option<Note>) -> Self {
		match self {
			Self::Denied(None) => {
				let note = note.unwrap_or_else(|| Note::new(RefusalCheck::Unnamed, None));
				Self::Denied(Some(AuthorityRefusal {
					tool: stage.to_owned(),
					check: note.check,
					action: note.action,
					subject: note.subject,
					call: None,
				}))
			}
			other => other,
		}
	}

	pub fn refusal(&self) -> Option<&AuthorityRefusal> {
		match self {
			Self::Denied(refusal) => refusal.as_ref(),
			Self::IdentityUnavailable => None,
		}
	}
}

/// Adapter classification retains transport details outside the use case.
#[derive(Debug)]
pub enum ExecutionFailure {
	Semantic(SemanticFailure),
	/// A typed context-recovery outcome. It pauses the Run; never a transport retry.
	Context(ContextFailure),
	/// Every Decider Route was unavailable. The step is retried a bounded
	/// number of times, then pauses as `decider_unavailable` (ADR 0066).
	DeciderOutage,
	Authority(AuthorityFailure),
	/// Another Node did not answer a check the Run depends on (#751). With
	/// `retry` the step is retried a bounded number of times, then pauses as
	/// `node_unreachable`. Without it, a tool's effect has already happened,
	/// so the Run pauses at once.
	NodeUnreachable {
		node: String,
		retry: bool,
	},
	MediaRoute(String),
	Deferred,
	/// The database could not serve the step. The Run waits and resumes for as
	/// long as that lasts, spending none of its attempts (#728).
	DatabaseUnavailable,
	ToolRetry {
		count: u32,
		retry_after: Option<std::time::Duration>,
		message: String,
	},
	Inference {
		transport: bool,
		status: Option<u16>,
		message: String,
	},
	Other(String),
}
impl ExecutionFailure {
	pub fn retryable(&self) -> bool {
		matches!(
			self,
			Self::Inference {
				transport: true,
				..
			}
		) || matches!(
			self,
			Self::Inference {
				status: Some(408 | 429 | 500..=599),
				..
			}
		)
	}
	pub fn message(&self) -> String {
		match self {
			Self::Semantic(reason) => reason.to_string(),
			Self::Context(reason) => reason.to_string(),
			Self::Authority(failure) => failure.reason(),
			Self::MediaRoute(message) | Self::Inference { message, .. } | Self::Other(message) => {
				message.clone()
			}
			Self::Deferred => "execution state is temporarily unavailable".into(),
			Self::DatabaseUnavailable => "the database is temporarily unavailable".into(),
			Self::ToolRetry { message, .. } => message.clone(),
			Self::DeciderOutage => "every Decider Route is unavailable".into(),
			Self::NodeUnreachable { node, .. } => {
				format!("Node {node} could not be reached to confirm this. Try again.")
			}
		}
	}
}
