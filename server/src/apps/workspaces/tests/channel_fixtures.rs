//! Channel scenarios compose the production endpoint fixture and native HTTP client.
use crate::endpoint::{EndpointFixture, assert_json, endpoint, workspace};
use reinhardt::test::fixtures::api_client_from_url;
use reinhardt::test::{APIClient, TestResponse};
use rstest::fixture;
use serde_json::{Value, json};
use std::future::Future;

pub struct ScopedChannel {
	pub app: EndpointFixture,
	pub policy: Value,
	pub token: String,
	pub workspace: String,
}

#[fixture]
pub fn scoped_channel(#[future] endpoint: EndpointFixture) -> impl Future<Output = ScopedChannel> {
	// Box before composing the async state machine; nested bootstrap futures
	// otherwise exceed the default test-thread stack in debug builds.
	let endpoint = Box::pin(endpoint);
	Box::pin(async move {
		let app = endpoint.await;
		let policy = json!({
			"tenant":"acme", "subjects":{"alice":{"kind":"user"}},
			"policies":[{"id":"channel-work","effect":"allow","subjects":{"any":true},
				"actions":["*"],"resources":{"kinds":["*"]}}]
		});
		assert_json(
			app.operator
				.post(
					"/api/authorization/acme",
					&json!({"expected_revision":0,"bundle":policy}),
					"json",
				)
				.await
				.unwrap(),
			200,
		);
		let issued = assert_json(
			app.operator
				.post(
					"/api/authorization/acme/credentials",
					&json!({"subject":"alice"}),
					"json",
				)
				.await
				.unwrap(),
			200,
		);
		let token = issued["token"].as_str().unwrap().to_owned();
		let client = client_for(&app.server.url, &token).await;
		let created = workspace(&client, "Channel access fixture").await;
		ScopedChannel {
			app,
			policy,
			token,
			workspace: created["id"].as_str().unwrap().to_owned(),
		}
	})
}

pub async fn client_for(url: &str, token: &str) -> APIClient {
	let client = api_client_from_url(url);
	client
		.set_header("Authorization", &format!("Bearer {token}"))
		.await
		.unwrap();
	client
}

pub fn decoded(response: TestResponse) -> (u16, Value) {
	let status = response.status_code();
	(status, assert_json(response, status))
}

pub async fn request(
	app: &EndpointFixture,
	token: &str,
	method: &str,
	path: &str,
	body: Value,
) -> (u16, Value) {
	let client = client_for(&app.server.url, token).await;
	let response = match method {
		"GET" => client.get(path).await,
		"POST" => client.post(path, &body, "json").await,
		_ => panic!("unsupported channel test method: {method}"),
	}
	.unwrap();
	decoded(response)
}
