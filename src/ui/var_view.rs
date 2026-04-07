use std::collections::HashSet;

use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use crate::app::{App, Panel};

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focused_panel == Panel::Vars;
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let block = Block::default()
        .title("Variables")
        .borders(Borders::ALL)
        .border_style(border_style);

    if app.variables.is_empty() {
        let widget = Paragraph::new("(変数なし)").block(block);
        f.render_widget(widget, area);
        return;
    }

    // 前ステップから値が変わった変数名セットを構築する
    let changed: HashSet<&str> = app
        .prev_variables
        .iter()
        .filter_map(|prev| {
            let current = app.variables.iter().find(|v| v.name == prev.name)?;
            if current.value != prev.value {
                Some(prev.name.as_str())
            } else {
                None
            }
        })
        .collect();

    // ボーダー 2 行 + ヘッダー 1 行を除いた表示可能行数
    let visible = area.height.saturating_sub(3) as usize;

    let rows: Vec<Row> = app
        .variables
        .iter()
        .skip(app.var_scroll)
        .take(visible)
        .map(|var| {
            let style = if changed.contains(var.name.as_str()) {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            Row::new([
                Cell::from(var.name.clone()).style(style),
                Cell::from(var.type_name.clone()).style(style),
                Cell::from(var.value.clone()).style(style),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(30),
        Constraint::Percentage(25),
        Constraint::Percentage(45),
    ];

    let header_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let header = Row::new([
        Cell::from("名前").style(header_style),
        Cell::from("型").style(header_style),
        Cell::from("値").style(header_style),
    ])
    .height(1);

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}
