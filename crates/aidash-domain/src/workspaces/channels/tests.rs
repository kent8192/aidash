use super::*;
use rstest::rstest;

#[rstest]
#[case("../hidden", "text/plain", b"x".as_slice(), "invalid attachment filename")]
#[case("evidence.txt", "text/plain\r\nInjected: yes", b"x".as_slice(), "invalid attachment media type")]
#[case("evidence.txt", "plain", b"x".as_slice(), "invalid attachment media type")]
#[case("evidence.txt", "text/plain", b"".as_slice(), "attachment must not be empty")]
fn invalid_attachment_is_rejected(
	#[case] filename: &str,
	#[case] media_type: &str,
	#[case] content: &[u8],
	#[case] expected: &str,
) {
	assert_eq!(
		validate_attachment(filename, media_type, content),
		Err(Error::Invalid(expected.into()))
	);
}
#[rstest]
fn attachment_size_limit_accepts_exactly_one_mib() {
	let exact = vec![1; 1024 * 1024];
	assert_eq!(
		validate_attachment("evidence.bin", "application/octet-stream", &exact),
		Ok(())
	);
	let too_large = vec![1; exact.len() + 1];
	assert_eq!(
		validate_attachment("evidence.bin", "application/octet-stream", &too_large),
		Err(Error::Invalid("attachment exceeds 1 MiB".into()))
	);
}
#[rstest]
fn attachment_identity_preserves_order_and_rejects_duplicates() {
	let a = Uuid::from_u128(1);
	let b = Uuid::from_u128(2);
	assert_ne!(digest_ids(&[a, b]).unwrap(), digest_ids(&[b, a]).unwrap());
	assert_eq!(
		digest_ids(&[a, a]),
		Err(Error::Invalid("attachment ids must be unique".into()))
	);
}
#[rstest]
fn only_legacy_unordered_records_can_use_sorted_replay_identity() {
	let a = Uuid::from_u128(1);
	let b = Uuid::from_u128(2);
	let digest = digest_ids(&[a, b]).unwrap();
	assert_eq!(
		legacy_digest_matches(&[b, a], &[(a, 0), (b, 0)], &digest),
		Ok(true)
	);
	assert_eq!(
		legacy_digest_matches(&[b, a], &[(a, 0), (b, 1)], &digest),
		Ok(false)
	);
	assert_eq!(legacy_digest_matches(&[a], &[(a, 0)], &digest), Ok(false));
	assert_eq!(
		legacy_digest_matches(&[b, a], &[(a, 0), (Uuid::from_u128(3), 0)], &digest),
		Ok(false)
	);
}

#[rstest]
fn citations_are_bounded_unique_and_ordered() {
	let citation = |area: u128, file: u128| CitedFileVersion {
		area_id: Uuid::from_u128(area),
		file_id: Uuid::from_u128(file),
	};
	let accepted: Vec<_> = (1..=MAX_CITED_VERSIONS)
		.map(|file| citation(1, file as u128))
		.collect();
	assert_eq!(validate_citations(&accepted), Ok(()));
	let mut too_many = accepted.clone();
	too_many.push(citation(1, 99));
	assert!(matches!(
		validate_citations(&too_many),
		Err(Error::Invalid(ref value)) if value.starts_with("CITATION_LIMIT")
	));
	assert!(matches!(
		validate_citations(&[citation(1, 2), citation(1, 2)]),
		Err(Error::Invalid(ref value)) if value.starts_with("CITATION_DUPLICATE")
	));
}
