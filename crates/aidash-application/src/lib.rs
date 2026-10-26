//! Authorized use cases and the external capabilities they require.
pub mod accounts;
pub mod authorization;
pub mod catalog;
pub mod context;
pub mod decision;
pub mod deployment;
pub mod execution;
pub mod federation;
pub mod lifecycle;
pub mod memory;
pub mod plan_connections;
pub mod plugin_tools;
pub mod ports;
pub mod provider_access;
pub mod provider_authorization;
pub mod provider_credentials;
pub mod recovery;
pub mod research;
pub mod retention;
pub mod tenancy;
pub mod tenant_secrets;
pub mod usage;

pub type Result<T> = std::result::Result<T, Error>;

/// Errors at the use-case boundary contain no transport or database types.
#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	Domain(#[from] aidash_domain::Error),
	#[error(transparent)]
	Json(#[from] serde_json::Error),
	#[error("{0}")]
	Invalid(String),
	#[error("{0}")]
	Conflict(String),
	#[error("{0}")]
	NotFound(String),
	/// The request is well formed and the facts it names do not hold, such as a
	/// Deployment that does not publish the key it was registered with.
	#[error("{0}")]
	Unprocessable(String),
	/// `[deployment] federation` is off, or this Deployment holds no Deployment
	/// Key, so nothing is sent to another Deployment.
	#[error("federation_disabled")]
	FederationDisabled,
	/// The build's admission gate refused new work; the code is shown to the caller.
	#[error("{0}")]
	Refused(String),
	#[error("unauthorized")]
	Unauthorized,
	#[error("forbidden")]
	Forbidden,
	#[error("{0}")]
	External(String),
	#[error("verified model media route unavailable for {0}")]
	MediaRouteUnavailable(String),
	#[error("Kubernetes observations unavailable; check service account and API connectivity")]
	OrchestrationUnavailable,
	#[error("OpenRouter returned {status}: {reason}")]
	ProviderRejected { status: u16, reason: String },
	/// A streamed response produced no data within the stall timeout.
	#[error("model provider stream stalled")]
	InferenceStalled,
	#[error("semantic backend unavailable or invalid; inspect index status and retry")]
	SemanticUnavailable,
	#[error("external identity status is unavailable")]
	IdentityStatusUnavailable,
	#[error("{0}")]
	RemoteSemantic(aidash_domain::semantic::Failure),
	/// Typed context-recovery pause; never retried as transport.
	#[error("{0}")]
	Context(aidash_domain::context::recovery::Failure),
	/// Every Decider Route was unavailable in this decision. The step is
	/// retried a bounded number of times before it pauses as
	/// `decider_unavailable` (ADR 0066).
	#[error("every Decider Route is unavailable")]
	DeciderOutage,
	/// A truncated or refused provider completion. It pauses like `Context`;
	/// its usage still settles the call's reservation, and no tool call from
	/// it is ever dispatched.
	#[error("{0}")]
	TerminalResponse(
		aidash_domain::context::recovery::Failure,
		Box<aidash_domain::provider::ModelResponse>,
	),
	/// A Plan Connection could not serve a call (ADR 0007). Never retried as
	/// transport and never answered by a Provider Credential.
	#[error("{0}")]
	Plan(aidash_domain::plan_connections::Failure),
	/// Provider-proven Context Overflow. Only this enters compact-and-retry.
	#[error("the provider reported that the request exceeded its context window")]
	ContextOverflow,
	/// A request rendered under a Projection Version that its model does not
	/// declare. It is never retried or rendered under another version.
	#[error("model {model} does not support Projection Version {version}")]
	UnsupportedProjection {
		model: String,
		version: aidash_domain::projection::ProjectionVersion,
	},
	#[error("atomic transaction visibility pending; retry after recovery")]
	TransactionPending,
	/// The Tenant Secret broker could not serve the call. A person cannot repair
	/// this: the message names no broker reason, only the operator-side fault
	/// and the correlation the broker logged.
	#[error(
		"Tenant Secret calls are unavailable because of a Deployment operator issue (correlation: {})",
		correlation.as_deref().unwrap_or("none")
	)]
	BrokerUnavailable { correlation: Option<String> },
	/// The Tenant Secret broker refused the call as its operator configured it.
	/// The reason stays in server logs; the caller sees only the correlation.
	#[error(
		"Tenant Secret calls were refused because of a Deployment operator issue (correlation: {})",
		correlation.as_deref().unwrap_or("none")
	)]
	BrokerRejected { correlation: Option<String> },
	/// Opaque adapter errors retain their identity for retry and recovery.
	#[error(transparent)]
	Port(Box<dyn std::error::Error + Send + Sync>),
}

impl Error {
	/// Billable usage of a completed provider call, including a terminal one.
	pub fn terminal_usage(&self) -> Option<&aidash_domain::provider::ModelResponse> {
		match self {
			Self::TerminalResponse(_, usage) => Some(usage),
			_ => None,
		}
	}

	/// Once its usage is settled, a terminal response is a typed context pause.
	pub fn settled(self) -> Self {
		match self {
			Self::TerminalResponse(failure, _) => Self::Context(failure),
			error => error,
		}
	}
}

pub mod events;

pub mod tools;

pub mod registry;

pub mod workspaces;

pub mod marketplace;

/// Preserve portable contract failures at the transport/use-case boundary.
impl From<aidash_domain::semantic::remote::ContractError> for Error {
	fn from(error: aidash_domain::semantic::remote::ContractError) -> Self {
		use aidash_domain::semantic::remote::ContractError;
		match error {
			ContractError::Domain(error) => error.into(),
			ContractError::Json(error) => error.into(),
			ContractError::Semantic(reason) => Self::RemoteSemantic(reason),
		}
	}
}

pub mod generation;

pub mod semantic;

pub mod capabilities;

pub mod transactions;

pub mod activation;

#[cfg(test)]
pub(crate) mod test_support;

pub mod egress;
