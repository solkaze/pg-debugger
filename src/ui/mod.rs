use ratatui::{
    layout::{Constraint, Direction, Layout},
    Frame,
};

use crate::app::App;

pub mod console_view;
pub mod source_view;
pub mod status_bar;
pub mod var_view;

pub fn render(f: &mut Frame, app: &App) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(2)]) // ステータスバー 2 行
        .split(f.area());

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(vertical[0]);

    // 左パネルをソース(70%) / コンソール(30%) に縦分割
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(body[0]);

    source_view::render(f, app, left[0]);
    console_view::render(f, app, left[1]);
    var_view::render(f, app, body[1]);
    status_bar::render(f, app, vertical[1]);
}
