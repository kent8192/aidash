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
  if path=="/token" { assert_eq!(headers["metadata-flavor"],"Google"); return Json(json!({"access_token":"metadata-token","expires_in":3600})); }
  assert_eq!(headers["authorization"],"Bearer metadata-token");
  let body:Value=if body.is_empty(){Value::Null}else{serde_json::from_slice(&body).unwrap()};
  history.lock().unwrap().push((method.to_string(),path.clone(),body));
  if path.contains(":addVersion") {Json(json!({"name":format!("{remote}/versions/2")}))}
  else if path.contains("pageToken="){Json(json!({"versions":[{"name":format!("{remote}/versions/3"),"state":"DESTROYED"}]}))}
  else if path.contains("?pageSize="){Json(json!({"versions":[{"name":format!("{remote}/versions/1"),"state":"DISABLED"},{"name":format!("{remote}/versions/2"),"state":"ENABLED"}],"nextPageToken":"next page"}))}
  else if method==Method::GET && path.ends_with("/versions/1"){Json(json!({"state":"DISABLED"}))}
  else if method==Method::GET && path.ends_with("/versions/3"){Json(json!({"state":"DESTROYED"}))}
  else {Json(json!({"state":"ENABLED"}))}
 }});
	let (url, _server) = serve(app).await;
	let mut store = SecretManager::new("byok-project".into(), "dev".into()).unwrap();
	store.api = format!("{url}/v1");
	store.metadata = format!("{url}/token");
	store.create("alpha", id).await.unwrap();
	let key: SecretString = "canary-provider-write-only-key".into();
	let version = store.add_version("alpha", &resource, &key).await.unwrap();
	assert_eq!(version, format!("{resource}/versions/2"));
	assert_eq!(store.versions(&resource).await.unwrap().len(), 2);
	store.disable(&version).await.unwrap();
	store
		.disable(&format!("{resource}/versions/1"))
		.await
		.unwrap();
	store.destroy(&version).await.unwrap();
	store
		.destroy(&format!("{resource}/versions/3"))
		.await
		.unwrap();
	store.delete(&resource).await.unwrap();
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
#[rstest::rstest]
#[case(200, Some(10), false, false)]
#[case(200, None, false, true)]
#[case(401, None, true, false)]
#[case(403, None, true, false)]
#[case(503, None, true, false)]
#[case(429, None, true, false)]
#[tokio::test]
async fn validation_classifies_rejections_and_never_returns_upstream_key_material(
	#[case] status: u16,
	#[case] limit: Option<u64>,
	#[case] rejected: bool,
	#[case] warned: bool,
) {
	use axum::{
		Json, Router,
		http::{HeaderMap, StatusCode},
		routing::get,
	};
	let app = Router::new().route(
		"/key",
		get(move |headers: HeaderMap| async move {
			assert_eq!(headers["authorization"], "Bearer canary-key-for-validation");
			(
				StatusCode::from_u16(status).unwrap(),
				Json(json!({"data":{"limit":limit},"error":"canary-key-for-validation"})),
			)
		}),
	);
	let (url, _server) = serve(app).await;
	let validator = OpenRouterKeyValidator {
		client: reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.build()
			.unwrap(),
	};
	let result = validator
		.validate_at(
			&format!("{url}/key"),
			&SecretString::from("canary-key-for-validation"),
		)
		.await;
	assert_eq!(result.is_err(), rejected);
	match result {
		Ok(v) => assert_eq!(!v.warnings.is_empty(), warned),
		Err(e) => {
			assert!(!e.to_string().contains("canary-key-for-validation"));
			if status == 503 || status == 429 {
				assert!(matches!(e, Error::ProviderRejected { status: 503, .. }));
			} else {
				assert!(matches!(e, Error::Invalid(_)));
			}
		}
	}
}
