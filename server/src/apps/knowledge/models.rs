//! Persistent records owned by the semantic app.
pub(crate) mod cleanup;
mod index_configuration;
pub(crate) mod memory_records;
pub(crate) mod memory_units;
mod read_dependencies;
pub(crate) mod unit_origins;

mod semantic_collections;
pub use semantic_collections::SemanticCollection;
mod semantic_entries;
pub use semantic_entries::SemanticEntry;
mod semantic_history;
pub use semantic_history::SemanticHistory;
mod semantic_indexes;
pub use semantic_indexes::SemanticIndexe;
mod semantic_points;
pub use semantic_points::SemanticPoint;
mod semantic_run_reads;
pub use semantic_run_reads::SemanticRunRead;

pub mod states;

pub mod semantic_remote_attempts;

pub mod semantic_remote_operations;

pub mod semantic_remote_reads;

pub mod semantic_remote_receipts;
