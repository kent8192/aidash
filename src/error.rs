use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    NotFound(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("atomic transaction visibility pending; retry after recovery")]
    TransactionPending,
    #[error("{0}")]
    External(String),
    #[error("{0}")]
    Database(#[from] sqlx::Error),
    #[error("{0}")]
    Orm(#[from] sea_orm::DbErr),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Self::Invalid(s) => (StatusCode::BAD_REQUEST, s.clone()),
            Self::Conflict(s) => (StatusCode::CONFLICT, s.clone()),
            Self::NotFound(s) => (StatusCode::NOT_FOUND, s.clone()),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".into()),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden".into()),
            Self::TransactionPending => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
            Self::Database(error)
                if error
                    .as_database_error()
                    .is_some_and(|e| e.code().as_deref() == Some("55P03")) =>
            {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "atomic transaction visibility pending; retry after recovery".into(),
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
        let mut response = (status, Json(json!({"error": message}))).into_response();
        if status == StatusCode::SERVICE_UNAVAILABLE {
            response.headers_mut().insert(
                "x-aidash-transaction-pending",
                axum::http::HeaderValue::from_static("1"),
            );
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("1"),
            );
        }
        response
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        // URLs can contain credentials; provider response bodies are never exposed.
        Self::External(e.without_url().to_string())
    }
}
