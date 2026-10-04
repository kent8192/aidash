//! Database-boundary compatibility for the pinned Reinhardt revision.
mod records;
pub use records::Record;
mod projections;
pub use projections::query_as;
use uuid::Uuid;

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
