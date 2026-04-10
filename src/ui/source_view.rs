use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::{App, Panel};
use crate::app::FrameView;

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focused_panel == Panel::Source;
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let title = match &app.current_file {
        Some(p) => format!("Source – {}", p.display()),
        None => "Source".to_string(),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    if app.source_lines.is_empty() {
        let widget = Paragraph::new("(no file loaded)").block(block);
        f.render_widget(widget, area);
        return;
    }

    // 表示可能な高さ（ボーダー分を引く）
    let view_height = area.height.saturating_sub(2) as usize;

    // current_line は 1-origin
    let current_idx = app
        .current_line
        .map(|l| (l as usize).saturating_sub(1))
        .unwrap_or(0);

    // source_cursor は 1-origin
    let cursor_idx = app.source_cursor.saturating_sub(1);

    // app.source_scroll をそのまま使う
    let scroll_offset = app.source_scroll;

    let line_num_width = app.source_lines.len().to_string().len().max(2);
    let current_file = app.current_file.as_deref();
    let has_current = app.current_line.is_some();

    let lines: Vec<Line> = app
        .source_lines
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(view_height)
        .map(|(i, text)| {
            let line_num = (i + 1) as u32;
            let is_current = has_current && i == current_idx;
            let is_cursor = i == cursor_idx;
            let is_bp = current_file.map_or(false, |f| {
                app.breakpoints.iter().any(|bp| bp.file == f && bp.line == line_num)
            });

            let bg = if is_cursor { Color::DarkGray } else { Color::Reset };

            let bp_span = if is_bp {
                Span::styled("●", Style::default().fg(Color::Red).bg(bg))
            } else {
                Span::styled(" ", Style::default().bg(bg))
            };
            let arrow_span = if is_current {
                Span::styled("→", Style::default().fg(Color::Yellow).bg(bg))
            } else {
                Span::styled(" ", Style::default().bg(bg))
            };
            let num_span = Span::styled(
                format!("{:>width$} | ", line_num, width = line_num_width),
                Style::default().fg(Color::DarkGray).bg(bg),
            );
            let text_span = if is_current {
                Span::styled(
                    text.clone(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                        .bg(bg),
                )
            } else {
                Span::styled(text.clone(), Style::default().bg(bg))
            };

            Line::from(vec![bp_span, arrow_span, num_span, text_span])
        })
        .collect();

    let widget = Paragraph::new(lines).block(block);
    f.render_widget(widget, area);
}

/// 呼び出し元フレームを固定表示する（左パネル用）。
/// frame_stack.last() の FrameView を使い、highlight_line を赤でマークする。
pub fn render_frozen(f: &mut Frame, app: &App, area: Rect) {
    let frame: &FrameView = match app.frame_stack.last() {
        Some(fr) => fr,
        None => return,
    };

    let file_name = frame.file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?");
    let title = format!("呼び出し元 – {}", file_name);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default());

    if frame.source_lines.is_empty() {
        let widget = Paragraph::new("(no file loaded)").block(block);
        f.render_widget(widget, area);
        return;
    }

    let view_height = area.height.saturating_sub(2) as usize;

    // highlight_line は 1-origin → 0-origin に変換
    let highlight_idx = frame.highlight_line.saturating_sub(1);

    // ハイライト行が中央に来るようにスクロールオフセットを固定する
    let scroll_offset = highlight_idx.saturating_sub(view_height / 2);

    let line_num_width = frame.source_lines.len().to_string().len().max(2);

    let lines: Vec<Line> = frame.source_lines
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(view_height)
        .map(|(i, text)| {
            let line_num = (i + 1) as u32;
            let is_highlight = i == highlight_idx;

            let bg = Color::Reset;
            let bp_span = Span::styled(" ", Style::default().bg(bg));
            let arrow_span = if is_highlight {
                Span::styled("→", Style::default().fg(Color::Red).bg(bg))
            } else {
                Span::styled(" ", Style::default().bg(bg))
            };
            let num_span = Span::styled(
                format!("{:>width$} | ", line_num, width = line_num_width),
                Style::default().fg(Color::DarkGray).bg(bg),
            );
            let text_span = if is_highlight {
                Span::styled(
                    text.clone(),
                    Style::default()
                        .fg(Color::Red)
                        .add_modifier(Modifier::BOLD)
                        .bg(bg),
                )
            } else {
                Span::styled(text.clone(), Style::default().bg(bg))
            };

            Line::from(vec![bp_span, arrow_span, num_span, text_span])
        })
        .collect();

    let widget = Paragraph::new(lines).block(block);
    f.render_widget(widget, area);
}
