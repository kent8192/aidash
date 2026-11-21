use super::*;

#[test]
fn a_later_page_of_due_schedules_keeps_every_due_condition() {
	// Arrange: a page that ended at a time with microseconds.
	let now: DateTime<Utc> = "2026-11-17T00:00:00Z".parse().unwrap();
	let at: DateTime<Utc> = "2026-11-16T23:59:30.123456Z".parse().unwrap();
	let id = Uuid::from_u128(7);
	// Act
	let sql = due_query(now, Some((at, id)), 32);
	// Assert: the keyset is one group ANDed with the due predicates, and the
	// cursor keeps its microseconds, so no row is read twice or skipped.
	assert!(
		sql.contains(r#""halt" IS NULL AND "next_fire_at" <= "#),
		"{sql}"
	);
	assert!(sql.contains(r#" OR ("next_fire_at" = "#), "{sql}");
	assert!(sql.contains("23:59:30.123456"), "{sql}");
	assert!(
		sql.ends_with(r#"ORDER BY "next_fire_at" ASC, "id" ASC LIMIT 32"#),
		"{sql}"
	);
}
