//! Native protocol, persistence and transport adapters.
pub mod grpc;
pub mod migrations;
pub mod rpc;
pub mod store;
#[cfg(test)]
mod tests;
pub mod views;
