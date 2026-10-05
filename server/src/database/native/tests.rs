use super::*;
use rstest::rstest;
use serde_json::{Value, json};

#[rstest]
#[case::sql_null(QueryValue::Null, None)]
#[case::json_sql_null(QueryValue::Json(None), None)]
#[case::json_null(QueryValue::Json(Some(Box::new(Value::Null))), Some(Value::Null))]
#[case::json_string(QueryValue::Json(Some(Box::new(json!("null")))), Some(json!("null")))]
fn nullable_json_retains_database_null_identity(
	#[case] value: QueryValue,
	#[case] expected: Option<Value>,
) {
	// Arrange
	let mut row = reinhardt::db::backends::Row::new();
	row.insert("payload".into(), value);
	// Act
	let actual = Row(row).try_get::<Option<Value>>("payload").unwrap();
	// Assert
	assert_eq!(actual, expected);
}

#[rstest]
fn tuple_projections_follow_explicit_columns_not_hash_map_order() {
	// Arrange
	let mut row = reinhardt::db::backends::Row::new();
	row.insert("z_name".into(), QueryValue::String("agent".into()));
	row.insert("a_revision".into(), QueryValue::Int(19));
	// Act
	let actual = <(String, i64)>::decode(&Row(row), &["z_name", "a_revision"]).unwrap();
	// Assert
	assert_eq!(actual, ("agent".into(), 19));
}

#[rstest]
fn byte_projections_remain_bytes_at_the_domain_boundary() {
	// Arrange
	let mut row = reinhardt::db::backends::Row::new();
	row.insert("content".into(), QueryValue::Bytes(vec![0, 255, 10, 42]));
	// Act
	let actual: Vec<u8> = Row(row).try_get("content").unwrap();
	// Assert
	assert_eq!(actual, vec![0, 255, 10, 42]);
}

#[rstest]
#[case::open("OPEN", aidash_domain::TaskStatus::Open)]
#[case::completed("COMPLETED", aidash_domain::TaskStatus::Completed)]
#[case::abandoned("ABANDONED", aidash_domain::TaskStatus::Abandoned)]
fn task_status_projections_retain_the_serialized_domain_enum(
	#[case] status: &str,
	#[case] expected: aidash_domain::TaskStatus,
) {
	// Arrange
	let mut row = reinhardt::db::backends::Row::new();
	row.insert("status".into(), QueryValue::String(status.into()));
	// Act
	let actual: aidash_domain::TaskStatus = Row(row).try_get("status").unwrap();
	// Assert
	assert_eq!(actual, expected);
}

#[rstest]
fn newtype_json_projections_preserve_sql_null_distinction() {
	// Arrange
	#[derive(Debug, PartialEq, serde::Deserialize)]
	struct Payload(Option<Value>);
	let mut row = reinhardt::db::backends::Row::new();
	row.insert("sql_null".into(), QueryValue::Null);
	row.insert(
		"json_null".into(),
		QueryValue::Json(Some(Box::new(Value::Null))),
	);
	let row = Row(row);
	// Act
	let sql_null: Payload = row.try_get("sql_null").unwrap();
	let json_null: Payload = row.try_get("json_null").unwrap();
	// Assert
	assert_eq!(sql_null, Payload(None));
	assert_eq!(json_null, Payload(Some(Value::Null)));
}

#[rstest]
fn ambiguous_scalar_or_unmapped_tuple_is_rejected() {
	// Arrange
	let mut row = reinhardt::db::backends::Row::new();
	row.insert("left".into(), QueryValue::Int(1));
	row.insert("right".into(), QueryValue::Int(2));
	let row = Row(row);
	// Act / Assert
	assert!(
		matches!(row.only_column(), Err(Error::Invalid(reason)) if reason == "scalar projection requires one column")
	);
	assert!(
		matches!(<(i64,i64)>::decode(&row, &[]), Err(Error::Invalid(reason)) if reason == "tuple projection requires explicit columns")
	);
	assert!(
		matches!(<(i64,i64)>::decode(&row, &["left", "right", "unselected"]), Err(Error::Invalid(reason)) if reason == "tuple projection requires explicit columns")
	);
}
