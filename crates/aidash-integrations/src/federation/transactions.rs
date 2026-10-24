//! Atomic protocol replies keep their deadline, body bound and retry classification.
use super::HttpTransport;
use aidash_application::{Error, Result};
use aidash_domain::federation::route::Route;
use serde::{Serialize, de::DeserializeOwned};
use std::{future::Future, time::Duration};

impl HttpTransport {
	/// Resolve the current enabled route inside the same ten-second deadline as
	/// sending, headers and the complete body. The server supplies persistence;
	/// each send mints its own token.
	pub async fn transaction<T: DeserializeOwned>(
		&self,
		route: impl Future<Output = Result<Route>>,
		method: &str,
		path: &str,
		body: Option<&impl Serialize>,
	) -> Result<T> {
		let value = body.map(serde_json::to_value).transpose()?;
		tokio::time::timeout(Duration::from_secs(10), async {
			let route = route.await?;
			let response = self.send(&route, method, path, value.as_ref()).await?;
			let status = response.status();
			if !status.is_success() {
				return Err(match status {
					reqwest::StatusCode::BAD_REQUEST
					| reqwest::StatusCode::UNPROCESSABLE_ENTITY
					| reqwest::StatusCode::NOT_FOUND
					| reqwest::StatusCode::METHOD_NOT_ALLOWED => Error::Invalid(format!(
						"transaction participant rejected request: {status}"
					)),
					reqwest::StatusCode::UNAUTHORIZED => Error::Unauthorized,
					reqwest::StatusCode::FORBIDDEN => Error::Forbidden,
					reqwest::StatusCode::CONFLICT => Error::Conflict(
						"transaction participant rejected its state precondition".into(),
					),
					reqwest::StatusCode::SERVICE_UNAVAILABLE
						if response
							.headers()
							.get("x-aidash-transaction-pending")
							.is_some_and(|v| v == "1") =>
					{
						Error::TransactionPending
					}
					_ => Error::External(format!("transaction participant returned {status}")),
				});
			}
			crate::response::json(response, 4_194_304).await
		})
		.await
		.map_err(|_| {
			Error::External(
				"transaction participant response timed out; outcome retained for recovery".into(),
			)
		})?
	}
}

#[cfg(test)]
mod tests;
