use aidash_domain::identity::tenancy::CreationRefusal;
use http::{HeaderValue, header::RETRY_AFTER};
use reinhardt::core::exception::{DatabaseErrorKind, Error as FrameworkError};
use reinhardt::{Response as HttpResponse, StatusCode};
use serde_json::json;

fn tenant_creation_message(refusal: &CreationRefusal) -> &'static str {
	match refusal {
		CreationRefusal::NotAllowed => "tenant creation is not allowed",
		CreationRefusal::HourlyLimit { .. } => "tenant_creation_rate_limited",
		CreationRefusal::TotalLimit => "tenant_limit_reached",
	}
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("{}", .0.code())]
	Connection(aidash_domain::agent_plugin::ConnectionCause),
	#[error("{0}")]
	Framework(#[from] FrameworkError),
	#[error("{0}")]
	Invalid(String),
	#[error("{0}")]
	Conflict(String),
	#[error("{0}")]
	NotFound(String),
	/// Federation with other Deployments is off or has no key (503).
	#[error("federation_disabled")]
	FederationDisabled,
	/// The request is well formed and the facts it names do not hold (422).
	#[error("{0}")]
	Unprocessable(String),
	#[error("unauthorized")]
	Unauthorized,
	#[error("forbidden")]
	Forbidden,
	/// A tool call the Run's authority refused; the API answers it like [`Error::Forbidden`].
	#[error("forbidden")]
	AuthorityRefused(Box<aidash_domain::tool::AuthorityRefusal>),
	#[error("{0}")]
	ResumeBlocked(aidash_domain::run_state::RunReadsBlock),
	#[error("forbidden")]
	RunHidden(aidash_domain::run_state::RunReadsBlock),
	#[error("{0}")]
	RemoteExecutionRefused(aidash_domain::federation::execution::Refusal),
	/// The build's admission gate refused new work (402); the code is stable.
	#[error("{0}")]
	Refused(String),
	#[error("back-channel logout is busy; retry shortly")]
	RateLimited,
	/// The Tenancy Rules refuse this Tenant creation now.
	#[error("{}", tenant_creation_message(.0))]
	TenantCreation(aidash_domain::identity::tenancy::CreationRefusal),
	/// The Tenant is deleted (ADR 0060). Its members' browser Mappings see
	/// this; credentials that could be probed from outside see 401 instead.
	#[error("tenant_deleted")]
	TenantDeleted,
	/// "Nodes per Tenant", or the Tier's limit, refuses another Node.
	#[error("node_limit_reached")]
	NodeLimitReached,
	#[error("external identity status is unavailable")]
	IdentityStatusUnavailable,
	#[error("OAuth Client secret storage is unavailable")]
	OAuthClientSecretsUnavailable,
	#[error("verified model media route unavailable for {model}{}", aidash_application::route_refusal::CandidateRefusals(.candidates))]
	MediaRouteUnavailable {
		model: String,
		candidates: Vec<aidash_application::route_refusal::RouteRefusal>,
	},
	#[error("atomic transaction visibility pending; retry after recovery")]
	TransactionPending,
	/// Running work did not finish within the limit of an Authority Change,
	/// which was not applied (503). The same request can be sent again.
	#[error("POLICY_BUSY")]
	PolicyBusy { retry_after: u64 },
	#[error("an atomic transaction committed during inference; retrying from fresh state")]
	StaleInference,
	#[error("semantic backend unavailable or invalid; inspect index status and retry")]
	SemanticUnavailable,
	#[error("{0}")]
	RemoteSemantic(crate::semantic::remote::Failure),
	#[error("{0}")]
	Context(aidash_domain::context::recovery::Failure),
	#[error("every Decider Route is unavailable")]
	DeciderOutage,
	#[error("the provider reported that the request exceeded its context window")]
	ContextOverflow,
	#[error("Kubernetes observations unavailable; check service account and API connectivity")]
	OrchestrationUnavailable,
	#[error("{0}")]
	External(String),
	/// The configured Secret Store does not support MCP OAuth Connections.
	#[error("connection_unavailable")]
	ConnectionStoreUnavailable,
	#[error("{message}")]
	ToolNotDelivered {
		message: String,
		retry_after: Option<std::time::Duration>,
	},
	/// Another Node did not answer a check this call depends on (#751).
	#[error("Node {node} could not be reached to confirm this. Try again.")]
	NodeUnreachable { node: String, after_effect: bool },
	#[error("{message}")]
	ToolRetry {
		message: String,
		count: u32,
		retry_after: Option<std::time::Duration>,
	},
	#[error("OpenRouter returned {status}: {reason}")]
	ProviderRejected { status: u16, reason: String },
	/// A Plan Connection could not serve a call (ADR 0007); never retried.
	#[error("{0}")]
	Plan(aidash_domain::plan_connections::Failure),
	/// A streamed provider response stopped producing data. Retryable like a
	/// provider transport timeout.
	#[error("model provider stream stalled")]
	InferenceStalled,
	#[error("{0}")]
	Database(#[from] sqlx::Error),
	#[error("{0}")]
	Json(#[from] serde_json::Error),
	#[error("{0}")]
	Io(#[from] std::io::Error),
}

impl Error {
	pub fn http_response(self) -> HttpResponse {
		let transaction_pending =
			matches!(&self, Self::TransactionPending) || self.has_database_code("55P03");
		let run_message_pending = self.has_database_code("A3301");
		let (status, message) = match &self {
			Self::Invalid(s) => (StatusCode::BAD_REQUEST, s.clone()),
			Self::Conflict(s) => {
				tracing::debug!(reason = s, "request conflicts with current state");
				(StatusCode::CONFLICT, s.clone())
			}
			Self::NotFound(s) => (StatusCode::NOT_FOUND, s.clone()),
			Self::Unprocessable(s) => (StatusCode::UNPROCESSABLE_ENTITY, s.clone()),
			Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".into()),
			Self::Connection(cause) => (StatusCode::CONFLICT, cause.code().into()),
			Self::Forbidden | Self::AuthorityRefused(_) | Self::RunHidden(_) => {
				(StatusCode::FORBIDDEN, "forbidden".into())
			}
			Self::ResumeBlocked(block) => (StatusCode::CONFLICT, block.to_string()),
			Self::RemoteExecutionRefused(reason) => (StatusCode::FORBIDDEN, reason.to_string()),
			Self::Refused(code) => (StatusCode::PAYMENT_REQUIRED, code.clone()),
			Self::RateLimited => (StatusCode::TOO_MANY_REQUESTS, self.to_string()),
			Self::TenantDeleted => (StatusCode::GONE, self.to_string()),
			Self::NodeLimitReached => (StatusCode::FORBIDDEN, self.to_string()),
			Self::FederationDisabled => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
			Self::TenantCreation(refusal) => (
				match refusal {
					CreationRefusal::HourlyLimit { .. } => StatusCode::TOO_MANY_REQUESTS,
					CreationRefusal::NotAllowed | CreationRefusal::TotalLimit => {
						StatusCode::FORBIDDEN
					}
				},
				self.to_string(),
			),
			Self::IdentityStatusUnavailable
			| Self::OAuthClientSecretsUnavailable
			| Self::MediaRouteUnavailable { .. } => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
			Self::TransactionPending
			| Self::ConnectionStoreUnavailable
			| Self::SemanticUnavailable
			| Self::OrchestrationUnavailable => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
			Self::RemoteSemantic(reason) => (
				if reason.transient() || matches!(reason, crate::semantic::remote::Failure::Pending)
				{
					StatusCode::SERVICE_UNAVAILABLE
				} else {
					StatusCode::CONFLICT
				},
				self.to_string(),
			),
			Self::StaleInference => (StatusCode::CONFLICT, self.to_string()),
			Self::Context(_) => (StatusCode::CONFLICT, self.to_string()),
			Self::DeciderOutage | Self::NodeUnreachable { .. } => {
				(StatusCode::SERVICE_UNAVAILABLE, self.to_string())
			}
			Self::ContextOverflow => (
				StatusCode::BAD_REQUEST,
				"the request exceeded the model context window".into(),
			),
			Self::ProviderRejected { status, .. } => (
				StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY),
				self.to_string(),
			),
			Self::Plan(failure) => {
				use aidash_domain::plan_connections::Failure;
				(
					match failure {
						Failure::UsageLimit => StatusCode::TOO_MANY_REQUESTS,
						Failure::Unsupported(_) => StatusCode::BAD_REQUEST,
						Failure::Misconfigured => StatusCode::BAD_GATEWAY,
						Failure::NotConnected
						| Failure::NotAdmitted
						| Failure::NeedsReconnection
						| Failure::Ineligible
						| Failure::Restricted => StatusCode::CONFLICT,
					},
					self.to_string(),
				)
			}
			Self::Framework(FrameworkError::ParamValidation(context))
				if matches!(
					context.param_type,
					reinhardt::core::exception::ParamType::Json
				) =>
			{
				let status = if context.field_name.as_deref() == Some("Content-Type") {
					StatusCode::UNSUPPORTED_MEDIA_TYPE
				} else {
					StatusCode::BAD_REQUEST
				};
				(status, "invalid JSON request".into())
			}
			_ if run_message_pending => {
				(StatusCode::CONFLICT, "run messages await inference".into())
			}
			_ if transaction_pending => (
				StatusCode::SERVICE_UNAVAILABLE,
				"atomic transaction visibility pending; retry after recovery".into(),
			),
			Self::PolicyBusy { .. } => (StatusCode::SERVICE_UNAVAILABLE, "POLICY_BUSY".into()),
			_ if self.is_transient_database() => {
				tracing::warn!(error = %self, "request failed on a temporary database error");
				(
					StatusCode::SERVICE_UNAVAILABLE,
					"database temporarily unavailable; retry".into(),
				)
			}
			Self::Framework(error) if error.database_error().is_some() => {
				tracing::error!(error = %error, "request failed");
				(
					StatusCode::INTERNAL_SERVER_ERROR,
					"operation failed; see server logs".into(),
				)
			}
			Self::Framework(error) if error.status_code() < 500 => {
				tracing::debug!(error = %error, "request rejected by framework");
				(
					StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::BAD_REQUEST),
					"invalid request".into(),
				)
			}
			_ => {
				tracing::error!(error = %self, "request failed");
				(
					StatusCode::INTERNAL_SERVER_ERROR,
					"operation failed; see server logs".into(),
				)
			}
		};
		let mut envelope = match &self {
			Self::NodeUnreachable { node, after_effect } => {
				json!({"error": message, "code": "node_unreachable", "node": node, "after_effect": after_effect})
			}
			_ => json!({"error": message}),
		};
		if let Self::RunHidden(block) | Self::ResumeBlocked(block) = &self {
			envelope["run_reads"] = json!(block);
		}
		let mut response = HttpResponse::new(status)
			.with_body(serde_json::to_vec(&envelope).expect("error envelope serializes"))
			.with_header("Content-Type", "application/json");
		if let Self::RemoteExecutionRefused(reason) = &self
			&& let Some(code) = serde_json::to_value(reason)
				.ok()
				.and_then(|v| v.as_str().map(str::to_owned))
			&& let Ok(value) = HeaderValue::from_str(&code)
		{
			response.headers.insert("x-aidash-execution-refusal", value);
		}
		if transaction_pending {
			response.headers.insert(
				"x-aidash-transaction-pending",
				HeaderValue::from_static("1"),
			);
		}
		if run_message_pending {
			response.headers.insert(
				"x-aidash-run-message-pending",
				HeaderValue::from_static("1"),
			);
		}
		if let Self::TenantCreation(CreationRefusal::HourlyLimit {
			retry_after_seconds,
		}) = &self
		{
			response
				.headers
				.insert(RETRY_AFTER, HeaderValue::from(*retry_after_seconds));
		} else if let Self::PolicyBusy { retry_after } = &self {
			response
				.headers
				.insert(RETRY_AFTER, HeaderValue::from(*retry_after));
			// A peer's client keeps no body of a refusal.
			response
				.headers
				.insert("x-aidash-policy-busy", HeaderValue::from(*retry_after));
		} else if status == StatusCode::SERVICE_UNAVAILABLE
			|| status == StatusCode::TOO_MANY_REQUESTS
		{
			response
				.headers
				.insert(RETRY_AFTER, HeaderValue::from_static("1"));
		}
		if let Self::RemoteSemantic(reason) = self
			&& let Some(code) = serde_json::to_value(reason)
				.ok()
				.and_then(|v| v.as_str().map(str::to_owned))
			&& let Ok(value) = http::HeaderValue::from_str(&code)
		{
			if reason.compatible() != reason {
				response.headers.insert("x-aidash-semantic-cause", value);
				response.headers.insert(
					"x-aidash-semantic-reason",
					HeaderValue::from_static("context_budget"),
				);
			} else {
				response.headers.insert("x-aidash-semantic-reason", value);
			}
		}

		response
	}
}

impl Error {
	pub(crate) fn has_database_code(&self, expected: &str) -> bool {
		match self {
			Self::Database(error) => error
				.as_database_error()
				.is_some_and(|error| error.code().as_deref() == Some(expected)),
			Self::Framework(error) => error
				.database_error()
				.is_some_and(|error| error.code() == Some(expected)),
			_ => false,
		}
	}

	/// Whether this came from the database at all, temporary or not. Callers
	/// that otherwise redact or flatten an error keep these typed (#787).
	pub(crate) fn is_database(&self) -> bool {
		match self {
			Self::Database(_) => true,
			Self::Framework(error) => error.database_error().is_some(),
			_ => false,
		}
	}

	pub(crate) fn is_transient_database(&self) -> bool {
		fn retryable_code(code: &str) -> bool {
			code.starts_with("08")
				|| matches!(
					code,
					"40001"
						| "40P01" | "53300"
						| "55P03" | "57014"
						| "57P01" | "57P02"
						| "57P03" | "A3301"
				)
		}
		match self {
			Self::Database(
				sqlx::Error::PoolTimedOut
				| sqlx::Error::Io(_)
				| sqlx::Error::Tls(_)
				| sqlx::Error::Protocol(_),
			) => true,
			Self::Database(sqlx::Error::Database(error)) => {
				error.code().is_some_and(|code| retryable_code(&code))
			}
			Self::Framework(error) => error.database_error().is_some_and(|error| {
				matches!(
					error.kind(),
					DatabaseErrorKind::Connection | DatabaseErrorKind::Timeout
				) || error.code().is_some_and(retryable_code)
			}),
			_ => false,
		}
	}

	/// The database could not be reached or did not answer: a lost or refused
	/// connection, or a client deadline. Lock waits, deadlocks and serialization
	/// failures are transient too, but they happen while the database is up, so
	/// they do not start a post-outage hold on lease recoveries (#786).
	pub(crate) fn is_database_unreachable(&self) -> bool {
		fn connection_code(code: &str) -> bool {
			code.starts_with("08") || matches!(code, "57P01" | "57P02" | "57P03")
		}
		match self {
			Self::Database(
				sqlx::Error::PoolTimedOut
				| sqlx::Error::Io(_)
				| sqlx::Error::Tls(_)
				| sqlx::Error::Protocol(_),
			) => true,
			Self::Database(sqlx::Error::Database(error)) => {
				error.code().is_some_and(|code| connection_code(&code))
			}
			Self::Framework(error) => error.database_error().is_some_and(|error| {
				matches!(
					error.kind(),
					DatabaseErrorKind::Connection | DatabaseErrorKind::Timeout
				) || error.code().is_some_and(connection_code)
			}),
			_ => false,
		}
	}
}

#[cfg(test)]
#[path = "apps/execution/tests/errors.rs"]
mod tests;

impl From<reqwest::Error> for Error {
	fn from(e: reqwest::Error) -> Self {
		// URLs can contain credentials; provider response bodies are never exposed.
		Self::External(e.without_url().to_string())
	}
}

impl From<aidash_domain::Error> for Error {
	fn from(error: aidash_domain::Error) -> Self {
		match error {
			aidash_domain::Error::Invalid(message) => Self::Invalid(message),
			aidash_domain::Error::Conflict(message) => Self::Conflict(message),
			error @ aidash_domain::Error::UnsupportedState(_) => Self::Conflict(error.to_string()),
		}
	}
}

impl From<aidash_application::Error> for Error {
	fn from(error: aidash_application::Error) -> Self {
		use aidash_application::Error as ApplicationError;
		match error {
			ApplicationError::Connection(cause) => Self::Connection(cause),
			ApplicationError::Domain(error) => error.into(),
			ApplicationError::Json(error) => Self::Json(error),
			ApplicationError::Invalid(message) => Self::Invalid(message),
			ApplicationError::Conflict(message) => Self::Conflict(message),
			ApplicationError::NotFound(message) => Self::NotFound(message),
			ApplicationError::Unprocessable(message) => Self::Unprocessable(message),
			ApplicationError::FederationDisabled => Self::FederationDisabled,
			ApplicationError::Refused(code) => Self::Refused(code),
			ApplicationError::Unauthorized => Self::Unauthorized,
			ApplicationError::Forbidden => Self::Forbidden,
			ApplicationError::AuthorityRefused(refusal) => Self::AuthorityRefused(refusal),
			ApplicationError::ResumeBlocked(block) => Self::ResumeBlocked(block),
			ApplicationError::RunHidden(block) => Self::RunHidden(block),
			ApplicationError::RemoteExecutionRefused(reason) => {
				Self::RemoteExecutionRefused(reason)
			}
			ApplicationError::External(message) => Self::External(message),
			ApplicationError::ConnectionStoreUnavailable => Self::ConnectionStoreUnavailable,
			ApplicationError::ToolNotDelivered {
				message,
				retry_after,
			} => Self::ToolNotDelivered {
				message,
				retry_after,
			},
			ApplicationError::NodeUnreachable { node, after_effect } => {
				Self::NodeUnreachable { node, after_effect }
			}
			ApplicationError::ToolServiceStatus { status, .. } => {
				Self::External(format!("tool service returned HTTP {status}"))
			}
			ApplicationError::ToolRetry {
				message,
				count,
				retry_after,
			} => Self::ToolRetry {
				message,
				count,
				retry_after,
			},
			ApplicationError::MediaRouteUnavailable { model, candidates } => {
				Self::MediaRouteUnavailable { model, candidates }
			}
			error @ ApplicationError::ProviderRouteUnavailable { .. } => {
				Self::Invalid(error.to_string())
			}
			ApplicationError::OrchestrationUnavailable => Self::OrchestrationUnavailable,
			ApplicationError::RemoteSemantic(reason) => Self::RemoteSemantic(reason),
			ApplicationError::Context(reason) | ApplicationError::TerminalResponse(reason, _) => {
				Self::Context(reason)
			}
			ApplicationError::ContextOverflow => Self::ContextOverflow,
			ApplicationError::DeciderOutage => Self::DeciderOutage,
			error @ ApplicationError::UnsupportedProjection { .. } => {
				Self::Invalid(error.to_string())
			}
			ApplicationError::SemanticUnavailable => Self::SemanticUnavailable,
			ApplicationError::IdentityStatusUnavailable => Self::IdentityStatusUnavailable,
			ApplicationError::OAuthClientSecretsUnavailable => Self::OAuthClientSecretsUnavailable,
			ApplicationError::ProviderRejected { status, reason } => {
				Self::ProviderRejected { status, reason }
			}
			ApplicationError::Plan(failure) => Self::Plan(failure),
			ApplicationError::InferenceStalled => Self::InferenceStalled,
			ApplicationError::TransactionPending => Self::TransactionPending,
			ApplicationError::PolicyBusy { retry_after } => Self::PolicyBusy { retry_after },
			ApplicationError::RunLockContended => Self::External("Run row lock contended".into()),
			// The broker fault keeps its generic Deployment Administrator message and correlation.
			error @ (ApplicationError::BrokerUnavailable { .. }
			| ApplicationError::BrokerRejected { .. }) => Self::External(error.to_string()),
			ApplicationError::Port(error) | ApplicationError::TemporaryDatabase(error) => {
				match error.downcast::<Self>() {
					Ok(error) => *error,
					Err(error) => Self::External(error.to_string()),
				}
			}
		}
	}
}

/// Preserve database codes and domain-specific recovery errors across ports.
impl From<Error> for aidash_application::Error {
	fn from(error: Error) -> Self {
		match error {
			Error::Connection(cause) => Self::Connection(cause),
			Error::Invalid(message) => Self::Invalid(message),
			Error::Conflict(message) => Self::Conflict(message),
			Error::NotFound(message) => Self::NotFound(message),
			Error::Unprocessable(message) => Self::Unprocessable(message),
			Error::FederationDisabled => Self::FederationDisabled,
			Error::Forbidden => Self::Forbidden,
			// Work stopped by a Tenant Deletion pauses like any refused authority. The note
			// names the check where it reaches the application, so every source of the error
			// (the Tenant's policy lock, its deletion record) is covered at once.
			Error::TenantDeleted => {
				aidash_application::authorization::refusal_note::set(
					aidash_application::authorization::refusal_note::Note::new(
						aidash_domain::tool::RefusalCheck::TenantDeletion,
						None,
					),
				);
				Self::Forbidden
			}
			Error::AuthorityRefused(refusal) => Self::AuthorityRefused(refusal),
			Error::ResumeBlocked(block) => Self::ResumeBlocked(block),
			Error::RunHidden(block) => Self::RunHidden(block),
			Error::RemoteExecutionRefused(reason) => Self::RemoteExecutionRefused(reason),
			Error::Refused(code) => Self::Refused(code),
			Error::Unauthorized => Self::Unauthorized,
			Error::External(message) => Self::External(message),
			Error::Json(error) => Self::Json(error),
			Error::RemoteSemantic(reason) => Self::RemoteSemantic(reason),
			Error::Context(reason) => Self::Context(reason),
			Error::DeciderOutage => Self::DeciderOutage,
			Error::ContextOverflow => Self::ContextOverflow,
			Error::MediaRouteUnavailable { model, candidates } => {
				Self::MediaRouteUnavailable { model, candidates }
			}
			Error::SemanticUnavailable => Self::SemanticUnavailable,
			Error::IdentityStatusUnavailable => Self::IdentityStatusUnavailable,
			Error::ConnectionStoreUnavailable => Self::ConnectionStoreUnavailable,
			Error::TransactionPending => Self::TransactionPending,
			Error::PolicyBusy { retry_after } => Self::PolicyBusy { retry_after },
			Error::InferenceStalled => Self::InferenceStalled,
			Error::ToolNotDelivered {
				message,
				retry_after,
			} => Self::ToolNotDelivered {
				message,
				retry_after,
			},
			Error::NodeUnreachable { node, after_effect } => {
				Self::NodeUnreachable { node, after_effect }
			}
			Error::ToolRetry {
				message,
				count,
				retry_after,
			} => Self::ToolRetry {
				message,
				count,
				retry_after,
			},
			Error::Plan(failure) => Self::Plan(failure),
			error if error.is_transient_database() => Self::TemporaryDatabase(Box::new(error)),
			error => Self::Port(Box::new(error)),
		}
	}
}

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

impl From<aidash_domain::transactions::MissingParticipant> for Error {
	fn from(_: aidash_domain::transactions::MissingParticipant) -> Self {
		Self::Forbidden
	}
}
