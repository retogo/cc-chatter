//! ratatui のトップレベル `view(f, app)` 関数。
//!
//! 画面分岐はここで一元化し、個別 screen を呼び出すだけに留める。

use ratatui::Frame;

use crate::application::state::AppView;
use crate::application::App;

pub fn view(f: &mut Frame, app: &mut App) {
	let area = f.area();
	match app.view.current_view {
		AppView::SessionSelect => crate::ui::screens::session_select::render(f, area, app),
		AppView::SubAgentSelect => crate::ui::screens::subagent_select::render(f, area, app),
		AppView::Waiting => crate::ui::screens::waiting::render(f, area, app),
		AppView::Watching => crate::ui::screens::watching::render(f, area, app),
	}
}
