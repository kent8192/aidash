//! Knowledge persistence implements portable semantic ports.
pub(crate) mod access;
pub(crate) mod bindings;
pub(crate) mod candidates;
pub(crate) mod discard;
pub(crate) mod indexing;
pub(crate) mod memory_decay;
pub(crate) mod memory_graph;
pub(crate) mod memory_reads;
pub(crate) mod memory_receipts;
pub(crate) mod memory_scope;
pub(crate) mod mutations;
pub(crate) mod native_memory;
pub mod postgres_vector;
pub(crate) mod publications;
pub(crate) mod receiver_caches;
pub(crate) mod remote_memory_reads;
pub(crate) mod remote_participants;
pub(crate) mod storage;
pub(crate) mod units;

pub(crate) mod retrieval;

pub(crate) mod embedding;

pub(crate) mod memory;

pub(crate) mod disclosure;

pub(crate) mod run_context;

pub(crate) mod remote_journal;

pub(crate) mod remote_status;

pub(crate) mod purge;

pub(crate) mod bank_settings;

pub(crate) mod retention;

pub(crate) mod engine_jobs;
pub(crate) mod learning;
pub(crate) mod recovery;
pub(crate) mod unit_origins;
