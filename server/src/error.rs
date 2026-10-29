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
	#[error("verified model media route unavailable for {0}")]
	MediaRouteUnavailable(String),
	#[error("atomic transaction visibility pending; retry after recovery")]
	TransactionPending,
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
			Self::Forbidden | Self::AuthorityRefused(_) => {
				(StatusCode::FORBIDDEN, "forbidden".into())
			}
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
			| Self::MediaRouteUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
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
			Self::DeciderOutage => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
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
		let mut response = HttpResponse::new(status)
			.with_body(
				serde_json::to_vec(&json!({"error": message})).expect("error envelope serializes"),
			)
			.with_header("Content-Type", "application/json");
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

	pub(crate) fn is_transient_database(&self) -> bool {
		fn retryable_code(code: &str) -> bool {
			code.starts_with("08")
				|| matches!(
					code,
					"40001" | "40P01" | "53300" | "55P03" | "57P01" | "57P02" | "57P03" | "A3301"
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
			ApplicationError::External(message) => Self::External(message),
			ApplicationError::ConnectionStoreUnavailable => Self::ConnectionStoreUnavailable,
			ApplicationError::MediaRouteUnavailable(model) => Self::MediaRouteUnavailable(model),
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
			// The broker fault keeps its generic Deployment Administrator message and correlation.
			error @ (ApplicationError::BrokerUnavailable { .. }
			| ApplicationError::BrokerRejected { .. }) => Self::External(error.to_string()),
			ApplicationError::Port(error) => match error.downcast::<Self>() {
				Ok(error) => *error,
				Err(error) => Self::External(error.to_string()),
			},
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
			// Work stopped by a Tenant Deletion pauses like any refused authority.
			Error::Forbidden | Error::TenantDeleted => Self::Forbidden,
			Error::AuthorityRefused(refusal) => Self::AuthorityRefused(refusal),
			Error::Refused(code) => Self::Refused(code),
			Error::Unauthorized => Self::Unauthorized,
			Error::Json(error) => Self::Json(error),
			Error::RemoteSemantic(reason) => Self::RemoteSemantic(reason),
			Error::Context(reason) => Self::Context(reason),
			Error::DeciderOutage => Self::DeciderOutage,
			Error::ContextOverflow => Self::ContextOverflow,
			Error::SemanticUnavailable => Self::SemanticUnavailable,
			Error::IdentityStatusUnavailable => Self::IdentityStatusUnavailable,
			Error::ConnectionStoreUnavailable => Self::ConnectionStoreUnavailable,
			Error::TransactionPending => Self::TransactionPending,
			Error::InferenceStalled => Self::InferenceStalled,
			Error::Plan(failure) => Self::Plan(failure),
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
