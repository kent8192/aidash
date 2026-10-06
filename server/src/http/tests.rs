use super::{mark_sensitive_headers, private_response};
use http::{HeaderValue, header};
use reinhardt::{Request, Response};
use rstest::rstest;

#[rstest]
#[case(true)]
#[case(false)]
fn every_response_cookie_is_redacted_without_changing_its_wire_value(#[case] private: bool) {
	// Arrange: the callback sets session, CSRF and login cookies independently.
	let cookies = [
		"aidash-session=session-secret; HttpOnly",
		"aidash-csrf=csrf-secret",
		"aidash-login=; Max-Age=0",
	];
	let mut response = Response::ok();
	for cookie in cookies {
		response
			.headers
			.append(header::SET_COOKIE, HeaderValue::from_static(cookie));
	}
	response
		.headers
		.insert(header::LOCATION, HeaderValue::from_static("/dashboard"));

	// Act
	let response = private_response(response, "00000000-0000-0000-0000-000000000000", private);

	// Assert: every cookie remains usable on the wire and is hidden from Debug.
	let values: Vec<_> = response
		.headers
		.get_all(header::SET_COOKIE)
		.iter()
		.collect();
	assert_eq!(
		values
			.iter()
			.map(|value| value.to_str().unwrap())
			.collect::<Vec<_>>(),
		cookies
	);
	assert!(values.iter().all(|value| value.is_sensitive()));
	let diagnostic = format!("{:?}", response.headers);
	assert!(!diagnostic.contains("session-secret"));
	assert!(!diagnostic.contains("csrf-secret"));
	assert_eq!(response.headers[header::LOCATION], "/dashboard");
	assert!(!response.headers[header::LOCATION].is_sensitive());
}

#[rstest]
fn every_request_credential_field_is_redacted() {
	let names = ["authorization", "cookie", "x-aidash-csrf"];
	let mut request = Request::builder().uri("/auth/logout").build().unwrap();
	for name in names {
		request
			.headers
			.append(name, HeaderValue::from_static("first-secret"));
		request
			.headers
			.append(name, HeaderValue::from_static("second-secret"));
	}
	request.headers.insert(
		header::CONTENT_TYPE,
		HeaderValue::from_static("application/json"),
	);

	mark_sensitive_headers(&mut request.headers, &names);

	for name in names {
		let values: Vec<_> = request.headers.get_all(name).iter().collect();
		assert_eq!(values.len(), 2);
		assert!(values.iter().all(|value| value.is_sensitive()));
	}
	let diagnostic = format!("{:?}", request.headers);
	assert!(!diagnostic.contains("first-secret"));
	assert!(!diagnostic.contains("second-secret"));
	assert!(!request.headers[header::CONTENT_TYPE].is_sensitive());
}
