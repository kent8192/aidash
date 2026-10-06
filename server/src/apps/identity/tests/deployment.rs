use crate::manage::{ManageFixture, native_server};
use rstest::rstest;

#[rstest]
#[tokio::test]
async fn packaged_oidc_profile_enables_dashboard_sign_in(
	#[future]
	#[with("oidc")]
	native_server: ManageFixture,
) {
	// Arrange
	let mut app = native_server.await;
	// Act
	let response = app.client.get("/auth/config").await.unwrap();
	// Assert
	assert_eq!(response.status_code(), 200, "{}", response.text());
	let configuration = response.json_value().unwrap();
	assert_eq!(configuration["enabled"], true);
	assert!(!response.text().contains("fixture-dashboard-secret"));
	assert!(!response.text().contains("fixture-status-secret"));
	app.shutdown().await;
}
