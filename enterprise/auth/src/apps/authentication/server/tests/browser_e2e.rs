// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Real Pages + server_fn + OIDC/BFF + PostgreSQL workflows. Only the external
//! GCIP browser module/key/status dependencies are fixtures; product handlers,
//! sessions, cookies, CSRF, migrations and distribution remain production code.
use super::*;
use reinhardt::test::fixtures::wasm::e2e::{BrowserClient, Locator, browser_client};
use std::path::PathBuf;

pub(super) struct BrowserUpstream(pub crate::config::urls::Application);
#[async_trait::async_trait]
impl Handler for BrowserUpstream {
	async fn handle(
		&self,
		request: reinhardt::Request,
	) -> reinhardt::core::exception::Result<reinhardt::Response> {
		if request.uri.path() == "/assets/gcip-sdk.js" {
			let primary = upstream_flow::signed_token(
				"browser-primary",
				Utc::now().timestamp() - 1,
				json!({}),
			);
			let other =
				upstream_flow::signed_token("browser-other", Utc::now().timestamp() - 1, json!({}));
			let source = format!(
				r#"
export async function createClient() {{
 let pending;
 const proof = email => email.startsWith('other') ? {other:?} : {primary:?};
 return {{
  password:async email=>{{ pending=proof(email); if(globalThis.fixtureFactor) throw Object.assign(new Error('factor due'),{{code:globalThis.fixtureFactor==='challenge'?'aidash/second-factor-required':'aidash/enrollment-required'}}); return pending; }},
  popup:async()=>{primary:?},redirectResult:async()=>null,
  register:async()=>true,resend:async()=>true,
  resolveSecondFactor:async()=>pending,
  startEnrollment:async()=>({{secret:'FIXTURESETUPKEY',uri:'otpauth://totp/Aidash:test?secret=FIXTURESETUPKEY'}}),
  finishEnrollment:async()=>pending,close:async()=>{{}}
 }};
}}
"#
			);
			return Ok(reinhardt::Response::ok()
				.with_body(source)
				.with_header("content-type", "text/javascript"));
		}
		self.0.handle(request).await
	}
}

fn assets() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dist")
}
async fn product_fixture() -> Fixture {
	assert!(
		assets().join("index.html").is_file(),
		"build Auth and Marketplace Pages first"
	);
	fixture(|auth| {
		Arc::make_mut(&mut auth.config).assets_dir = Some(assets());
		auth.verifier = Arc::new(aidash_gcp::gcip::TokenVerifier {
			project: "fixture-project".into(),
			keys: Arc::new(upstream_flow::FixtureKeys),
		});
		auth.upstream = Arc::new(upstream_flow::EnabledSource);
	})
	.await
}
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
			"missing {expected} in {selector}; page: {}",
			browser.source().await.unwrap_or_default()
		);
		tokio::time::sleep(Duration::from_millis(25)).await;
	}
}
async fn click(browser: &BrowserClient, label: &str) {
	browser
		.inner()
		.wait()
		.at_most(Duration::from_secs(20))
		.for_element(Locator::XPath(&format!(
			"//button[normalize-space()='{label}']"
		)))
		.await
		.unwrap()
		.click()
		.await
		.unwrap();
}
async fn password(browser: &BrowserClient, email: &str) {
	browser
		.wait_for("input[type=email]:not([disabled])")
		.await
		.unwrap();
	browser.type_into("input[type=email]", email).await.unwrap();
	browser
		.type_into("input[type=password]", "fixture-password")
		.await
		.unwrap();
	browser
		.click("form button[type=submit]:not([disabled])")
		.await
		.unwrap();
}
async fn no_product_tokens(browser: &BrowserClient) {
	let visible = browser
		.execute_js(
			"return {local: {...localStorage}, session: {...sessionStorage}, cookie:document.cookie}",
			vec![],
		)
		.await
		.unwrap()
		.to_string();
	assert!(
		!visible.contains("access_token")
			&& !visible.contains("refresh_token")
			&& !visible.contains("id_token")
	);
	assert!(
		!visible.contains("marketplace-session=") && !visible.contains("aidash-auth="),
		"opaque session cookies must remain HttpOnly"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn pages_e2e_account_publication_and_product_sessions(
	#[future] browser_client: BrowserClient,
) {
	let browser = browser_client.await;
	let fixture = product_fixture().await;
	browser.navigate(&fixture.marketplace_origin).await.unwrap();
	text(&browser, "h1", "Good tools.").await;
	browser.wait_for("a[href='/auth/login']").await.unwrap();
	browser.click("a[href='/auth/login']").await.unwrap();
	text(&browser, "h1", "Sign in to Aidash").await;
	password(&browser, "primary@example.test").await;
	text(&browser, "h1", "Choose your Aidash Account").await;
	click(&browser, "Create a new Aidash Account").await;
	browser
		.wait_for_url(|url| url.as_str().starts_with(&fixture.marketplace_origin))
		.await
		.unwrap();
	browser.wait_for("a[href='/auth/account']").await.unwrap();
	no_product_tokens(&browser).await;
	let cookies = browser.inner().get_all_cookies().await.unwrap();
	assert!(
		cookies.iter().any(
			|cookie| cookie.name() == "marketplace-session" && cookie.http_only() == Some(true)
		)
	);

	// Create and publish through the real typed Pages boundary, without seeding
	// a Publisher or Entitlement response in a browser network mock.
	browser.click("a[href='#/publishers']").await.unwrap();
	browser
		.wait_for(".publisher-create input[name=name]")
		.await
		.unwrap();

	browser
		.type_into(".publisher-create input[name=name]", "Browser Publisher")
		.await
		.unwrap();
	assert_eq!(
		browser
			.execute_js(
				"return document.querySelector('.publisher-create input[name=name]').value",
				vec![]
			)
			.await
			.unwrap(),
		json!("Browser Publisher"),
		"form fields must retain typed values before submission"
	);
	browser
		.click(".publisher-create button[type=submit]")
		.await
		.unwrap();
	text(&browser, "main", "Browser Publisher").await;
	click(&browser, "New plugin listing").await;
	for (name, value) in [
		("name", "Browser Plugin"),
		("slug", "browser-plugin"),
		("description", "Browser publication"),
		("license", "MIT"),
		("origin", "https://example.test/plugin"),
		("permissions", "repository:read"),
	] {
		browser
			.type_into(&format!("form:has(input[name=slug]) [name={name}]"), value)
			.await
			.unwrap();
	}
	browser
		.click("form:has(input[name=slug]) button[type=submit]")
		.await
		.unwrap();
	browser.wait_for("input[name=version]").await.unwrap();
	browser
		.type_into("input[name=version]", "1.0.0")
		.await
		.unwrap();
	let temporary = reinhardt::test::fixtures::temp_dir();
	let bundle = temporary.path().join("bundle.json");
	std::fs::write(&bundle,serde_json::to_vec(&json!({"plugin.json":STANDARD.encode(br#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"browser-plugin"}"#)})).unwrap()).unwrap();
	browser
		.find("input[type=file]")
		.await
		.unwrap()
		.send_keys(bundle.to_str().unwrap())
		.await
		.unwrap();
	browser
		.click("form:has(input[type=file]) button[type=submit]")
		.await
		.unwrap();
	text(
		&browser,
		"[aria-label='Publication result']",
		"Version 1.0.0 published.",
	)
	.await;
	text(&browser, "[aria-label='Publication result']", "sha256:").await;
	browser.click("a[href='#/']").await.unwrap();
	browser
		.wait_for("a[href='#/plugins/browser-plugin']")
		.await
		.unwrap();
	browser
		.click("a[href='#/plugins/browser-plugin']")
		.await
		.unwrap();
	text(&browser, "main", "repository:read").await;
	text(&browser, "main", "MIT").await;
	click(&browser, "Add to Library").await;
	text(&browser, "h1", "Your Library").await;
	browser
		.wait_for("a[download],a[href$='/download']")
		.await
		.unwrap();

	// Link/unlink remain separate fresh-proof steps, owned by Auth. The common
	// Account never supplies Marketplace permissions or a browser token.
	browser.click("a[href='/auth/account']").await.unwrap();
	text(&browser, "h1", "Sign-in methods").await;
	browser
		.wait_for(".identity button[disabled]")
		.await
		.unwrap();
	click(&browser, "Add a sign-in method").await;
	text(&browser, "h1", "Verify your current Account").await;
	password(&browser, "primary@example.test").await;
	text(&browser, "h1", "Verify the new sign-in method").await;
	assert_eq!(browser.execute_js("return Array.from(document.querySelectorAll('input[type=email],input[type=password]')).map(input=>input.value)",vec![]).await.unwrap(),json!(["",""]),"a new proof cannot retain previous credentials");
	password(&browser, "other@example.test").await;
	text(&browser, "h1", "Sign-in methods").await;
	let target = browser
		.inner()
		.find(Locator::XPath(
			"//article[.//code[normalize-space()='browser-other']]//button[normalize-space()='Unlink']",
		))
		.await
		.unwrap();
	target.click().await.unwrap();
	text(&browser, "h1", "Verify another sign-in method").await;
	password(&browser, "primary@example.test").await;
	text(&browser, "h1", "Sign-in methods").await;
	browser
		.wait_for(".identity button[disabled]")
		.await
		.unwrap();
	no_product_tokens(&browser).await;
	browser.click(".global-logout button").await.unwrap();
	text(&browser, "h1", "Signed out everywhere").await;
	assert!(
		!browser
			.inner()
			.get_all_cookies()
			.await
			.unwrap()
			.iter()
			.any(|cookie| cookie.name() == "aidash-auth"),
		"server_fn must propagate the native session-cookie expiration"
	);
	no_product_tokens(&browser).await;
	browser
		.navigate(&format!("{}/#/library", fixture.marketplace_origin))
		.await
		.unwrap();
	browser.wait_for("a[href='/auth/login']").await.unwrap();
	assert!(
		browser
			.find_all("a[href$='/download']")
			.await
			.unwrap()
			.is_empty()
	);
	browser.close().await.unwrap();
}

#[rstest::rstest]
#[case("challenge", "Two-step verification")]
#[case("enroll", "Set up two-step verification")]
#[tokio::test]
async fn pages_e2e_upstream_second_factor(
	#[future] browser_client: BrowserClient,
	#[case] mode: &str,
	#[case] title: &str,
) {
	let browser = browser_client.await;
	let fixture = product_fixture().await;
	browser
		.navigate(&format!("{}/auth/login", fixture.marketplace_origin))
		.await
		.unwrap();
	browser.wait_for("input[type=password]").await.unwrap();
	browser
		.execute_js("globalThis.fixtureFactor=arguments[0]", vec![json!(mode)])
		.await
		.unwrap();
	password(&browser, "primary@example.test").await;
	text(&browser, "h2", title).await;
	if mode == "enroll" {
		text(&browser, "main", "FIXTURESETUPKEY").await;
	}
	browser
		.type_into("input[autocomplete=one-time-code]", "123456")
		.await
		.unwrap();
	browser.click("form button[type=submit]").await.unwrap();
	text(&browser, "h1", "Choose your Aidash Account").await;
	no_product_tokens(&browser).await;
	browser.close().await.unwrap();
}
