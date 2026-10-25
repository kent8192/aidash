//! Shared typed declarations; native-only dependencies are injected.
pub mod catalog;
pub use catalog::*;
pub mod publishers;
pub use publishers::*;
pub mod library;
pub use library::*;
pub mod connections;
pub use connections::*;
