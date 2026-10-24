//! Bounded dependency RPCs use the same tokens and bound decoding at 128 KiB.
use super::HttpTransport;
use aidash_application::ports::federation::dependencies::DependencyTransport;
use aidash_domain::federation::{
	dependencies::{Checked, Input},
	route::Route,
};
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

#[async_trait]
impl DependencyTransport for HttpTransport {
	async fn check(&self, route: &Route, input: &Input, remaining: Duration) -> Option<Checked> {
		// The caller's remaining time bounds the headers and the whole body.
		tokio::time::timeout(remaining.min(Duration::from_secs(10)), async {
			let reply = self
				.send(
					route,
					"POST",
					"/scoped/dependencies/verify",
					Some(&json!({"reference":input.reference})),
				)
				.await
				.ok()?;
			if !reply.status().is_success() {
				return None;
			}
			crate::response::json(reply, 131072).await.ok()
		})
		.await
		.ok()?
	}
}

#[cfg(test)]
mod tests;
