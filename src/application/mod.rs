//! Application 層: ユースケース / 状態遷移 (Elm アーキテクチャ相当)。

pub mod app;
pub mod mappers;
pub mod msg;
pub mod state;
pub mod usecases;

pub use app::App;
pub use msg::{Cmd, Msg};
