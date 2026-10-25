use super::*;
use axum::{Router, http::StatusCode, routing::post};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rstest::rstest;
use serde_json::json;

fn artifact(manifest_version: Option<&str>) -> Artifact {
	let mut manifest =
		json!({"$schema":aidash_domain::agent_plugin::PLUGIN_SCHEMA,"name":"example"});
	if let Some(version) = manifest_version {
		manifest["version"] = json!(version);
	}
	let files = vec![PluginFile {
		path: "plugin.json".into(),
		bytes: serde_json::to_vec(&manifest).unwrap(),
	}];
	Artifact {
		entitlement_expires_at: None,
		plugin_id: uuid::Uuid::from_u128(1),
		version: "1.0.0".into(),
		digest: aidash_domain::agent_plugin::tree_digest(&files),
		files: files
			.into_iter()
			.map(|file| (file.path, STANDARD.encode(file.bytes)))
			.collect(),
	}
}

// Real HTTP proves authoritative denials, wrong bindings and content integrity
// cannot produce a local import. The server does not implement client validation.
#[rstest]
#[case::accepted(200, false, false, Some("1.0.0"), true)]
#[case::versionless(200, false, false, None, true)]
#[case::manifest_mismatch(200, false, false, Some("2.0.0"), false)]
#[case::entitlement_denied(403, false, false, Some("1.0.0"), false)]
#[case::digest_changed(200, true, false, Some("1.0.0"), false)]
#[case::wrong_plugin(200, false, true, Some("1.0.0"), false)]
#[case::redirect(302, false, false, Some("1.0.0"), false)]
#[tokio::test]
async fn only_authorized_exact_artifacts_reach_import(
	#[case] status: u16,
	#[case] bad_digest: bool,
	#[case] bad_plugin: bool,
	#[case] manifest_version: Option<&str>,
	#[case] accepted: bool,
) {
	let mut value = artifact(manifest_version);
	if bad_digest {
		value.digest = format!("sha256:{}", "0".repeat(64));
	}
	if bad_plugin {
		value.plugin_id = uuid::Uuid::from_u128(2);
	}
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let router = Router::new().route(
		"/api/marketplace/downloads",
		post(
			move |headers: axum::http::HeaderMap,
			      axum::Json(body): axum::Json<serde_json::Value>| {
				let value = value.clone();
				async move {
					assert_eq!(headers["authorization"], "Bearer connection-secret");
					assert_eq!(
						body["owner_account_id"],
						uuid::Uuid::from_u128(10).to_string()
					);
					(StatusCode::from_u16(status).unwrap(), axum::Json(value))
				}
			},
		),
	);
	let task = tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap();
	});
	let secret = tempfile::NamedTempFile::new().unwrap();
	std::fs::write(secret.path(), "connection-secret\n").unwrap();
	let client = Client::new(
		&endpoint,
		Connection {
			connection_id: uuid::Uuid::from_u128(9),
			owner_account_id: uuid::Uuid::from_u128(10),
			owner_tenant_id: None,
			secret_file: secret.path().to_owned(),
		},
	)
	.unwrap();
	let result = client
		.fetch(PluginImportRequest {
			url: "marketplace://00000000-0000-0000-0000-000000000001/1.0.0".into(),
			plugin_path: None,
		})
		.await;
	assert_eq!(result.is_ok(), accepted, "{result:?}");
	task.abort();
}
