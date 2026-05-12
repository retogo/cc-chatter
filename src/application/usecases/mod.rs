//! Application ユースケース。
//!
//! TS 版 `src/application/usecases/` の移植先。現状は `startup_skip` のみ。

pub mod startup_skip;

pub use startup_skip::{resolve_startup_state, ResolvedView, SkipResult, StartupError};
