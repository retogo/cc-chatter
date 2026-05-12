//! Application 層のマッパー群。
//!
//! TS 版 `src/application/mappers/` の移植。純粋な変換ロジックを純粋関数で
//! 提供する。

pub mod message_mapper;

pub use message_mapper::{build_message_id, to_mapper_outcomes, MapperOutcome};
