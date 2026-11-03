// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Shared typed declarations; native-only dependencies are injected.
pub mod flows;
pub use flows::*;
pub mod accounts;
pub use accounts::*;
