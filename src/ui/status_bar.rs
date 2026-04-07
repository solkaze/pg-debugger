use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::app::{App, InputMode};

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    // 上段: 現在の停止位置（GDB からのイベントで更新される）
    let location_text = if !app.status_message.is_empty() {
        app.status_message.clone()
    } else {
        "GDB 未起動".to_string()
    };

    let location = Paragraph::new(location_text)
        .block(Block::default())
        .style(Style::default().fg(Color::Yellow).bg(Color::DarkGray));
    f.render_widget(location, chunks[0]);

    // 下段: キーバインド一覧 or 各入力モード中のガイド
    let keys_text = match app.input_mode {
        InputMode::BreakpointLine => format!(" 行番号を入力: {}_", app.input_buffer),
        InputMode::GotoLine => format!(" ジャンプ先の行番号を入力: {}_", app.input_buffer),
        InputMode::StdinInput => " Enterで送信  Escでキャンセル".to_string(),
        InputMode::Normal => {
            " n/F10:次へ  s/F11:ステップイン  f/F12:ステップアウト  c:続行  b:BP切替  B:行指定BP  g:行ジャンプ  i:標準入力  r:再起動  Tab:切替  q:終了".to_string()
        }
    };

    let keybinds = Paragraph::new(keys_text)
        .block(Block::default())
        .style(Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD));
    f.render_widget(keybinds, chunks[1]);
}
