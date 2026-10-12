use super::*;
use rstest::rstest;

#[rstest]
#[case("https://example.com/page", true)]
#[case("http://example.com:80/", true)]
#[case("https://93.184.216.34/", true)]
#[case("https://example.com:8443/", false)]
#[case("ftp://example.com/", false)]
#[case("https://user:secret@example.com/", false)]
#[case("http://localhost/", false)]
#[case("http://api.localhost/", false)]
#[case("http://printer.local/", false)]
#[case("http://intranet/", false)]
#[case("http://127.0.0.1/", false)]
#[case("http://10.0.0.1/", false)]
#[case("http://169.254.169.254/latest/meta-data", false)]
#[case("http://[::1]/", false)]
#[case("http://[fd00::1]/", false)]
#[case("http://[fe80::1]/", false)]
#[case("http://[::ffff:127.0.0.1]/", false)]
#[case("http://[::ffff:93.184.216.34]/", false)]
fn literal_destinations_must_be_public(#[case] url: &str, #[case] admitted: bool) {
	assert_eq!(
		admit_public_url(&Url::parse(url).unwrap()).is_ok(),
		admitted,
		"{url}"
	);
}
