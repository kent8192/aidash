use super::*;

fn resource_metadata(url: &str) -> Option<Url> {
	Some(Url::parse(url).unwrap())
}

#[test]
fn a_401_without_a_header_still_means_the_server_needs_a_connection() {
	assert_eq!(
		parse_challenge(401, None),
		Challenge::Unauthorized {
			resource_metadata: None,
			scope: None,
		}
	);
}

#[test]
fn a_401_with_another_scheme_has_no_bearer_parameters() {
	assert_eq!(
		parse_challenge(401, Some(r#"Basic realm="mcp""#)),
		Challenge::Unauthorized {
			resource_metadata: None,
			scope: None,
		}
	);
}

#[test]
fn a_bearer_challenge_carries_resource_metadata_and_scope() {
	assert_eq!(
		parse_challenge(
			401,
			Some(
				r#"Bearer realm="mcp", resource_metadata="https://as.example/.well-known/oauth-protected-resource/mcp", scope="files:read files:write""#
			)
		),
		Challenge::Unauthorized {
			resource_metadata: resource_metadata(
				"https://as.example/.well-known/oauth-protected-resource/mcp"
			),
			scope: Some("files:read files:write".into()),
		}
	);
}

#[test]
fn quoted_commas_and_escaped_quotes_stay_in_one_parameter() {
	assert_eq!(
		parse_challenge(
			403,
			Some(r#"Bearer realm="one, two", error="insufficient_scope", scope="files:\"read\"""#)
		),
		Challenge::InsufficientScope {
			scope: Some(r#"files:"read""#.into()),
		}
	);
}

#[test]
fn a_comma_inside_a_quoted_resource_metadata_url_does_not_split_the_challenge() {
	assert_eq!(
		parse_challenge(
			401,
			Some(r#"Bearer resource_metadata="https://as.example/prm,tenant", scope="files:read""#)
		),
		Challenge::Unauthorized {
			resource_metadata: resource_metadata("https://as.example/prm,tenant"),
			scope: Some("files:read".into()),
		}
	);
}

#[test]
fn a_bearer_challenge_after_another_scheme_is_found() {
	assert_eq!(
		parse_challenge(
			401,
			Some(r#"Basic realm="one", Bearer resource_metadata="https://as.example/prm""#)
		),
		Challenge::Unauthorized {
			resource_metadata: resource_metadata("https://as.example/prm"),
			scope: None,
		}
	);
}

#[test]
fn a_bearer_challenge_before_another_scheme_is_not_polluted_by_it() {
	assert_eq!(
		parse_challenge(
			401,
			Some(r#"Bearer error="invalid_token", Basic realm="one""#)
		),
		Challenge::Unauthorized {
			resource_metadata: None,
			scope: None,
		}
	);
}

#[test]
fn scheme_and_parameter_names_are_case_insensitive() {
	assert_eq!(
		parse_challenge(
			401,
			Some(r#"bEaReR ReSoUrCe_MeTaDaTa="https://as.example/prm", SCOPE="files:read""#)
		),
		Challenge::Unauthorized {
			resource_metadata: resource_metadata("https://as.example/prm"),
			scope: Some("files:read".into()),
		}
	);
}

#[test]
fn unquoted_token_values_are_accepted() {
	assert_eq!(
		parse_challenge(403, Some("Bearer error=insufficient_scope")),
		Challenge::InsufficientScope { scope: None }
	);
}

#[test]
fn a_403_with_insufficient_scope_asks_for_consent_again() {
	assert_eq!(
		parse_challenge(
			403,
			Some(r#"Bearer error="insufficient_scope", scope="files:write""#)
		),
		Challenge::InsufficientScope {
			scope: Some("files:write".into()),
		}
	);
	assert_eq!(
		parse_challenge(403, Some(r#"Bearer error="insufficient_scope""#)),
		Challenge::InsufficientScope { scope: None }
	);
}

#[test]
fn a_403_for_another_reason_is_not_an_insufficient_scope_challenge() {
	for header in [
		r#"Bearer error="invalid_token", scope="files:read""#,
		r#"Basic realm="mcp""#,
		"",
	] {
		assert_eq!(
			parse_challenge(403, Some(header)),
			Challenge::None,
			"{header}"
		);
	}
	assert_eq!(parse_challenge(403, None), Challenge::None);
}

#[test]
fn other_statuses_carry_no_challenge() {
	for status in [200, 201, 302, 404, 500] {
		assert_eq!(
			parse_challenge(
				status,
				Some(r#"Bearer resource_metadata="https://as.example/prm""#)
			),
			Challenge::None,
			"{status}"
		);
	}
}

#[test]
fn an_invalid_resource_metadata_url_is_dropped_but_the_scope_survives() {
	assert_eq!(
		parse_challenge(
			401,
			Some(r#"Bearer resource_metadata="not a url", scope="files:read""#)
		),
		Challenge::Unauthorized {
			resource_metadata: None,
			scope: Some("files:read".into()),
		}
	);
}

#[test]
fn an_empty_parameter_is_absent() {
	assert_eq!(
		parse_challenge(401, Some(r#"Bearer resource_metadata="", scope="""#)),
		Challenge::Unauthorized {
			resource_metadata: None,
			scope: None,
		}
	);
}

#[test]
fn a_malformed_header_never_panics() {
	for header in [
		"Bearer",
		"Bearer =",
		"Bearer scope=\"unterminated",
		"Bearer scope=",
		"\"unexpected\"",
		",,,",
		"Bearer \u{00e9}",
	] {
		assert_eq!(
			parse_challenge(401, Some(header)),
			Challenge::Unauthorized {
				resource_metadata: None,
				scope: None,
			},
			"{header}"
		);
	}
}
