// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

use super::*;
#[test]
fn resources_and_numeric_pins_are_confined_to_the_configured_environment() {
	let store = SecretManager::new("byok-project".into(), "dev".into()).unwrap();
	let id = Uuid::now_v7();
	let resource = store.resource(id);
	assert_eq!(
		resource,
		format!("projects/byok-project/secrets/aidash-dev-cred-{id}")
	);
	assert!(store.validate_resource(&resource).is_ok());
	assert!(
		store
			.validate_version(&format!("{resource}/versions/1"))
			.is_ok()
	);
	for value in [
		resource.replace("dev-cred", "prod-cred"),
		resource.replace("byok-project", "shared-project"),
		resource.replace(&id.to_string(), &Uuid::new_v4().to_string()),
	] {
		assert!(store.validate_resource(&value).is_err());
	}
	for pin in ["latest", "0", "1:access", "../access"] {
		assert!(
			store
				.validate_version(&format!("{resource}/versions/{pin}"))
				.is_err()
		);
	}
}

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}
async fn serve(app: axum::Router) -> (String, Server) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}", listener.local_addr().unwrap());
	(
		url,
		Server(tokio::spawn(async move {
			axum::serve(listener, app).await.unwrap();
		})),
	)
}
#[tokio::test]
async fn rest_store_writes_payload_once_and_uses_only_metadata_and_lifecycle_endpoints() {
	use axum::{
		Json, Router,
		body::Bytes,
		http::{HeaderMap, Method, Uri},
	};
	use std::sync::{Arc, Mutex};
	let id = Uuid::now_v7();
	let resource = format!("projects/byok-project/secrets/aidash-dev-cred-{id}");
	let calls = Arc::new(Mutex::new(Vec::new()));
	let history = calls.clone();
	let remote = resource.clone();
	let app=Router::new().fallback(move |method:Method,uri:Uri,headers:HeaderMap,body:Bytes| { let history=history.clone();let remote=remote.clone(); async move {
  let path=uri.to_string();
  let value = if path=="/token" { assert_eq!(headers["metadata-flavor"],"Google"); json!({"access_token":"metadata-token","expires_in":3600}) } else {
  assert_eq!(headers["authorization"],"Bearer metadata-token");
  let body:Value=if body.is_empty(){Value::Null}else{serde_json::from_slice(&body).unwrap()};
  history.lock().unwrap().push((method.to_string(),path.clone(),body));
  if path.contains(":addVersion") {json!({"name":format!("{remote}/versions/2")})}
  else if path.contains("pageToken="){json!({"versions":[{"name":format!("{remote}/versions/3"),"state":"DESTROYED"}]})}
  else if path.contains("?pageSize="){json!({"versions":[{"name":format!("{remote}/versions/1"),"state":"DISABLED"},{"name":format!("{remote}/versions/2"),"state":"ENABLED"}],"nextPageToken":"next page"})}
  else if method==Method::GET && path.ends_with("/versions/1"){json!({"state":"DISABLED"})}
  else if method==Method::GET && path.ends_with("/versions/3"){json!({"state":"DESTROYED"})}
  else {json!({"state":"ENABLED"})}
  };
  ([("Metadata-Flavor", "Google")], Json(value))
 }});
	let (url, _server) = serve(app).await;
	let mut store = SecretManager::new("byok-project".into(), "dev".into()).unwrap();
	store.api = format!("{url}/v1");
	store.metadata = format!("{url}/token");
	store.create("alpha", id).await.unwrap();
	let key: SecretString = "canary-provider-write-only-key".into();
	let version = store.add_version("alpha", &resource, &key).await.unwrap();
	assert_eq!(version, format!("{resource}/versions/2"));
	assert_eq!(store.versions("alpha", &resource).await.unwrap().len(), 2);
	store.disable("alpha", &version).await.unwrap();
	store
		.disable("alpha", &format!("{resource}/versions/1"))
		.await
		.unwrap();
	store.destroy("alpha", &version).await.unwrap();
	store
		.destroy("alpha", &format!("{resource}/versions/3"))
		.await
		.unwrap();
	store.delete("alpha", &resource).await.unwrap();
	let calls = calls.lock().unwrap();
	assert!(
		calls.iter().all(|(_, p, _)| !p.contains(":access")
			&& (!p.ends_with("/secrets") || p.contains("secretId=")))
	);
	let created = &calls[0].2;
	assert_eq!(
		created["replication"]["userManaged"]["replicas"][0]["location"],
		"us-central1"
	);
	assert_eq!(created["labels"].as_object().unwrap().len(), 2);
	let writes: Vec<_> = calls
		.iter()
		.filter(|(_, p, _)| p.ends_with(":addVersion"))
		.collect();
	assert_eq!(writes.len(), 1);
	assert_eq!(
		writes[0].2["payload"]["data"],
		base64::engine::general_purpose::STANDARD.encode(key.expose_secret())
	);
	assert!(
		!calls
			.iter()
			.any(|(_, p, _)| p.ends_with("/versions/1:disable")
				|| p.ends_with("/versions/3:destroy"))
	);
	assert!(
		calls
			.iter()
			.any(|(_, p, _)| p.ends_with("/versions/2:disable"))
	);
	assert!(
		calls
			.iter()
			.any(|(_, p, _)| p.ends_with("/versions/2:destroy"))
	);
}
#[tokio::test]
async fn rest_store_rejects_metadata_tokens_without_the_google_response_header() {
	use axum::{Json, Router, http::Uri};
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};
	let store_calls = Arc::new(AtomicUsize::new(0));
	let observed = store_calls.clone();
	// An interposed responder that omits the header must not be trusted as metadata.
	let app = Router::new().fallback(move |uri: Uri| {
		let observed = observed.clone();
		async move {
			if uri.path() != "/token" {
				observed.fetch_add(1, Ordering::SeqCst);
			}
			Json(json!({"access_token":"interposed-token","expires_in":3600}))
		}
	});
	let (url, _server) = serve(app).await;
	let mut store = SecretManager::new("byok-project".into(), "dev".into()).unwrap();
	store.api = format!("{url}/v1");
	store.metadata = format!("{url}/token");
	let error = store.create("alpha", Uuid::now_v7()).await.unwrap_err();
	assert!(matches!(error, Error::External(_)), "{error:?}");
	assert_eq!(store_calls.load(Ordering::SeqCst), 0);
}
