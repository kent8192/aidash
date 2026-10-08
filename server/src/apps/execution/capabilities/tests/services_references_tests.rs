//! Unit tests for services::references.
use super::next_receipt_cursor;
use serde_json::json;
use uuid::Uuid;

#[rstest::rstest]
fn receipt_batches_advance_past_failed_rows_and_restart_after_the_end() {
	let first = Uuid::from_u128(1);
	let second = Uuid::from_u128(2);
	let third = Uuid::from_u128(3);
	let page = vec![(first, json!({})), (second, json!({}))];
	let cursor = next_receipt_cursor(&page);
	assert_eq!(cursor, second, "the cursor advances despite ack failures");
	let next_page = vec![(third, json!({}))];
	assert_eq!(next_receipt_cursor(&next_page), third);
	assert_eq!(next_receipt_cursor(&[]), Uuid::nil());
}
