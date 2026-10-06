//! Compatibility exports for transaction-scoped capability records.
pub use crate::apps::execution::capabilities::serializers::records::Record;
pub(crate) use crate::apps::execution::repositories::core_records::{
	get, insert, update, update_committed,
};
