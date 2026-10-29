//! Isolated HTTP test tools use no redirects and a bounded response. A test
//! endpoint is the Tenant's choice, so it reaches public addresses only,
//! unless the Deployment permits its origin (#361, ADR 0036).
use crate::broker;
use aidash_application::{
	Result,
	ports::registry::workbench::sandbox::dispatch::{PreparedRealRequest, RealToolTransport},
	provider_access::KeylessAccess,
	tenant_secrets::CallSecret,
};
use aidash_domain::{
	keyless::Destination, provider::ToolCall, registry::workbench::profile::RealToolRule,
};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;
const TIMEOUT: Duration = Duration::from_secs(30);
pub struct RealTools {
	origins: KeylessAccess,
	/// For a permitted origin. It follows no redirects, so the origin cannot
	/// hand a request on to another private origin.
	permitted: reqwest::Client,
	/// Every other endpoint: public addresses only.
	public: reqwest::Client,
}
impl RealTools {
	pub fn new(origins: KeylessAccess) -> crate::Result<Self> {
		Ok(Self {
			origins,
			permitted: reqwest::Client::builder()
				.redirect(reqwest::redirect::Policy::none())
				.timeout(TIMEOUT)
				.build()
				.map_err(crate::http_error)?,
			public: crate::keyless::public_client(TIMEOUT, TIMEOUT)?,
		})
	}
}
struct Request {
	request: reqwest::RequestBuilder,
	brokered: bool,
}
#[async_trait]
impl RealToolTransport for RealTools {
	async fn prepare(
		&self,
		session: Uuid,
		rule: &RealToolRule,
		call: &ToolCall,
		secret: Option<&CallSecret>,
	) -> Result<Box<dyn PreparedRealRequest>> {
		let client = match self.origins.destination(&rule.endpoint)? {
			Destination::Permitted => &self.permitted,
			Destination::PublicOnly => &self.public,
			Destination::Forbidden => return Err(aidash_application::Error::Forbidden),
		};
		let brokered = matches!(secret, Some(CallSecret::Brokered { .. }));
		let request = if let Some(CallSecret::Brokered { source, rest, .. }) = secret {
			// The broker is Deployment configuration, so it may be an in-cluster
			// private address. The test endpoint's authority check above still ran.
			let route = source.route().await?;
			let url = broker::forward_url(&route.base, rest)?;
			self.permitted
				.post(url)
				.header(broker::SECRET_CALL, route.token.expose_secret())
				.header("idempotency-key", format!("test-{session}-{}", call.id))
				.json(&call.arguments)
		} else {
			let request = client
				.post(&rule.endpoint)
				.header("idempotency-key", format!("test-{session}-{}", call.id))
				.json(&call.arguments);
			// A self-hosted Tenant Secret goes only to the test endpoint, one of
			// its recorded hosts.
			if let Some(CallSecret::Value { value, .. }) = secret {
				request.bearer_auth(value.expose_secret())
			} else {
				request
			}
		};
		Ok(Box::new(Request { request, brokered }))
	}
}
#[async_trait]
impl PreparedRealRequest for Request {
	async fn send(self: Box<Self>) -> Result<(Value, &'static str)> {
		let response = self.request.send().await.map_err(|error| {
			if self.brokered && !crate::keyless::refused(&error) {
				broker::BrokerFailure::connect_failure().into_error()
			} else {
				crate::keyless::send_error(error)
			}
		})?;
		if self.brokered
			&& let Some(failure) = broker::BrokerFailure::from_headers(response.headers())
		{
			// The broker could not reach the test endpoint, limited the rate, or
			// named a repairable Tenant Secret problem: keep the failed outcome
			// that the test session records. A Deployment Administrator fault is an error.
			if failure.is_upstream_or_rate_limited() {
				return Ok((
					json!({"error":"test Tool returned an HTTP error","status":response.status().as_u16()}),
					"failed",
				));
			}
			if failure.is_repairable() {
				return Ok((json!({"error":failure.code_str()}), "failed"));
			}
			return Err(failure.into_error());
		}
		if response.status().is_success() {
			Ok((crate::response::json(response, 256_000).await?, "real"))
		} else {
			Ok((
				json!({"error":"test Tool returned an HTTP error","status":response.status().as_u16()}),
				"failed",
			))
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidash_application::tenant_secrets::{
		CallSecret, SecretRecord, SecretRoute, SecretRouteSource,
	};
	use axum::{
		Json, Router,
		http::{HeaderMap, Uri},
	};
	use serde_json::json;
	use std::sync::Arc;

	struct FixedRoute {
		base: String,
		token: &'static str,
	}
	#[async_trait]
	impl SecretRouteSource for FixedRoute {
		async fn route(&self) -> aidash_application::Result<SecretRoute> {
			Ok(SecretRoute {
				base: reqwest::Url::parse(&self.base).unwrap(),
				token: secrecy::SecretString::from(self.token.to_owned()),
			})
		}
	}

	#[tokio::test]
	async fn a_workbench_test_call_is_forwarded_through_the_broker_when_brokered() {
		let router = Router::new().fallback(|uri: Uri, headers: HeaderMap| async move {
			Json(json!({
				"path": uri.path(),
				"token": headers.get("x-aidash-secret-call").and_then(|v| v.to_str().ok()),
				"authorization": headers.get("authorization").and_then(|v| v.to_str().ok()),
			}))
		});
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let base = format!("http://{}", listener.local_addr().unwrap());
		let server = tokio::spawn(async move {
			axum::serve(listener, router).await.unwrap();
		});
		let endpoint = "http://127.0.0.1:9/api/rpc".to_owned();
		let tools = RealTools::new(KeylessAccess::permitting(
			aidash_domain::keyless::PermittedOrigins::parse(["http://127.0.0.1:9".to_string()])
				.unwrap(),
		))
		.unwrap();
		let rule = RealToolRule {
			read_only_verified: true,
			tool: aidash_domain::registry::EntityRef {
				id: "tool".into(),
				version: "1.0.0".into(),
			},
			endpoint,
			allowed_actions: vec!["read".into()],
			allowed_resources: vec!["fixture".into()],
			secret: Some("key".into()),
		};
		let call = ToolCall {
			id: "call-1".into(),
			name: "tool".into(),
			arguments: json!({"action":"read","resource":"fixture"}),
		};
		let secret = CallSecret::Brokered {
			record: SecretRecord {
				id: uuid::Uuid::now_v7(),
				version: "3".into(),
				host: "127.0.0.1".into(),
			},
			source: Arc::new(FixedRoute {
				base: format!("{base}/v1/forward"),
				token: "broker-token",
			}),
			rest: "rpc".into(),
		};
		let request = tools
			.prepare(uuid::Uuid::now_v7(), &rule, &call, Some(&secret))
			.await
			.unwrap();
		let (result, outcome) = request.send().await.unwrap();
		assert_eq!(outcome, "real");
		assert_eq!(result["path"], "/v1/forward/rpc");
		assert_eq!(result["token"], "broker-token");
		assert_eq!(result["authorization"], serde_json::Value::Null);
		server.abort();
		let _ = server.await;
	}
	#[tokio::test]
	async fn workbench_classifies_broker_failures_without_exposing_the_body() {
		let router = Router::new().fallback(|uri: Uri| async move {
			let code = uri.path().trim_start_matches('/').to_owned();
			(
				axum::http::StatusCode::BAD_GATEWAY,
				[
					("x-aidash-broker-error", code),
					("x-aidash-correlation", "trace-1".to_owned()),
				],
				"secret broker body",
			)
		});
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let base = format!("http://{}", listener.local_addr().unwrap());
		let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
		for code in [
			"secret_unavailable",
			"secret_destination_refused",
			"upstream_unreachable",
			"rate_limited",
			"broker_rejected",
			"broker_unavailable",
			"unexpected",
		] {
			let request = Request {
				request: reqwest::Client::new().post(format!("{base}/{code}")),
				brokered: true,
			};
			let result = Box::new(request).send().await;
			match code {
				"secret_unavailable" | "secret_destination_refused" => {
					let (body, outcome) = result.unwrap();
					assert_eq!(outcome, "failed");
					assert_eq!(body, json!({"error": code}));
				}
				"upstream_unreachable" | "rate_limited" => {
					let (body, outcome) = result.unwrap();
					assert_eq!(outcome, "failed");
					assert_eq!(
						body,
						json!({"error":"test Tool returned an HTTP error", "status":502})
					);
				}
				"broker_rejected" => assert!(
					matches!(result, Err(aidash_application::Error::BrokerRejected { correlation: Some(id) }) if id == "trace-1")
				),
				_ => assert!(
					matches!(result, Err(aidash_application::Error::BrokerUnavailable { correlation: Some(id) }) if id == "trace-1")
				),
			}
		}
		server.abort();
		let _ = server.await;
	}
}
