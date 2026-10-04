use super::*;
use rstest::rstest;
#[rstest]
#[case::outbound("transfer_out", "transfer_snapshot", true)]
#[case::inbound("transfer_in", "transfer_staging", true)]
#[case::reference_staging("reference", "reference_staging", true)]
#[case::reference_original("reference", "reference_original", true)]
#[case::reference_extraction("reference", "reference_extraction", true)]
#[case::wrong_transfer_owner("transfer_out", "transfer_staging", false)]
#[case::published_working("reference", "working", false)]
#[case::published_received("transfer_in", "received", false)]
#[case::unrelated("unknown", "transfer_snapshot", false)]
fn reclamation_is_limited_to_the_record_owned_provisional_namespace(
	#[case] record: &str,
	#[case] object: &str,
	#[case] permitted: bool,
) {
	assert_eq!(provisional_kind(record, object), permitted);
}
