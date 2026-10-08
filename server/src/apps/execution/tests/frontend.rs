use crate::endpoint::{EndpointFixture, endpoint};
use aidash_server::config::settings::ProjectSettings;
use reinhardt::test::fixtures::temp_dir;
use rstest::{fixture, rstest};
use serde_json::json;
use tempfile::TempDir;

const INDEX: &str = "<!doctype html><title>Aidash fixture</title>";
const SCRIPT: &str = "console.log('native frontend');";

struct FrontendFixture {
	app: EndpointFixture,
	_directory: TempDir,
}

#[fixture]
fn frontend(
	#[future] endpoint: EndpointFixture,
	temp_dir: TempDir,
) -> impl Future<Output = FrontendFixture> {
	let endpoint = Box::pin(endpoint);
	Box::pin(async move {
		let app = endpoint.await;
		let public = temp_dir.path().join("public");
		std::fs::create_dir_all(public.join("assets")).unwrap();
		std::fs::write(public.join("index.html"), INDEX).unwrap();
		std::fs::write(public.join("assets/app.js"), SCRIPT).unwrap();
		std::fs::write(public.join(".private"), "hidden fixture").unwrap();
		std::fs::write(temp_dir.path().join("private.txt"), "outside fixture").unwrap();
		#[cfg(unix)]
		{
			std::os::unix::fs::symlink(
				temp_dir.path().join("private.txt"),
				public.join("outside.txt"),
			)
			.unwrap();
			std::os::unix::fs::symlink(temp_dir.path(), public.join("escape")).unwrap();
		}
		let mut settings = app
			.context
			.get_singleton::<ProjectSettings>()
			.unwrap()
			.as_ref()
			.clone();
		settings.node.web_dir = public.to_string_lossy().into_owned();
		app.context.set_singleton(settings);
		FrontendFixture {
			app,
			_directory: temp_dir,
		}
	})
}

#[rstest]
#[tokio::test]
async fn native_frontend_serves_assets_shell_routes_head_and_conditional_requests(
	#[future] frontend: FrontendFixture,
) {
	// Arrange
	let fixture = Box::pin(frontend).await;
	let client = &fixture.app.anonymous;
	// Act / Assert
	for path in [
		"/",
		"/workspaces/fixture",
		"/workspaces/fixture?view=threads",
	] {
		let response = client.get(path).await.unwrap();
		assert_eq!(response.status_code(), 200, "{path}: {}", response.text());
		assert_eq!(response.content_type(), Some("text/html"));
		assert_eq!(response.text(), INDEX);
	}
	let script = client.get("/assets/app.js").await.unwrap();
	assert_eq!(script.status_code(), 200);
	assert!(matches!(
		script.content_type(),
		Some("text/javascript" | "application/javascript")
	));
	assert_eq!(script.text(), SCRIPT);
	let head = client.head("/assets/app.js").await.unwrap();
	assert_eq!(head.status_code(), 200);
	assert!(head.body().is_empty());
	assert_eq!(
		head.header("content-length"),
		Some(SCRIPT.len().to_string().as_str())
	);
	assert_eq!(head.header("etag"), script.header("etag"));
	let cached = client
		.get_with_headers(
			"/assets/app.js",
			&[("If-None-Match", script.header("etag").unwrap())],
		)
		.await
		.unwrap();
	assert_eq!(cached.status_code(), 304);
	assert!(cached.body().is_empty());
}

#[rstest]
#[tokio::test]
async fn frontend_fallback_preserves_api_boundaries_and_missing_asset_errors(
	#[future] frontend: FrontendFixture,
) {
	// Arrange
	let fixture = Box::pin(frontend).await;
	let client = &fixture.app.anonymous;
	// Act / Assert
	for path in [
		"/api/no-such-route",
		"/auth/no-such-route",
		"/federation/unknown",
		"/assets/missing.js",
		"/assets/missing",
		"/.private",
		"/%2e%2e%2fprivate.txt",
	] {
		let response = client.get(path).await.unwrap();
		assert_eq!(response.status_code(), 404, "{path}: {}", response.text());
		assert!(
			!response.text().contains("fixture"),
			"no shell or private file for {path}"
		);
	}
	let response = client
		.post("/workspaces/fixture", &json!({}), "json")
		.await
		.unwrap();
	assert_eq!(response.status_code(), 405);
	let api = client.get("/api/state").await.unwrap();
	assert_eq!(
		api.status_code(),
		401,
		"registered APIs retain their authentication middleware"
	);
}

#[cfg(unix)]
#[rstest]
#[tokio::test]
async fn frontend_storage_cannot_follow_file_or_directory_links_outside_its_root(
	#[future] frontend: FrontendFixture,
) {
	// Arrange
	let fixture = Box::pin(frontend).await;
	// Act / Assert
	for path in ["/outside.txt", "/escape/private.txt"] {
		let response = fixture.app.anonymous.get(path).await.unwrap();
		assert_eq!(response.status_code(), 404, "{path}: {}", response.text());
		assert!(!response.text().contains("outside fixture"));
	}
}
