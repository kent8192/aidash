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
