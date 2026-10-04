//! Native persistence and protocol adapters use the portable semantic contracts.
pub(crate) mod journal;
pub mod status;
pub use aidash_domain::semantic::Failure;
pub use aidash_domain::semantic::remote::{Binding, Provider, Request};
pub(crate) use aidash_domain::semantic::remote::{
	Boundary, InputRead, Operation, Receipt, SourceRead, VERSION, bounded_query,
};
