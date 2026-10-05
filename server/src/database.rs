//! Database-boundary compatibility for the pinned Reinhardt revision.
pub mod native;
mod records;
pub use records::Record;
mod projections;
pub use projections::query_as;
use uuid::Uuid;

/// Preserve the previous PostgreSQL driver's timestamp precision at an ORM boundary.
/// Reinhardt's generated argument codec rejects sub-microsecond values. SQLx's
/// PostgreSQL encoder truncates the duration from its 2000-01-01 epoch toward zero.
pub(crate) fn postgres_timestamp(
	value: chrono::DateTime<chrono::Utc>,
) -> chrono::DateTime<chrono::Utc> {
	let epoch = chrono::DateTime::from_timestamp(946_684_800, 0)
		.expect("the PostgreSQL epoch is a valid UTC timestamp");
	let micros = value
		.signed_duration_since(epoch)
		.num_microseconds()
		.expect("Chrono timestamps fit in PostgreSQL's signed microsecond representation");
	epoch + chrono::Duration::microseconds(micros)
}

/// Preserve PostgreSQL array element types, including empty arrays, when the
/// selected Query release has no `IntoValue` implementation for these vectors.
pub(crate) fn text_array(
	values: impl IntoIterator<Item = impl AsRef<str>>,
) -> reinhardt::query::SimpleExpr {
	array_expression(
		"text",
		values
			.into_iter()
			.map(|value| reinhardt::query::Expr::value(value.as_ref().to_owned()).into())
			.collect(),
	)
}

pub(crate) fn uuid_array(values: impl IntoIterator<Item = Uuid>) -> reinhardt::query::SimpleExpr {
	array_expression(
		"uuid",
		values
			.into_iter()
			.map(|value| reinhardt::query::Expr::value(value).into())
			.collect(),
	)
}

fn array_expression(
	element: &str,
	values: Vec<reinhardt::query::SimpleExpr>,
) -> reinhardt::query::SimpleExpr {
	let placeholders = std::iter::repeat_n("?", values.len())
		.collect::<Vec<_>>()
		.join(",");
	reinhardt::query::SimpleExpr::CustomWithExpr(
		format!("ARRAY[{placeholders}]::{element}[]"),
		values,
	)
}

/// Bind the named interval argument as a managed expression so the duration
/// remains scoped correctly after preceding update values. PostgreSQL's named
/// `make_interval` argument has no dedicated Query builder API.
pub(crate) fn lease_deadline(seconds: i32) -> reinhardt::query::SimpleExpr {
	reinhardt::query::SimpleExpr::CustomWithExpr(
		"CURRENT_TIMESTAMP + make_interval(secs => ?)".into(),
		vec![reinhardt::query::Expr::value(seconds).into()],
	)
}
