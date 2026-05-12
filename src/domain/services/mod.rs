//! Domain services.
//!
//! TS 版 `src/domain/services/` のうち、純粋なフィルタリング・表示ロジックを
//! 移植する (`AgentMappingService` は infrastructure 側 `AgentMapperImpl` に
//! 畳み込んで再実装)。

pub mod session_filter;

pub use session_filter::{get_display_text, is_explicit_user_action};
