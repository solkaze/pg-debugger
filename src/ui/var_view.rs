use std::collections::HashSet;

use unicode_width::UnicodeWidthChar;

use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use crate::app::{App, Panel};
use crate::gdb_utils::decode_gdb_octal_string;

/// "{v1, v2, v3, ...}" → Vec<String>
fn parse_array_elements(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        let inner = &trimmed[1..trimmed.len() - 1];
        if inner.trim().is_empty() {
            vec![]
        } else {
            inner.split(',').map(|s| s.trim().to_string()).collect()
        }
    } else {
        vec![]
    }
}


/// double/float の値を小数点以下6桁に丸め、末尾の余分な0を除去する
fn format_float_value(value: &str) -> String {
    if let Ok(f) = value.parse::<f64>() {
        let s = format!("{:.6}", f);
        let s = s.trim_end_matches('0');
        if s.ends_with('.') {
            format!("{}0", s)
        } else {
            s.to_string()
        }
    } else {
        value.to_string()
    }
}

/// 値が30文字を超える場合は末尾を ... で切り詰める
fn truncate_value(value: &str) -> String {
    const MAX_LEN: usize = 30;
    if value.chars().count() > MAX_LEN {
        let truncated: String = value.chars().take(MAX_LEN).collect();
        format!("{}...", truncated)
    } else {
        value.to_string()
    }
}

/// char* / const char* の値からアドレス・シンボル名を除去してデコードした文字列を返す。
/// パターンA: "0x401234 \"せかい\""
/// パターンB: "0x401234 \"\\343\\201\\233\""  (オクタルエスケープ)
/// パターンC: "0x401234 <symbol_name> \"...\""  (シンボル名あり)
fn format_char_ptr_value(value: &str) -> String {
    tracing::debug!("format_char_ptr_value input={:?}", value);
    let trimmed = value.trim();
    if !trimmed.starts_with("0x") {
        tracing::debug!("format_char_ptr_value: not an address, returning as-is");
        return trimmed.to_string();
    }

    // アドレス部分を除去
    let rest = trimmed.splitn(2, ' ').nth(1).unwrap_or("").trim();

    // シンボル名 <...> を除去（ネストしたものも考慮して繰り返す）
    let mut rest = rest;
    while rest.starts_with('<') {
        if let Some(end) = rest.find('>') {
            rest = rest[end + 1..].trim();
        } else {
            break;
        }
    }

    tracing::debug!("format_char_ptr_value: after stripping address/symbol: {:?}", rest);

    if rest.is_empty() {
        tracing::debug!("format_char_ptr_value: no string content, returning address");
        return trimmed.to_string();
    }
    let result = decode_gdb_octal_string(rest);
    tracing::debug!("format_char_ptr_value output={:?}", result);
    result
}

/// 表示幅オフセットを考慮して文字列をスキップする
fn skip_display_width(s: &str, skip_width: usize) -> &str {
    let mut current_width = 0;
    for (i, c) in s.char_indices() {
        if current_width >= skip_width {
            return &s[i..];
        }
        current_width += c.width().unwrap_or(0);
    }
    ""
}

/// 展開可能な配列かどうか（type に "[" が含まれ、値が "{" で始まる）
fn is_expandable_array(type_name: &str, value: &str) -> bool {
    type_name.contains('[') && value.trim().starts_with('{')
}

/// char配列の {N, N, N, ...} 数値リスト形式をUTF-8文字列に変換する。
/// 0バイトで終端し、結果を "\"...\"" 形式で返す。
fn decode_numeric_char_array(value: &str) -> String {
    let elements = parse_array_elements(value);
    let bytes: Vec<u8> = elements
        .iter()
        .filter_map(|e| e.trim().parse::<u8>().ok())
        .take_while(|&b| b != 0)
        .collect();
    match String::from_utf8(bytes.clone()) {
        Ok(s) => format!("\"{}\"", s),
        Err(_) => {
            let ascii: String = bytes
                .iter()
                .map(|&b| if (32..=126).contains(&b) { b as char } else { '?' })
                .collect();
            format!("\"{}\"", ascii)
        }
    }
}

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

    // 全レンダリング行を構築してからスクロールオフセット分スキップして表示する
    let all_rows = build_rows(app, &changed, focused);
    let rows: Vec<Row> = all_rows
        .into_iter()
        .skip(app.var_scroll)
        .take(visible)
        .collect();

    let widths = [
        Constraint::Percentage(30),
        Constraint::Percentage(25),
        Constraint::Percentage(45),
    ];

    let header_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let value_header = if app.var_col_scroll > 0 {
        "値 ◀"
    } else {
        "値"
    };
    let header = Row::new([
        Cell::from("名前").style(header_style),
        Cell::from("型").style(header_style),
        Cell::from(value_header).style(header_style),
    ])
    .height(1);

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

/// 全変数から表示行（Row）一覧を構築する
fn build_rows<'a>(
    app: &'a App,
    changed: &HashSet<&str>,
    focused: bool,
) -> Vec<Row<'static>> {
    let mut rows: Vec<Row<'static>> = Vec::new();
    let mut render_row_idx = 0usize;

    for var in &app.variables {
        let is_changed = changed.contains(var.name.as_str());
        let base_fg = if is_changed { Color::Green } else { Color::Reset };

        let expandable = is_expandable_array(&var.type_name, &var.value);
        tracing::debug!(
            "var={} type={} value={:?} expandable={}",
            var.name, var.type_name, var.value, expandable
        );
        let collapsed = app.collapsed_vars.contains(&var.name);

        // ヘッダー行
        let is_cursor = focused && render_row_idx == app.var_cursor;
        let header_style = make_style(base_fg, is_cursor);

        if expandable {
            let indicator = if collapsed { "▶" } else { "▼" };
            let name_cell = format!("{} {}", indicator, var.name);

            let value_cell = if collapsed {
                // 折りたたみ時: char配列なら文字列表示、それ以外は末尾省略
                let raw = if var.type_name.starts_with("char [") {
                    if var.value.trim().starts_with('{') {
                        // {N, N, N, ...} 数値リスト形式 → UTF-8文字列に変換
                        decode_numeric_char_array(&var.value)
                    } else {
                        // "\343\201\223..." 8進数エスケープ形式 → デコード
                        decode_gdb_octal_string(&var.value)
                    }
                } else {
                    truncate_value(&var.value)
                };
                skip_display_width(&raw, app.var_col_scroll).to_owned()
            } else {
                String::new()
            };

            rows.push(Row::new([
                Cell::from(name_cell).style(header_style),
                Cell::from(var.type_name.clone()).style(header_style),
                Cell::from(value_cell).style(header_style),
            ]));
            render_row_idx += 1;

            // 展開時: 要素を1行ずつ表示
            if !collapsed {
                let elements = parse_array_elements(&var.value);
                let is_char = var.type_name.starts_with("char [");
                let is_float = var.type_name.starts_with("double [") || var.type_name.starts_with("float [");

                for (i, elem) in elements.iter().enumerate() {
                    let elem_cursor = focused && render_row_idx == app.var_cursor;
                    let elem_style = make_style(base_fg, elem_cursor);

                    let raw_value = if is_char {
                        format_char_element(elem)
                    } else if is_float {
                        format_float_value(elem)
                    } else {
                        elem.clone()
                    };
                    let display_value = skip_display_width(&raw_value, app.var_col_scroll).to_owned();

                    rows.push(Row::new([
                        Cell::from(format!("  [{i}]")).style(elem_style),
                        Cell::from("").style(elem_style),
                        Cell::from(display_value).style(elem_style),
                    ]));
                    render_row_idx += 1;
                }
            }
        } else {
            // 通常変数（または "{" で始まらない型）
            let raw_value = if var.type_name.contains("char *") {
                format_char_ptr_value(&var.value)
            } else if var.type_name.starts_with("char [") {
                // ArrayValue で raw 値が保持されているのでここで一度だけデコードする
                let decoded = decode_gdb_octal_string(&var.value);
                tracing::debug!("char[] decode: input={:?} output={:?}", var.value, decoded);
                decoded
            } else if var.type_name == "double" || var.type_name == "float" {
                format_float_value(&var.value)
            } else {
                truncate_value(&var.value)
            };
            let display_value = skip_display_width(&raw_value, app.var_col_scroll).to_owned();

            rows.push(Row::new([
                Cell::from(var.name.clone()).style(header_style),
                Cell::from(var.type_name.clone()).style(header_style),
                Cell::from(display_value).style(header_style),
            ]));
            render_row_idx += 1;
        }
    }

    rows
}

/// char配列の1要素を "'H' (72)" 形式で返す
fn format_char_element(elem: &str) -> String {
    match elem.trim().parse::<u32>() {
        Ok(0) => "'\\0' (0)".to_string(),
        Ok(n) if (32..=126).contains(&n) => {
            let c = char::from_u32(n).unwrap_or('?');
            format!("'{}' ({})", c, n)
        }
        Ok(n) => format!("'\\x{:02x}' ({})", n, n),
        Err(_) => elem.to_string(),
    }
}

/// 行スタイルを生成する（変更色 + カーソルハイライト）
fn make_style(fg: Color, is_cursor: bool) -> Style {
    let mut style = if fg == Color::Reset {
        Style::default()
    } else {
        Style::default().fg(fg)
    };
    if is_cursor {
        style = style.bg(Color::DarkGray).add_modifier(Modifier::BOLD);
    }
    style
}
