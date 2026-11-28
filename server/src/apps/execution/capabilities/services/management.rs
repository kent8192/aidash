//! Typed management responses. Only provenance/configuration payloads are open-ended.

pub use crate::apps::execution::capabilities::serializers::management::{
	ApprovalDecided, ApprovalPage, ConfiguredAgent, DeletedThread, OperationPage, OutboundStatus,
	ReferenceRevocation, ReferenceRevoked, Revoked,
};

#[cfg(test)]
#[path = "../tests/services_management_tests.rs"]
mod tests;
