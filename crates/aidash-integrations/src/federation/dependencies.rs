//! Bounded dependency RPCs use the same tokens and bound decoding at 128 KiB.
use super::{HttpTransport, unavailable};
use aidash_application::{
	Error,
	ports::federation::dependencies::{Check, DependencyTransport, ResponseTooLarge},
};
use aidash_domain::federation::{dependencies::Input, route::Route};
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

// Protect local decode memory. 128 KiB covers the verdict and its disclosed dependency frontier.
const MAX_RESPONSE_BYTES: usize = 128 * 1024;

#[async_trait]
impl DependencyTransport for HttpTransport {
	async fn check(&self, route: &Route, input: &Input, remaining: Duration) -> Check {
		// The caller's remaining time bounds the headers and the whole body.
		let answered = tokio::time::timeout(remaining.min(Duration::from_secs(10)), async {
			let reply = match self
				.send(
					route,
					"POST",
					"/scoped/dependencies/verify",
					Some(&json!({"reference":input.reference})),
				)
				.await
			{
				Ok(reply) => reply,
				// Nothing was sent: this Deployment refuses to federate.
				Err(Error::FederationDisabled) => return Check::Refused,
				Err(error) => {
					unavailable(route, &error);
					return Check::Unavailable;
				}
			};
			let status = reply.status();
			if !status.is_success() {
				let check = Check::of_status(status.as_u16());
				if matches!(check, Check::Unavailable) {
					unavailable(route, &format!("HTTP {status}"));
				}
				return check;
			}
			let body = match crate::response::within(reply, MAX_RESPONSE_BYTES).await {
				Ok(Ok(body)) => body,
				Ok(Err(crate::response::Oversized)) => {
					unavailable(route, &ResponseTooLarge);
					return Check::ResponseTooLarge(ResponseTooLarge);
				}
				Err(error) => {
					unavailable(route, &error);
					return Check::Unavailable;
				}
			};
			match serde_json::from_slice(&body) {
				Ok(checked) => Check::Confirmed(checked),
				Err(error) => {
					unavailable(route, &error);
					Check::Unavailable
				}
			}
		})
		.await;
		answered.unwrap_or_else(|_| {
			unavailable(route, &"the request timed out");
			Check::Unavailable
		})
	}
}

#[cfg(test)]
mod tests;
