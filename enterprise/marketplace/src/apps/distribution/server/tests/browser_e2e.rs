//! The browser exercises actual Pages/server_fn and the persistence-backed
//! Product Session. Only the remote auth status service is the parent fixture.
use reinhardt::test::fixtures::wasm::e2e::{BrowserClient, browser_client, browser_config};
use std::time::Duration;
async fn text(browser: &BrowserClient, selector: &str, expected: &str) {
	let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
	loop {
		if browser
			.get_text(selector)
			.await
			.is_ok_and(|value| value.contains(expected))
		{
			return;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"missing {expected}; page: {}",
			browser.source().await.unwrap_or_default()
		);
		tokio::time::sleep(Duration::from_millis(25)).await;
	}
}
pub(super) async fn acquire(
	endpoint: &str,
	slug: &str,
	plugin: &str,
	token: &str,
) -> BrowserClient {
	let browser = browser_client(browser_config()).await;
	browser.navigate(endpoint).await.unwrap();
	browser
		.wait_for(&format!("a[href='#/plugins/{slug}']"))
		.await
		.unwrap();
	browser
		.click(&format!("a[href='#/plugins/{slug}']"))
		.await
		.unwrap();
	text(&browser, "main", "HTTP Publisher").await;
	text(&browser, "main", "MIT").await;
	text(&browser, "main", "https://example.com/plugin").await;
	browser.wait_for("a[href^='/auth/login']").await.unwrap();
	// The browser's opaque cookie is seeded through WebDriver, retaining HttpOnly;
	// no JS token store or network mock stands in for the Product Session.
	browser
		.inner()
		.add_cookie(
			cookie::Cookie::build(("marketplace-session", token.to_owned()))
				.domain(
					url::Url::parse(endpoint)
						.unwrap()
						.host_str()
						.unwrap()
						.to_owned(),
				)
				.path("/")
				.http_only(true)
				.same_site(cookie::SameSite::Lax)
				.build(),
		)
		.await
		.unwrap();
	browser
		.navigate(&format!("{endpoint}/#/plugins/{slug}"))
		.await
		.unwrap();
	browser.wait_for("a[href='/auth/account']").await.unwrap();
	browser
		.inner()
		.wait()
		.at_most(Duration::from_secs(20))
		.for_element(reinhardt::test::fixtures::wasm::e2e::Locator::XPath(
			"//button[normalize-space()='Add to Library']",
		))
		.await
		.unwrap()
		.click()
		.await
		.unwrap();
	text(&browser, "h1", "Your Library").await;
	let download = format!("/api/marketplace/plugins/{plugin}/versions/1.0.0/download");
	browser
		.wait_for(&format!("a[href='{download}']"))
		.await
		.unwrap();
	let visible = browser
		.execute_js(
			"return {cookie:document.cookie,local:{...localStorage},session:{...sessionStorage}}",
			vec![],
		)
		.await
		.unwrap()
		.to_string();
	assert!(
		!visible.contains(token)
			&& !visible.contains("fixture-access")
			&& !visible.contains("access_token")
			&& !visible.contains("refresh_token")
	);
	browser
}
pub(super) async fn recalled(browser: &BrowserClient, endpoint: &str, plugin: &str) {
	browser
		.navigate(&format!("{endpoint}/#/library"))
		.await
		.unwrap();
	text(browser, "main", "Recalled").await;
	text(browser, "main", "unsafe version").await;
	assert!(
		browser
			.find_all(&format!(
				"a[href='/api/marketplace/plugins/{plugin}/versions/1.0.0/download']"
			))
			.await
			.unwrap()
			.is_empty()
	);
	// Recall applies to one version. The later, unrecalled publication remains
	// available under the same Entitlement.
	browser
		.wait_for(&format!(
			"a[href='/api/marketplace/plugins/{plugin}/versions/3.0.0/download']"
		))
		.await
		.unwrap();
}
pub(super) async fn stopped(browser: BrowserClient, endpoint: &str) {
	browser
		.navigate(&format!("{endpoint}/#/library"))
		.await
		.unwrap();
	browser.wait_for("[role=alert]").await.unwrap();
	assert!(
		browser
			.find_all("a[href$='/download']")
			.await
			.unwrap()
			.is_empty()
	);
	browser.close().await.unwrap();
}

/// Hostile markup in every field a Publisher or Aidash Recall controls. Each
/// payload would execute or create an element if the Pages UI ever treated
/// publisher text as markup instead of as a text node.
const HOSTILE: &str = r#"<img src=x onerror="window.__xss=1"> <script>window.__xss=1</script> "><svg onload=window.__xss=1>"#;

/// Waits until `main` renders every expected string as DOM text. `textContent`
/// is used instead of rendered text because CSS (for example `text-transform`)
/// must not hide a missing or altered payload.
async fn dom_text(browser: &BrowserClient, expected: &[String]) {
	let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
	loop {
		let content = browser
			.execute_js(
				"return document.querySelector('main')?.textContent ?? ''",
				vec![],
			)
			.await
			.unwrap();
		let content = content.as_str().unwrap_or_default();
		if expected.iter().all(|value| content.contains(value)) {
			return;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"missing hostile text {expected:?}; main: {content}"
		);
		tokio::time::sleep(Duration::from_millis(25)).await;
	}
}

/// Loads `route` in a fresh document so session and catalog resources refetch.
async fn visit(browser: &BrowserClient, endpoint: &str, route: &str, expected: &[String]) {
	browser
		.navigate(&format!("{endpoint}/#{route}"))
		.await
		.unwrap();
	browser.inner().refresh().await.unwrap();
	dom_text(browser, expected).await;
	// An `onerror` handler fires only after the image request fails, so give any
	// injected element time to run before asserting that nothing executed.
	tokio::time::sleep(Duration::from_millis(500)).await;
	let injected = browser
		.execute_js(
			"return document.querySelector('img[src=\"x\"], svg[onload], script:not([src]), [onerror], [onload]')\
			 ?.outerHTML ?? null",
			vec![],
		)
		.await
		.unwrap();
	assert!(
		injected.is_null(),
		"{route} created an element from publisher text: {injected}"
	);
	let executed = browser
		.execute_js("return typeof window.__xss", vec![])
		.await
		.unwrap();
	assert_eq!(executed, "undefined", "{route} executed publisher text");
}

#[tokio::test]
async fn publisher_controlled_text_is_rendered_as_text() {
	use crate::auth::{Session as ProductSession, hash};
	use aidash_domain::agent_plugin::{PluginFile, tree_digest};
	use base64::{Engine as _, engine::general_purpose::STANDARD};
	use serde_json::json as value;
	use std::sync::{Arc, atomic::AtomicBool};
	use uuid::Uuid;

	let super::ProvisionedDatabase {
		app_url,
		_container,
		..
	} = super::provisioned_database().await;

	let account = Uuid::new_v4();
	let identity = Uuid::new_v4();
	let (issuer, auth_task) = super::server(super::Status {
		account,
		identity,
		active: Arc::new(AtomicBool::new(true)),
	})
	.await;
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	let service = crate::Marketplace::connect(super::config(&origin, &issuer), &app_url)
		.await
		.unwrap();
	let token = crate::auth::secret();
	let csrf = crate::auth::secret();
	let session = ProductSession {
		account_id: account,
		source_identity_id: identity,
		provider_subject: account.to_string(),
		csrf_token: csrf.clone(),
		refresh_request_id: Uuid::new_v4(),
		access_token: "fixture-access".into(),
		refresh_token: None,
		created_at: chrono::Utc::now(),
		last_used: chrono::Utc::now(),
	};
	service
		.store
		.connection
		.atomic(async |tx| {
			crate::store::put(
				tx,
				&format!("session:{}", hash(&token)),
				"session",
				&session,
			)
			.await
		})
		.await
		.unwrap();
	let (endpoint, task) =
		super::server_on(listener, crate::config::urls::handler(service.clone())).await;
	let client = reqwest::Client::new();
	let post = |path: &str, body: serde_json::Value| {
		client
			.post(format!("{endpoint}{path}"))
			.header("cookie", format!("marketplace-session={token}"))
			.header("origin", &origin)
			.header("x-marketplace-csrf", &csrf)
			.json(&body)
	};

	// Every free-text field the server accepts carries the payloads. The plugin
	// slug (lowercase ASCII, digits and hyphens), the version (semantic version)
	// and the origin scheme (HTTPS URL) are validated and cannot carry markup
	// in those positions; the origin's path and query still can.
	let publisher_name = format!("publisher:{HOSTILE}");
	let plugin_name = format!("name:{HOSTILE}");
	let description = format!("description:{HOSTILE}");
	let license = format!("license:{HOSTILE}");
	let permission = format!("permission:{HOSTILE}");
	let plugin_origin = format!("https://example.com/plugin?origin={HOSTILE}");
	let reason = format!("reason:{HOSTILE}");
	let slug = format!("xss-{}", Uuid::new_v4().simple());
	let publisher: serde_json::Value = post(
		"/api/server_fn/marketplace_create_publisher",
		value!({"name":publisher_name,"csrf":csrf}),
	)
	.send()
	.await
	.unwrap()
	.json()
	.await
	.unwrap();
	let publisher = publisher["publisher"]["id"].as_str().unwrap();
	let created = post(
		"/api/marketplace/plugins",
		value!({"publisher_id":publisher,"slug":slug,"name":plugin_name,"description":description,"price_cents":0,"currency":"USD","declared_permissions":[permission],"license":license,"origin":plugin_origin}),
	)
	.send()
	.await
	.unwrap();
	assert_eq!(
		created.status(),
		200,
		"hostile text must be accepted as data"
	);
	let created: serde_json::Value = created.json().await.unwrap();
	let plugin = created["plugin"]["id"].as_str().unwrap();
	let manifest = br#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"hostile-plugin"}"#;
	let files = [PluginFile {
		path: "plugin.json".into(),
		bytes: manifest.to_vec(),
	}];
	for version in ["1.0.0", "2.0.0"] {
		let published = post(
			&format!("/api/marketplace/plugins/{plugin}/versions"),
			value!({"version":version,"files":{"plugin.json":STANDARD.encode(manifest)},"expected_sha256":tree_digest(&files)}),
		)
		.send()
		.await
		.unwrap();
		assert_eq!(
			published.status(),
			200,
			"{}",
			published.text().await.unwrap()
		);
	}
	assert_eq!(
		post(
			&format!("/api/marketplace/plugins/{plugin}/acquire"),
			value!({})
		)
		.send()
		.await
		.unwrap()
		.status(),
		200
	);
	assert_eq!(
		client
			.post(format!("{endpoint}/internal/recalls"))
			.bearer_auth("test-recall-secret-32-characters-long")
			.json(&value!({"plugin_id":plugin,"version":"1.0.0","reason":reason}))
			.send()
			.await
			.unwrap()
			.status(),
		200
	);

	let browser = browser_client(browser_config()).await;
	browser.navigate(&endpoint).await.unwrap();
	browser
		.inner()
		.add_cookie(
			cookie::Cookie::build(("marketplace-session", token.clone()))
				.domain(
					url::Url::parse(&endpoint)
						.unwrap()
						.host_str()
						.unwrap()
						.to_owned(),
				)
				.path("/")
				.http_only(true)
				.same_site(cookie::SameSite::Lax)
				.build(),
		)
		.await
		.unwrap();
	// Catalog: Publisher, name, description and license are listed publicly.
	visit(
		&browser,
		&endpoint,
		"/",
		&[
			publisher_name.clone(),
			plugin_name.clone(),
			description.clone(),
			license.clone(),
		],
	)
	.await;
	// Detail: also the origin, declared permissions and the recall reason.
	visit(
		&browser,
		&endpoint,
		&format!("/plugins/{slug}"),
		&[
			publisher_name.clone(),
			plugin_name.clone(),
			description.clone(),
			license.clone(),
			permission.clone(),
			plugin_origin.clone(),
			reason.clone(),
		],
	)
	.await;
	// Library of the Account that acquired it, listing the recalled version.
	visit(
		&browser,
		&endpoint,
		"/library",
		&[plugin_name.clone(), reason.clone()],
	)
	.await;
	// Publisher tools list the same Publisher and plugin names back to their owner.
	visit(
		&browser,
		&endpoint,
		"/publishers",
		&[publisher_name.clone(), plugin_name.clone()],
	)
	.await;
	browser.close().await.unwrap();
	auth_task.abort();
	task.abort();
}
