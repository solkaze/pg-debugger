use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::compiler;
use crate::debugger::gdb::{GdbBackend, GdbEvent};
use crate::debugger::{Breakpoint, Variable};

#[derive(Default, Clone, Copy, PartialEq)]
pub enum Panel {
    #[default]
    Source,
    Vars,
    Console,
}

#[derive(Default, PartialEq)]
pub enum InputMode {
    #[default]
    Normal,
    BreakpointLine,
    GotoLine,
    StdinInput,
}

pub struct App {
    pub focused_panel: Panel,
    /// GDB バックエンド（実行ファイルが指定された場合のみ Some）
    gdb: Option<GdbBackend>,
    /// 再起動用の実行ファイルパス
    executable: Option<PathBuf>,
    /// 元のソースファイルパス（再コンパイル用。実行ファイル直接指定時は None）
    source_file: Option<PathBuf>,
    /// 現在停止しているソースファイル
    pub current_file: Option<PathBuf>,
    /// 現在停止している行番号
    pub current_line: Option<u32>,
    /// ステータスバーに表示するメッセージ
    pub status_message: String,
    /// ソースファイルの行キャッシュ
    pub source_lines: Vec<String>,
    /// キャッシュ済みファイルパス（変更検知用）
    loaded_file: Option<PathBuf>,
    /// 現在ステップの変数一覧
    pub variables: Vec<Variable>,
    /// 1 ステップ前の変数一覧（変更検知用）
    pub prev_variables: Vec<Variable>,
    /// 変数ビューのスクロールオフセット（表示行ベース）
    pub var_scroll: usize,
    /// 変数ビューのカーソル行（表示行ベース、0-origin）
    pub var_cursor: usize,
    /// 変数ビューの値カラム横スクロールオフセット（文字数）
    pub var_col_scroll: usize,
    /// 折りたたまれている配列変数名のセット
    pub collapsed_vars: HashSet<String>,
    /// コンソール出力行（最大 500 行）
    pub console_lines: Vec<String>,
    /// 改行待ちのコンソール行バッファ
    pub console_line_buf: String,
    /// 設定済みブレークポイント一覧
    pub breakpoints: Vec<Breakpoint>,
    /// ソースビューのカーソル行（1-origin）
    pub source_cursor: usize,
    /// ソースビューのスクロールオフセット（スキップ行数）
    pub source_scroll: usize,
    /// コンソールのスクロール位置（None = 自動スクロール、Some(n) = 手動スクロール）
    pub console_scroll: Option<usize>,
    /// 入力モード
    pub input_mode: InputMode,
    /// 行番号入力バッファ
    pub input_buffer: String,
    /// 標準入力バッファ（StdinInput モード中の入力文字列）
    pub stdin_buffer: String,
    /// 再起動要求フラグ（メインループで検知して await する）
    pub restart_requested: bool,
    /// プログラムが実行中（GdbEvent::Running 受信後、Stopped 受信前）
    program_running: bool,
}

impl App {
    /// アプリケーションを初期化する。
    /// executable が Some の場合は GDB を起動して main の先頭で停止させる。
    pub async fn new(executable: Option<PathBuf>, source_file: Option<PathBuf>) -> Result<Self> {
        let mut gdb = None;

        if let Some(ref exe) = executable {
            let backend = GdbBackend::new(exe).await?;
            backend.start()?;
            gdb = Some(backend);
        }

        Ok(Self {
            focused_panel: Panel::Source,
            gdb,
            executable,
            source_file,
            current_file: None,
            current_line: None,
            status_message: "準備完了 – n: Next  s: Step  f: Finish  c: Continue  q: Quit".to_string(),
            source_lines: Vec::new(),
            loaded_file: None,
            variables: Vec::new(),
            prev_variables: Vec::new(),
            var_scroll: 0,
            var_cursor: 0,
            var_col_scroll: 0,
            collapsed_vars: HashSet::new(),
            console_lines: Vec::new(),
            console_line_buf: String::new(),
            breakpoints: Vec::new(),
            source_cursor: 1,
            source_scroll: 0,
            console_scroll: None,
            input_mode: InputMode::Normal,
            input_buffer: String::new(),
            stdin_buffer: String::new(),
            restart_requested: false,
            program_running: false,
        })
    }

    /// GDB セッションを再起動する。
    /// ソースファイルがある場合は再コンパイルし、ブレークポイントを復元する。
    pub async fn restart(&mut self) {
        // 既存の GDB セッションを終了する
        self.gdb = None;

        // ソースファイルがある場合は再コンパイルする
        if let Some(ref src) = self.source_file.clone() {
            match compiler::compile_c(src).await {
                Ok(bin) => {
                    self.executable = Some(bin);
                }
                Err(e) => {
                    self.status_message = format!("コンパイルエラー: {}", e);
                    return;
                }
            }
        }

        // 実行ファイルがなければ再起動できない
        let exe = match self.executable.clone() {
            Some(p) => p,
            None => {
                self.status_message = "再起動エラー: 実行ファイルが不明です".to_string();
                return;
            }
        };

        // 新しい GDB セッションを起動する
        let backend = match GdbBackend::new(&exe).await {
            Ok(b) => b,
            Err(e) => {
                self.status_message = format!("GDB 起動エラー: {}", e);
                return;
            }
        };
        if let Err(e) = backend.start() {
            self.status_message = format!("GDB 起動エラー: {}", e);
            return;
        }

        // 保存済みブレークポイントを取り出してから GDB に再登録する
        let saved_bps = std::mem::take(&mut self.breakpoints);
        self.gdb = Some(backend);
        for bp in &saved_bps {
            if let Some(gdb) = &self.gdb {
                if let Err(e) = gdb.break_insert(&bp.file, bp.line) {
                    tracing::error!("BP 再登録エラー: {}", e);
                }
            }
        }
        // breakpoints は GdbEvent::BreakpointSet で再挿入される

        // 表示状態をリセットする
        self.current_line = None;
        self.current_file = None;
        self.source_lines.clear();
        self.loaded_file = None;
        self.variables.clear();
        self.prev_variables.clear();
        self.console_lines.clear();
        self.console_line_buf.clear();
        self.source_cursor = 1;
        self.source_scroll = 0;
        self.console_scroll = None;
        self.var_scroll = 0;
        self.var_cursor = 0;
        self.var_col_scroll = 0;
        self.collapsed_vars.clear();
        self.input_mode = InputMode::Normal;
        self.input_buffer.clear();
        self.stdin_buffer.clear();
        self.program_running = false;
        self.status_message = "再起動しました".to_string();
    }

    /// キー入力を処理する
    pub fn handle_key(&mut self, key: KeyEvent) {
        match self.input_mode {
            InputMode::StdinInput => match key.code {
                KeyCode::Char(c) => {
                    self.stdin_buffer.push(c);
                }
                KeyCode::Backspace => {
                    self.stdin_buffer.pop();
                }
                KeyCode::Enter => {
                    let text = self.stdin_buffer.clone();
                    if let Some(gdb) = &self.gdb {
                        if let Err(e) = gdb.send_input(&text) {
                            tracing::error!("stdin 送信エラー: {}", e);
                            self.status_message = format!("入力送信エラー: {}", e);
                        }
                    }
                    self.stdin_buffer.clear();
                    self.input_mode = InputMode::Normal;
                    self.console_scroll = None;
                }
                KeyCode::Esc => {
                    self.stdin_buffer.clear();
                    self.input_mode = InputMode::Normal;
                }
                _ => {}
            },
            InputMode::BreakpointLine => match key.code {
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    self.input_buffer.push(c);
                }
                KeyCode::Enter => {
                    if let Ok(line) = self.input_buffer.parse::<usize>() {
                        if line >= 1 && line <= self.source_lines.len() {
                            self.toggle_breakpoint(line);
                        }
                    }
                    self.input_buffer.clear();
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Esc => {
                    self.input_buffer.clear();
                    self.input_mode = InputMode::Normal;
                }
                _ => {}
            },
            InputMode::GotoLine => match key.code {
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    self.input_buffer.push(c);
                }
                KeyCode::Enter => {
                    if let Ok(line) = self.input_buffer.parse::<usize>() {
                        if line >= 1 && line <= self.source_lines.len() {
                            self.send_goto_line(line);
                        }
                    }
                    self.input_buffer.clear();
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Esc => {
                    self.input_buffer.clear();
                    self.input_mode = InputMode::Normal;
                }
                _ => {}
            },
            InputMode::Normal => match key.code {
                KeyCode::Tab => self.toggle_focus(),
                KeyCode::Char('n') | KeyCode::F(10) => self.send_next(),
                KeyCode::Char('s') => self.send_step(),
                KeyCode::Char('f') => self.send_finish(),
                KeyCode::Char('b') => self.toggle_breakpoint(self.source_cursor),
                KeyCode::Char('B') => {
                    self.input_mode = InputMode::BreakpointLine;
                    self.input_buffer.clear();
                }
                KeyCode::Char('g') => {
                    self.input_mode = InputMode::GotoLine;
                    self.input_buffer.clear();
                }
                KeyCode::Char('i') => {
                    self.input_mode = InputMode::StdinInput;
                    self.stdin_buffer.clear();
                }
                KeyCode::Char('r') => {
                    self.restart_requested = true;
                }
                KeyCode::Char('c') | KeyCode::F(5) => self.send_continue(),
                KeyCode::Enter => {
                    if self.focused_panel == Panel::Vars {
                        self.toggle_var_collapse();
                    }
                }
                KeyCode::Up => self.scroll_up(),
                KeyCode::Down => self.scroll_down(),
                KeyCode::Left => {
                    if self.focused_panel == Panel::Vars {
                        self.var_col_scroll = self.var_col_scroll.saturating_sub(2);
                    }
                }
                KeyCode::Right => {
                    if self.focused_panel == Panel::Vars {
                        self.var_col_scroll += 2;
                    }
                }
                KeyCode::PageUp => self.page_up(),
                KeyCode::PageDown => self.page_down(),
                _ => {}
            },
        }
    }

    /// GDB イベントをポーリングし、App の状態を更新する
    /// メインループの各フレームで呼び出す
    pub fn poll_gdb_events(&mut self) {
        let Some(gdb) = &mut self.gdb else { return };

        while let Some(event) = gdb.try_recv_event() {
            match event {
                GdbEvent::Stopped { file, line } => {
                    self.program_running = false;
                    self.status_message = format!("{}: {}行目", file.display(), line);
                    self.current_line = Some(line);
                    self.source_cursor = line as usize;
                    let view_height = 20usize;
                    let current_idx = (line as usize).saturating_sub(1);
                    self.source_scroll = current_idx.saturating_sub(view_height / 2);
                    if self.loaded_file.as_deref() != Some(file.as_path()) {
                        tracing::info!("ソースファイル読み込み: {}", file.display());
                        match std::fs::read_to_string(&file) {
                            Ok(content) => {
                                self.source_lines = content.lines().map(|l| l.to_string()).collect();
                                self.loaded_file = Some(file.clone());
                                tracing::info!("読み込み完了: {} 行", self.source_lines.len());
                            }
                            Err(e) => {
                                tracing::error!("ファイル読み込み失敗 {}: {}", file.display(), e);
                                self.source_lines.clear();
                                self.loaded_file = None;
                            }
                        }
                    }
                    self.current_file = Some(file.clone());
                    gdb.update_location(file, line);
                    // 停止のたびに変数一覧を要求する
                    if let Err(e) = gdb.request_variables() {
                        tracing::error!("変数要求エラー: {}", e);
                    }
                }
                GdbEvent::Running => {
                    self.status_message = "実行中...".to_string();
                    self.program_running = true;
                }
                GdbEvent::VariablesUpdated(vars) => {
                    tracing::info!("simple-values受信: {:?}", vars);
                    // 配列型変数・char*型変数の name を収集してから self.variables を更新する
                    // char* は simple-values の値が不正確な場合があるため -data-evaluate-expression で再取得する
                    let eval_names: Vec<String> = vars
                        .iter()
                        .filter(|v| v.type_name.contains('[') || v.type_name.contains("char *"))
                        .map(|v| v.name.clone())
                        .collect();

                    // 前の変数を保存してから新しい変数で更新する
                    self.prev_variables = std::mem::take(&mut self.variables);
                    self.variables = vars;

                    // カーソル位置を補正する（配列は未展開で計算）
                    let total = self.variables.len();
                    let max_pos = total.saturating_sub(1);
                    if self.var_cursor > max_pos {
                        self.var_cursor = max_pos;
                    }
                    if self.var_scroll > max_pos {
                        self.var_scroll = max_pos;
                    }

                    // 配列型・char*型変数の値を -data-evaluate-expression で個別に取得する
                    for name in &eval_names {
                        if let Err(e) = gdb.request_array_value(name) {
                            tracing::error!("評価値要求エラー: {}", e);
                        }
                    }
                }
                GdbEvent::ArrayValue { name, value } => {
                    tracing::info!("配列値受信: {} = {}", name, value);
                    if let Some(var) = self.variables.iter_mut().find(|v| v.name == name) {
                        tracing::debug!(
                            "ArrayValue更新: name={} type={} old={:?} new={:?}",
                            name, var.type_name, var.value, value
                        );
                        // raw値をそのまま保持する。デコードは var_view.rs で一度だけ行う。
                        var.value = value;
                    }
                }
                GdbEvent::ProgramOutput(text) => {
                    for ch in text.chars() {
                        if ch == '\n' {
                            let line = std::mem::take(&mut self.console_line_buf);
                            if self.console_lines.len() >= 500 {
                                self.console_lines.remove(0);
                            }
                            self.console_lines.push(line);
                        } else if ch != '\r' {
                            self.console_line_buf.push(ch);
                        }
                    }
                }
                GdbEvent::BreakpointSet(bp) => {
                    self.breakpoints.push(bp);
                }
                GdbEvent::BreakpointDeleted(id) => {
                    self.breakpoints.retain(|bp| bp.id != id);
                }
                GdbEvent::Error(msg) => {
                    self.status_message = format!("GDB エラー: {}", msg);
                }
            }
        }

    }

    /// -exec-next（ステップオーバー）を GDB に送信する
    fn send_next(&mut self) {
        let Some(gdb) = &self.gdb else { return };
        if let Err(e) = gdb.next() {
            tracing::error!("next 送信エラー: {}", e);
            self.status_message = format!("エラー: {}", e);
        }
    }

    /// -exec-step（ステップイン）を GDB に送信する
    fn send_step(&mut self) {
        let Some(gdb) = &self.gdb else { return };
        if let Err(e) = gdb.step() {
            tracing::error!("step 送信エラー: {}", e);
            self.status_message = format!("エラー: {}", e);
        }
    }

    /// -exec-finish（現在の関数を最後まで実行）を GDB に送信する
    fn send_finish(&mut self) {
        let Some(gdb) = &self.gdb else { return };
        if let Err(e) = gdb.finish() {
            tracing::error!("finish 送信エラー: {}", e);
            self.status_message = format!("エラー: {}", e);
        }
    }

    /// -exec-continue（実行継続）を GDB に送信する
    fn send_continue(&mut self) {
        let Some(gdb) = &self.gdb else { return };
        if let Err(e) = gdb.continue_exec() {
            tracing::error!("continue 送信エラー: {}", e);
            self.status_message = format!("エラー: {}", e);
        }
    }

    /// 一時ブレークポイントを挿入して指定行まで実行する
    fn send_goto_line(&mut self, line: usize) {
        let Some(file) = self.current_file.clone() else { return };
        let Some(gdb) = &self.gdb else { return };
        if let Err(e) = gdb.goto_line(&file, line) {
            tracing::error!("goto_line 送信エラー: {}", e);
            self.status_message = format!("エラー: {}", e);
        }
    }

    /// 指定行のブレークポイントをトグルする（bキー・Bキー共通）
    fn toggle_breakpoint(&mut self, line: usize) {
        let Some(file) = &self.current_file else { return };
        if self.gdb.is_none() { return; }

        let existing = self.breakpoints
            .iter()
            .find(|bp| bp.file == *file && bp.line == line as u32);

        if let Some(bp) = existing {
            let id = bp.id;
            if let Err(e) = self.gdb.as_ref().unwrap().break_delete(id) {
                tracing::error!("break_delete エラー: {}", e);
                self.status_message = format!("BP削除エラー: {}", e);
            } else {
                // GDB 17 は -break-delete 時に =breakpoint-deleted 通知を送らないため
                // コマンド送信成功時点で即座にリストから削除する
                self.breakpoints.retain(|bp| bp.id != id);
            }
        } else {
            if let Err(e) = self.gdb.as_ref().unwrap().break_insert(file, line as u32) {
                tracing::error!("break_insert エラー: {}", e);
                self.status_message = format!("BP追加エラー: {}", e);
            }
        }
    }

    fn toggle_focus(&mut self) {
        self.focused_panel = match self.focused_panel {
            Panel::Source => Panel::Vars,
            Panel::Vars => Panel::Console,
            Panel::Console => Panel::Source,
        };
    }

    fn scroll_up(&mut self) {
        match self.focused_panel {
            Panel::Source => {
                if self.source_cursor > 1 {
                    self.source_cursor -= 1;
                }
                // カーソルが画面上端を超えたらスクロール
                if self.source_cursor <= self.source_scroll {
                    self.source_scroll = self.source_scroll.saturating_sub(1);
                }
            }
            Panel::Vars => {
                if self.var_cursor > 0 {
                    self.var_cursor -= 1;
                }
                if self.var_cursor < self.var_scroll {
                    self.var_scroll = self.var_cursor;
                }
            }
            Panel::Console => {
                let view_height = 20usize;
                match self.console_scroll {
                    None => {
                        // 最下部から1行上へ
                        let max_skip = self.console_lines.len().saturating_sub(view_height);
                        self.console_scroll = Some(max_skip.saturating_sub(1));
                    }
                    Some(n) => {
                        self.console_scroll = Some(n.saturating_sub(1));
                    }
                }
            }
        }
    }

    fn scroll_down(&mut self) {
        match self.focused_panel {
            Panel::Source => {
                let max = self.source_lines.len().max(1);
                if self.source_cursor < max {
                    self.source_cursor += 1;
                }
                // カーソルが画面下端を超えたらスクロール
                let view_height = 20usize;
                if self.source_cursor > self.source_scroll + view_height {
                    self.source_scroll += 1;
                }
            }
            Panel::Vars => {
                let total = self.var_render_rows();
                if self.var_cursor + 1 < total {
                    self.var_cursor += 1;
                }
                let view_height = 20usize;
                if self.var_cursor >= self.var_scroll + view_height {
                    self.var_scroll = self.var_cursor + 1 - view_height;
                }
            }
            Panel::Console => {
                let view_height = 20usize;
                if let Some(n) = self.console_scroll {
                    let max_skip = self.console_lines.len().saturating_sub(view_height);
                    if n + 1 >= max_skip {
                        self.console_scroll = None;
                    } else {
                        self.console_scroll = Some(n + 1);
                    }
                }
                // None（最下部）のときは何もしない
            }
        }
    }

    fn page_up(&mut self) {
        if self.focused_panel != Panel::Console {
            return;
        }
        let view_height = 20usize;
        match self.console_scroll {
            None => {
                let max_skip = self.console_lines.len().saturating_sub(view_height);
                self.console_scroll = Some(max_skip.saturating_sub(view_height));
            }
            Some(n) => {
                self.console_scroll = Some(n.saturating_sub(view_height));
            }
        }
    }

    fn page_down(&mut self) {
        if self.focused_panel != Panel::Console {
            return;
        }
        let view_height = 20usize;
        if let Some(n) = self.console_scroll {
            let max_skip = self.console_lines.len().saturating_sub(view_height);
            if n + view_height >= max_skip {
                self.console_scroll = None;
            } else {
                self.console_scroll = Some(n + view_height);
            }
        }
    }

    /// カーソル位置の配列をトグル（展開/折りたたみ）する
    fn toggle_var_collapse(&mut self) {
        let cursor_info = self.var_cursor_var_index();
        // デバッグ: Enter を押したときカーソル状態をステータスバーに表示する
        self.status_message = format!(
            "cursor={}, var_cursor_var_index={:?}",
            self.var_cursor, cursor_info
        );
        if let Some((var_idx, true)) = cursor_info {
            let var = &self.variables[var_idx];
            if var.type_name.contains('[') && var.value.trim().starts_with('{') {
                let name = var.name.clone();
                if self.collapsed_vars.contains(&name) {
                    self.collapsed_vars.remove(&name);
                } else {
                    self.collapsed_vars.insert(name);
                }
            }
        }
    }

    /// カーソル行がどの変数のどの行かを返す（var_index, is_header）
    pub fn var_cursor_var_index(&self) -> Option<(usize, bool)> {
        let mut row = 0usize;
        for (i, var) in self.variables.iter().enumerate() {
            if row == self.var_cursor {
                return Some((i, true));
            }
            row += 1;
            if var.type_name.contains('[')
                && var.value.trim().starts_with('{')
                && !self.collapsed_vars.contains(&var.name)
            {
                let count = count_array_elements(&var.value);
                if self.var_cursor < row + count {
                    return Some((i, false));
                }
                row += count;
            }
        }
        None
    }

    /// 変数ビューの総表示行数を返す
    pub fn var_render_rows(&self) -> usize {
        let mut count = 0;
        for var in &self.variables {
            count += 1;
            if var.type_name.contains('[')
                && var.value.trim().starts_with('{')
                && !self.collapsed_vars.contains(&var.name)
            {
                count += count_array_elements(&var.value);
            }
        }
        count
    }

    /// カーソル位置の変数の完全な値を返す（ステータスバー表示用）
    pub fn var_cursor_full_value(&self) -> Option<String> {
        let (var_idx, _) = self.var_cursor_var_index()?;
        Some(self.variables[var_idx].value.clone())
    }
}

/// GDB の "{v1, v2, ...}" 形式から要素数を返す
fn count_array_elements(value: &str) -> usize {
    let trimmed = value.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        let inner = &trimmed[1..trimmed.len() - 1];
        if inner.trim().is_empty() {
            0
        } else {
            inner.split(',').count()
        }
    } else {
        0
    }
}
