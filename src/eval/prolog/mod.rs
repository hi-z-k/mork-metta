//! SWI-Prolog bridge (enabled by the `prolog-bridge` feature).
//! See `docs/prolog-bridge.md` for the architecture.

pub mod builtins;
pub mod convert;
pub mod engine;
pub mod error;
pub mod ffi;
pub mod handle;
pub mod ops;
pub mod vm;
