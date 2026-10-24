use super::*;
use reinhardt::core::exception::DatabaseErrorKind;
use rstest::rstest;

#[rstest]
#[case::no_expiry(None)]
#[case::expiring(Some(DateTime::from_timestamp(1_800_000_000, 0).unwrap()))]
fn approval_projection_preserves_nullable_expiry(#[case] expires_at: Option<DateTime<Utc>>) {
	let mut row = Row::new();
	row.insert("state".into(), QueryValue::String("APPROVED".into()));
	row.insert(
		"expires_at".into(),
		expires_at.map_or(QueryValue::Null, QueryValue::Timestamp),
	);

	let decoded = approval(row).unwrap();

	assert_eq!(decoded.state, "APPROVED");
	assert_eq!(decoded.expires_at, expires_at);
}

#[rstest]
#[case::missing(None, DatabaseErrorKind::ColumnNotFound)]
#[case::invalid(Some(QueryValue::String("invalid".into())), DatabaseErrorKind::Type)]
fn approval_projection_rejects_missing_or_invalid_expiry(
	#[case] expires_at: Option<QueryValue>,
	#[case] kind: DatabaseErrorKind,
) {
	let mut row = Row::new();
	row.insert("state".into(), QueryValue::String("APPROVED".into()));
	if let Some(value) = expires_at {
		row.insert("expires_at".into(), value);
	}

	let error = approval(row).err().expect("invalid expiry must fail");

	let Error::Framework(error) = error else {
		panic!("expected the native database error");
	};
	assert_eq!(error.database_error().unwrap().kind(), kind);
}

#[rstest]
fn approval_projection_requires_a_state_even_without_expiry() {
	let mut row = Row::new();
	row.insert("expires_at".into(), QueryValue::Null);

	let error = approval(row).err().expect("missing state must fail");

	let Error::Framework(error) = error else {
		panic!("expected the native database error");
	};
	assert_eq!(
		error.database_error().unwrap().kind(),
		DatabaseErrorKind::ColumnNotFound
	);
}
