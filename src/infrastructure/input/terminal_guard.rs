//! Terminal の raw mode / alternate screen / mouse capture を RAII で管理。
//!
//! Drop で必ず後始末する。プロセスが panic しても Drop が走る限りは
//! ターミナルが壊れないようにする。panic = "abort" や SIGKILL 経由で Drop が
//! 走らないケースは signal hook + panic hook で補う
//! (`install_panic_hook` / `install_signal_hook` 参照)。
//!
//! Issue 003 (TS 版) の教訓: mouse reporting (`\x1b[?1006h\x1b[?1002h`) を
//! 解除しないとシェルが壊れる。crossterm の `DisableMouseCapture` が同等の
//! sequence を送るので、Drop で必ず呼ぶ。

use std::io::{self, Stdout, Write};

use crossterm::{
	event::{DisableMouseCapture, EnableMouseCapture},
	execute,
	terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use signal_hook::{
	consts::{SIGHUP, SIGINT, SIGTERM},
	iterator::Signals,
};

/// TUI を起動する際のオプション。
#[derive(Debug, Clone, Copy)]
pub struct TerminalGuardOptions {
	/// `--no-mouse` で無効化したいとき false。
	pub enable_mouse: bool,
}

impl Default for TerminalGuardOptions {
	fn default() -> Self {
		Self { enable_mouse: true }
	}
}

/// RAII でターミナルの状態を管理するガード。
///
/// `new()` で raw mode + alternate screen + (optional) mouse capture を有効化し、
/// Drop で必ず逆順に解除する。途中 execute! が失敗しても panic させず、
/// ターミナル破壊の可能性を最小化する。
pub struct TerminalGuard {
	active: bool,
}

impl TerminalGuard {
	/// ターミナルをセットアップしガードを返す。
	///
	/// - enable_raw_mode
	/// - EnterAlternateScreen
	/// - EnableMouseCapture (options で有効化されている場合のみ)
	pub fn enter(options: TerminalGuardOptions) -> io::Result<Self> {
		enable_raw_mode()?;
		let mut out = io::stdout();
		execute!(out, EnterAlternateScreen)?;
		if options.enable_mouse {
			// mouse capture が使えない環境でもここで失敗したら raw mode を戻す
			if let Err(err) = execute!(out, EnableMouseCapture) {
				let _ = execute!(out, LeaveAlternateScreen);
				let _ = disable_raw_mode();
				return Err(err);
			}
		}
		Ok(Self { active: true })
	}

	/// 明示的に解除する (Drop でも呼ぶので通常は不要)。
	pub fn leave(&mut self) {
		if !self.active {
			return;
		}
		self.active = false;
		// mouse_enabled=false でも念のため DisableMouseCapture を送っておく
		// (hook 経由で有効化された状態で入ってきても戻す)。失敗は無視。
		restore_terminal_quiet();
	}

	/// ratatui の Terminal を作るための stdout を返す (借用ではなく owned)。
	pub fn stdout() -> Stdout {
		io::stdout()
	}
}

impl Drop for TerminalGuard {
	fn drop(&mut self) {
		self.leave();
	}
}

/// panic hook を差し替えて、panic 時に mouse capture + alternate screen を
/// 解除してからデフォルトハンドラに委譲する。
///
/// `panic = "abort"` 下では Drop が走らないため、この hook が最後の砦になる。
pub fn install_panic_hook() {
	let original = std::panic::take_hook();
	std::panic::set_hook(Box::new(move |info| {
		// 可能な限り後始末を試みる。失敗しても panic の伝達は止めない。
		restore_terminal_quiet();
		original(info);
	}));
}

/// `SIGINT` / `SIGTERM` / `SIGHUP` を別スレッドで受け取り、ターミナルを復元
/// してから `std::process::exit` で終了する。
///
/// Ctrl+C は crossterm が `KeyEvent(Char('c'), CONTROL)` として捕まえる経路で
/// event_loop が `Cmd::Quit` まで辿り着くので通常の終了が走る。この hook は
/// それに到達しない経路 (別プロセスからの `kill <pid>` / tmux の kill-session
/// / セッション hangup など) の **保険**。
///
/// 呼び出しは `main` で TUI 起動前に 1 回だけ行うこと。二重登録しても
/// signal-hook 側で拒否されない (追加ハンドラとして登録される) ため、
/// テストや二重呼び出しは避ける。
pub fn install_signal_hook() -> io::Result<()> {
	let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP])?;
	std::thread::spawn(move || {
		// forever() は signal を受けたらループで yield する。最初の 1 本で
		// exit するのでループは実質 1 回転。
		if let Some(sig) = signals.forever().next() {
			restore_terminal_quiet();
			// 慣習的な終了コード (128 + signal number)。プログラム側から
			// 区別したいケースはないが、シェル側で `$?` を見たときに
			// 「シグナルで死んだ」と分かる値にしておく。
			let code = match sig {
				SIGINT => 130,
				SIGTERM => 143,
				SIGHUP => 129,
				_ => 1,
			};
			std::process::exit(code);
		}
	});
	Ok(())
}

/// mouse capture / alternate screen / raw mode をベストエフォートで解除する。
///
/// panic hook / signal hook / Drop で共通して呼ばれる。終了経路での失敗は
/// 握り潰す (ターミナル壊れのリスクが残るのは同じ)。
fn restore_terminal_quiet() {
	let mut out = io::stdout();
	let _ = execute!(out, DisableMouseCapture);
	let _ = execute!(out, LeaveAlternateScreen);
	let _ = disable_raw_mode();
	let _ = out.flush();
}
