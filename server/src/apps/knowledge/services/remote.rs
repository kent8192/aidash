//! Native persistence and protocol adapters use the portable semantic contracts.
pub(crate) mod journal;
pub mod status;
pub use aidash_domain::semantic::Failure;
pub use aidash_domain::semantic::remote::{Binding, Request};
pub(crate) use aidash_domain::semantic::remote::{
	Boundary, InputRead, Operation, Receipt, bounded_query,
};
