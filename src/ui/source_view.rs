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

    let raw_title = if app.frame_stack.is_empty() {
        format!("Source – {}", app.call_stack_title())
    } else {
        format!("▶ {}", app.call_stack_title())
    };
    let title = truncate_title(&raw_title, area.width as usize);

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
    // 現在実行中の関数スコープ
    let exec_range = app.current_func_range();
    // カーソル行の関数スコープ
    let cursor_range = app.func_range_at_line(app.source_cursor);
    // current_line が属する最も内側のブロック範囲
    let block_range = app.current_block_range();

    let in_scope = |line_num: usize| -> bool {
        let in_exec = exec_range
            .map(|(s, e)| line_num >= s && line_num <= e)
            .unwrap_or(true);
        let in_cursor = cursor_range
            .map(|(s, e)| line_num >= s && line_num <= e)
            .unwrap_or(false);
        in_exec || in_cursor
    };

    let lines: Vec<Line> = app
        .source_lines
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(view_height)
        .map(|(i, text)| {
            let line_num = i + 1; // 1-origin
            let line_num_u32 = line_num as u32;
            let is_current = has_current && i == current_idx;
            let is_cursor = i == cursor_idx;
            let is_bp = current_file.map_or(false, |f| {
                app.breakpoints.iter().any(|bp| bp.file == f && bp.line == line_num_u32)
            });

            let bg = if is_cursor { Color::DarkGray } else { Color::Reset };

            let is_block_open = block_range
                .map(|(open, _)| line_num == open)
                .unwrap_or(false);
            let is_block_close = block_range
                .map(|(_, close)| line_num == close)
                .unwrap_or(false);

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
            let num_style = if (is_block_open || is_block_close) && !is_current {
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD).bg(bg)
            } else {
                Style::default().fg(Color::DarkGray).bg(bg)
            };
            let num_span = Span::styled(
                format!("{:>width$} | ", line_num_u32, width = line_num_width),
                num_style,
            );
            let text_style = if is_current {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
                    .bg(bg)
            } else if !app.gray_out_enabled || in_scope(line_num) {
                Style::default().fg(Color::White).bg(bg)
            } else {
                Style::default().fg(Color::DarkGray).bg(bg)
            };

            let text_spans: Vec<Span> = if is_current {
                vec![Span::styled(text.clone(), text_style)]
            } else if is_block_open {
                if let Some(pos) = text.rfind('{') {
                    vec![
                        Span::styled(text[..pos].to_string(), text_style),
                        Span::styled("{".to_string(), Style::default()
                            .fg(Color::Green).add_modifier(Modifier::BOLD).bg(bg)),
                        Span::styled(text[pos+1..].to_string(), text_style),
                    ]
                } else {
                    vec![Span::styled(text.clone(), text_style)]
                }
            } else if is_block_close {
                if let Some(pos) = text.find('}') {
                    vec![
                        Span::styled(text[..pos].to_string(), text_style),
                        Span::styled("}".to_string(), Style::default()
                            .fg(Color::Green).add_modifier(Modifier::BOLD).bg(bg)),
                        Span::styled(text[pos+1..].to_string(), text_style),
                    ]
                } else {
                    vec![Span::styled(text.clone(), text_style)]
                }
            } else {
                vec![Span::styled(text.clone(), text_style)]
            };

            let mut line_spans = vec![bp_span, arrow_span, num_span];
            line_spans.extend(text_spans);
            Line::from(line_spans)
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

    let raw_title = format!("呼び出し元 ▶ {}", app.call_stack_title_frozen());
    let title = truncate_title(&raw_title, area.width as usize);

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

    // このフレームの関数スコープを計算する
    let frozen_range = compute_func_range(&frame.source_lines, &frame.func_name);

    let lines: Vec<Line> = frame.source_lines
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(view_height)
        .map(|(i, text)| {
            let line_num = i + 1; // 1-origin
            let line_num_u32 = line_num as u32;
            let is_highlight = i == highlight_idx;

            let in_scope = frozen_range
                .map(|(s, e)| line_num >= s && line_num <= e)
                .unwrap_or(true);

            let bg = Color::Reset;
            let bp_span = Span::styled(" ", Style::default().bg(bg));
            let arrow_span = if is_highlight {
                Span::styled("→", Style::default().fg(Color::Red).bg(bg))
            } else {
                Span::styled(" ", Style::default().bg(bg))
            };
            let num_span = Span::styled(
                format!("{:>width$} | ", line_num_u32, width = line_num_width),
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
            } else if !app.gray_out_enabled || in_scope {
                Span::styled(text.clone(), Style::default().fg(Color::White).bg(bg))
            } else {
                Span::styled(text.clone(), Style::default().fg(Color::DarkGray).bg(bg))
            };

            Line::from(vec![bp_span, arrow_span, num_span, text_span])
        })
        .collect();

    let widget = Paragraph::new(lines).block(block);
    f.render_widget(widget, area);
}

/// タイトルが幅を超える場合、先頭を省略して "..." を付けて返す。
/// area_width はボーダー込みの幅（内側は -2）。
fn truncate_title(title: &str, area_width: usize) -> String {
    // ボーダー2文字 + 左右の空白各1文字 = 4文字分を引く
    let max = area_width.saturating_sub(4);
    if title.chars().count() <= max {
        title.to_string()
    } else {
        // "..." の3文字分を引いた長さ分を末尾から取る
        let keep = max.saturating_sub(3);
        let chars: Vec<char> = title.chars().collect();
        let start = chars.len().saturating_sub(keep);
        // → の区切りの途中で切れないよう、先頭から次の " → " 以降を使う
        let suffix: String = chars[start..].iter().collect();
        if let Some(pos) = suffix.find(" → ") {
            format!("...{}", &suffix[pos..])
        } else {
            format!("...{}", suffix)
        }
    }
}

/// source_lines から func_name の関数スコープを計算して返す（1-origin）。
fn compute_func_range(lines: &[String], func_name: &str) -> Option<(usize, usize)> {
    if func_name.is_empty() {
        return None;
    }

    // 関数定義行を探す
    let start_line = lines.iter().enumerate().find_map(|(i, line)| {
        if line.contains(&format!("{}(", func_name))
            || line.contains(&format!("{} (", func_name))
        {
            Some(i + 1) // 1-origin
        } else {
            None
        }
    })?;

    // 開始行以降で最初の { を探す
    let mut brace_start = None;
    for (i, line) in lines.iter().enumerate().skip(start_line - 1) {
        if line.contains('{') {
            brace_start = Some(i);
            break;
        }
    }
    let brace_start = brace_start?;

    // 対応する } を深さを追って探す
    let mut depth = 0usize;
    let mut end_line = None;
    for (i, line) in lines.iter().enumerate().skip(brace_start) {
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    if depth > 0 { depth -= 1; }
                    if depth == 0 {
                        end_line = Some(i + 1); // 1-origin
                        break;
                    }
                }
                _ => {}
            }
        }
        if end_line.is_some() { break; }
    }

    Some((start_line, end_line.unwrap_or(lines.len())))
}
