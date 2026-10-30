//! Failure classifications shared by worker ports and native adapters.
use aidash_domain::{
	context::recovery::Failure as ContextFailure, semantic::Failure as SemanticFailure,
	tool::AuthorityRefusal,
};

/// Why a Run's authority stopped it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityFailure {
	/// The external identity provider's status could not be read.
	IdentityUnavailable,
	/// Authority was denied; the refusal names the tool and check when tool authorization refused.
	Denied(Option<AuthorityRefusal>),
}

impl AuthorityFailure {
	/// The sentence a paused Run shows as its `error`.
	pub fn reason(&self) -> String {
		match self {
			Self::IdentityUnavailable => "identity status unavailable".into(),
			Self::Denied(Some(refusal)) => refusal.to_string(),
			Self::Denied(None) => "execution authority denied".into(),
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
	MediaRoute(String),
	Deferred,
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
			Self::ToolRetry { message, .. } => message.clone(),
			Self::DeciderOutage => "every Decider Route is unavailable".into(),
		}
	}
}
