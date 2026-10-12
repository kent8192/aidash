//! Failure classifications shared by worker ports and native adapters.
use aidash_domain::{
	context::recovery::Failure as ContextFailure, semantic::Failure as SemanticFailure,
};

/// Adapter classification retains transport details outside the use case.
#[derive(Debug)]
pub enum ExecutionFailure {
	Semantic(SemanticFailure),
	/// A typed context-recovery outcome. It pauses the Run; never a transport retry.
	Context(ContextFailure),
	Authority {
		identity_unavailable: bool,
	},
	MediaRoute(String),
	Deferred,
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
			Self::Authority {
				identity_unavailable: true,
			} => "external identity status is unavailable".into(),
			Self::Authority {
				identity_unavailable: false,
			} => "execution authority denied".into(),
			Self::MediaRoute(message) | Self::Inference { message, .. } | Self::Other(message) => {
				message.clone()
			}
			Self::Deferred => "execution state is temporarily unavailable".into(),
		}
	}
}
