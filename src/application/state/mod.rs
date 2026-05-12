//! App 状態管理 (Elm アーキテクチャ)。
//!
//! 3 つの独立した reducer を統合:
//! app (画面遷移) / domain (ビジネスデータ) / viewport (選択・展開・スクロール)。
//!
//! TS 側の `appReducer` / `domainState` / `viewportState` に対応。

pub mod app;
pub mod domain;
pub mod heights_cache;
pub mod viewport;

pub use app::{AppView, AppViewState};
pub use domain::DomainState;
pub use heights_cache::HeightsCache;
pub use viewport::ViewportState;
