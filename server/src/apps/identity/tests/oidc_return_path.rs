use super::return_path;
use rstest::rstest;

#[rstest]
#[case(None, "/")]
#[case(Some("/"), "/")]
#[case(
	Some("/tenants/alpha?tab=runs#latest"),
	"/tenants/alpha?tab=runs#latest"
)]
#[case(
	Some("/auth/desktop/authorize?request=abc"),
	"/auth/desktop/authorize?request=abc"
)]
fn local_paths_are_returned_unchanged(#[case] value: Option<&str>, #[case] expected: &str) {
	// Act / Assert
	assert_eq!(return_path(value).unwrap(), expected);
}

#[rstest]
#[case("https://attacker.example/")]
#[case("attacker.example")]
#[case("//attacker.example/")]
#[case("/\\attacker.example/")]
// Browsers remove tab and newline characters, turning these into `//host`.
#[case("/\t/attacker.example/")]
#[case("/\n/attacker.example/")]
#[case("/\r/attacker.example/")]
#[case("/\t\t/attacker.example/")]
// Other control characters have no place in a dashboard path either.
#[case("/\u{0}/attacker.example/")]
#[case("/\u{b}/attacker.example/")]
#[case("/\u{c}/attacker.example/")]
#[case("/\u{7f}/attacker.example/")]
#[case("/\u{85}/attacker.example/")]
fn destinations_that_can_leave_the_origin_are_refused(#[case] value: &str) {
	// Act / Assert
	assert!(return_path(Some(value)).is_err());
}

#[rstest]
fn overlong_destinations_are_refused() {
	// Arrange
	let value = format!("/{}", "a".repeat(1024));

	// Act / Assert
	assert!(return_path(Some(&value)).is_err());
}
