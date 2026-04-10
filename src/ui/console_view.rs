use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::{App, InputMode, Panel};

pub fn render(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let focused = app.focused_panel == Panel::Console;
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let title = if app.console_scroll.is_some() {
        "Console（スクロール中 ↑↓/PgUp/PgDn）"
    } else {
        "Console"
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    // ボーダーの内側領域を先に取得し、ブロックを描画する
    let inner = block.inner(area);
    f.render_widget(block, area);

    // 内側を「出力エリア / 区切り線 / 入力行」に分割する
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),     // 出力エリア（残り全部）
            Constraint::Length(1),  // 区切り線
            Constraint::Length(1),  // 入力行
        ])
        .split(inner);

    let output_area = chunks[0];
    let sep_area = chunks[1];
    let input_area = chunks[2];

    // --- 出力エリア ---
    let view_height = output_area.height as usize;

    let mut display_lines = app.console_lines.clone();
    if !app.console_line_buf.is_empty() {
        display_lines.push(app.console_line_buf.clone());
    }

    if display_lines.is_empty() {
        f.render_widget(
            Paragraph::new("(出力なし)")
                .style(Style::default().fg(Color::DarkGray)),
            output_area,
        );
    } else {
        let skip = match app.console_scroll {
            None => display_lines.len().saturating_sub(view_height),
            Some(n) => n,
        };

        let lines: Vec<Line> = display_lines
            .iter()
            .skip(skip)
            .take(view_height)
            .map(|text| Line::from(Span::raw(text.clone())))
            .collect();

        f.render_widget(Paragraph::new(lines), output_area);
    }

    // --- 区切り線 ---
    let sep_text = "─".repeat(sep_area.width as usize);
    f.render_widget(
        Paragraph::new(sep_text).style(Style::default().fg(Color::DarkGray)),
        sep_area,
    );

    // --- 入力行 ---
    let (input_text, input_style) = match app.input_mode {
        InputMode::StdinInput => (
            format!("> {}_", app.stdin_buffer),
            Style::default().fg(Color::Cyan),
        ),
        _ => (
            "> (iキーで入力)".to_string(),
            Style::default().fg(Color::DarkGray),
        ),
    };

    f.render_widget(
        Paragraph::new(input_text).style(input_style),
        input_area,
    );
}
