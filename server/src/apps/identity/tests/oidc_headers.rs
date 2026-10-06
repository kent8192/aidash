use super::cookie_value;
use http::{HeaderValue, header};
use reinhardt::Request;
use rstest::rstest;

#[rstest]
#[case("aidash-session")]
#[case("__Host-aidash-session")]
#[case("aidash-login")]
#[case("__Host-aidash-login")]
#[case("aidash-csrf")]
fn credentials_in_later_cookie_fields_are_available(#[case] name: &str) {
	// Arrange: a proxy separates unrelated cookies from Aidash credentials.
	let mut request = Request::builder().uri("/auth/callback").build().unwrap();
	request.headers.append(
		header::COOKIE,
		HeaderValue::from_static("theme=dark; unrelated=first"),
	);
	request.headers.append(
		header::COOKIE,
		HeaderValue::from_str(&format!("other=next; {name}=credential=with-equals")).unwrap(),
	);

	// Act / Assert: preserve the entire opaque value from the later field.
	assert_eq!(
		cookie_value(&request.headers, name),
		Some("credential=with-equals")
	);
}

#[rstest]
fn invalid_cookie_fields_do_not_hide_a_later_valid_credential() {
	let mut request = Request::builder().uri("/auth/session").build().unwrap();
	request
		.headers
		.append(header::COOKIE, HeaderValue::from_bytes(b"\xff").unwrap());
	request.headers.append(
		header::COOKIE,
		HeaderValue::from_static("aidash-session=valid"),
	);

	assert_eq!(
		cookie_value(&request.headers, "aidash-session"),
		Some("valid")
	);
}

#[rstest]
fn duplicate_cookie_names_keep_the_first_value_and_missing_names_stay_absent() {
	let mut request = Request::builder().uri("/auth/logout").build().unwrap();
	request.headers.append(
		header::COOKIE,
		HeaderValue::from_static("broken; aidash-session=first"),
	);
	request.headers.append(
		header::COOKIE,
		HeaderValue::from_static("aidash-session=second; unrelated=value"),
	);

	assert_eq!(
		cookie_value(&request.headers, "aidash-session"),
		Some("first")
	);
	assert_eq!(cookie_value(&request.headers, "aidash-login"), None);
}
