use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::compiler;
use crate::debugger::gdb::{GdbBackend, GdbEvent};
use crate::debugger::{Breakpoint, StructMember, Variable};

/// ステップイン時に保存する呼び出し元フレームの表示状態
pub struct FrameView {
    pub source_lines: Vec<String>,
    /// ハイライト行（1-origin、赤でマーク）
    pub highlight_line: usize,
    /// このフレームが停止していたときの関数名
    pub func_name: String,
}

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
    /// 元のソースファイル一覧（再コンパイル用。実行ファイル直接指定時は空）
    source_files: Vec<PathBuf>,
    /// Makefile モードのターゲット（None=makeモードでない、Some(None)=デフォルトターゲット、Some(Some(t))=指定ターゲット）
    make_target: Option<Option<String>>,
    /// デバッグ対象プログラムへのコマンドライン引数
    prog_args: Vec<String>,
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
    /// ターミナルの高さ（メインループからフレームごとに更新）
    pub terminal_height: u16,
    /// ステップイン時に表示するコールスタック（[0]=最古の呼び出し元、last()=直前フレーム）
    pub frame_stack: Vec<FrameView>,
    /// 前回の Stopped 時点でのスタック深さ（初期値 1）
    prev_stack_depth: usize,
    /// 直前の Stopped イベントで保存した呼び出し元フレーム（StackDepth で使用）
    prev_stop_frame: Option<FrameView>,
    /// 未着の ArrayValue の数（0 になるまで display_variables を更新しない）
    pub pending_array_count: usize,
    /// 実際に変数ビューに表示する変数リスト（ArrayValue が全部揃ってから一括更新）
    pub display_variables: Vec<Variable>,
    /// 現在停止中の関数名
    pub current_func: String,
    /// グレーアウト機能の有効/無効（トグル）
    pub gray_out_enabled: bool,
}


/// StructMember ツリーを "a.b.c" 形式のパスで再帰的に探し、見つかったメンバの value を更新する。
fn update_member_value(members: &mut Vec<StructMember>, path: &str, value: &str) {
    let (head, tail) = match path.find('.') {
        Some(pos) => (&path[..pos], Some(&path[pos + 1..])),
        None => (path, None),
    };
    for member in members.iter_mut() {
        if member.name == head {
            match tail {
                None => {
                    member.value = value.to_string();
                }
                Some(rest) => {
                    update_member_value(&mut member.children, rest, value);
                }
            }
            return;
        }
    }
}

/// 型名が構造体型かどうかを判定する。
/// 配列型・ポインタ型・基本型は false を返す。
fn is_struct_type(type_name: &str) -> bool {
    let base_types = [
        "int", "long", "short", "float", "double",
        "char", "bool", "_Bool", "size_t",
        "unsigned", "signed", "void",
    ];
    let t = type_name.trim();
    // 配列・ポインタは除外
    if t.contains('[') || t.contains('*') {
        return false;
    }
    // 基本型は除外（完全一致 or "unsigned int" などのプレフィックス一致）
    if base_types.iter().any(|b| t == *b || t.starts_with(&format!("{} ", b))) {
        return false;
    }
    true
}

impl App {
    /// アプリケーションを初期化する。
    /// executable が Some の場合は GDB を起動して main の先頭で停止させる。
    pub async fn new(
        executable: Option<PathBuf>,
        source_files: Vec<PathBuf>,
        make_target: Option<Option<String>>,
        prog_args: Vec<String>,
    ) -> Result<Self> {
        let mut gdb = None;

        if let Some(ref exe) = executable {
            let backend = GdbBackend::new(exe).await?;
            backend.start(&prog_args)?;
            gdb = Some(backend);
        }

        Ok(Self {
            focused_panel: Panel::Source,
            gdb,
            executable,
            source_files,
            make_target,
            prog_args,
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
            terminal_height: 0,
            frame_stack: Vec::new(),
            prev_stack_depth: 1,
            prev_stop_frame: None,
            pending_array_count: 0,
            display_variables: Vec::new(),
            current_func: String::new(),
            gray_out_enabled: false,
        })
    }

    /// GDB セッションを再起動する。
    /// ソースファイルがある場合は再コンパイルし、ブレークポイントを復元する。
    pub async fn restart(&mut self) {
        // 既存の GDB セッションを終了する
        self.gdb = None;

        // ソースファイルがある場合は再コンパイルする
        if !self.source_files.is_empty() {
            let files: Vec<&str> = self.source_files
                .iter()
                .filter_map(|p| p.to_str())
                .collect();
            match compiler::compile_c_files(&files).await {
                Ok(bin) => {
                    self.executable = Some(bin);
                }
                Err(e) => {
                    self.status_message = format!("コンパイルエラー: {}", e);
                    return;
                }
            }
        } else if let Some(ref target) = self.make_target.clone() {
            match compiler::build_with_make(target.as_deref()).await {
                Ok(bin) => {
                    self.executable = Some(bin);
                }
                Err(e) => {
                    self.status_message = format!("makeエラー: {}", e);
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
        if let Err(e) = backend.start(&self.prog_args) {
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
        self.frame_stack.clear();
        self.prev_stack_depth = 1;
        self.prev_stop_frame = None;
        self.pending_array_count = 0;
        self.display_variables.clear();
        self.current_func = String::new();
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
                KeyCode::Char('h') => {
                    self.gray_out_enabled = !self.gray_out_enabled;
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
                GdbEvent::Stopped { file, line, func } => {
                    // prev_stop_frameはcurrent_funcを更新する前に保存する（呼び出し元の関数名を保持するため）
                    if self.current_file.is_some() {
                        self.prev_stop_frame = Some(FrameView {
                            source_lines: self.source_lines.clone(),
                            highlight_line: self.current_line.unwrap_or(0) as usize,
                            func_name: self.current_func.clone(),
                        });
                    }
                    self.current_func = func;
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
                    // 停止のたびに変数一覧とスタック深さを要求する
                    if let Err(e) = gdb.request_variables() {
                        tracing::error!("変数要求エラー: {}", e);
                    }
                    if let Err(e) = gdb.request_stack_depth() {
                        tracing::error!("スタック深さ要求エラー: {}", e);
                    }
                }
                GdbEvent::Running => {
                    self.status_message = "実行中...".to_string();
                    self.program_running = true;
                }
                GdbEvent::VariablesUpdated(vars) => {
                    tracing::info!("simple-values受信: {:?}", vars);
                    // 配列型・char*型・構造体型変数の name を収集してから self.variables を更新する
                    // これらは simple-values の値が不正確または空なので -data-evaluate-expression で再取得する
                    let eval_names: Vec<String> = vars
                        .iter()
                        .filter(|v| {
                            v.type_name.contains('[')
                                || v.type_name.contains("char *")
                                || is_struct_type(&v.type_name)
                        })
                        .map(|v| v.name.clone())
                        .collect();

                    // ArrayValue が届くまで display_variables の更新を保留するためカウントをセットする
                    self.pending_array_count = eval_names.len();

                    // 前の変数を保存してから新しい変数で更新する
                    self.prev_variables = std::mem::take(&mut self.variables);
                    self.variables = vars;

                    // 配列なしの場合は即座に表示用変数を更新する
                    if self.pending_array_count == 0 {
                        self.display_variables = self.variables.clone();
                    }
                    // pending 中は display_variables を更新しない（前回の表示を維持）

                    // カーソル・スクロール位置の補正はrenderに任せる
                    // ここでは補正しない（ArrayValueがまだ届いておらず配列展開状態が不確定なため）
                    // var_cursorがvar_scrollより小さくなる矛盾状態だけ防ぐ
                    if self.var_cursor < self.var_scroll {
                        self.var_scroll = self.var_cursor;
                    }

                    // 配列型・char*型変数の値を -data-evaluate-expression で個別に取得する
                    for name in &eval_names {
                        if let Err(e) = gdb.request_array_value(name) {
                            tracing::error!("評価値要求エラー: {}", e);
                        }
                    }
                    // 構造体型変数のメンバ型情報を -var-create で取得する
                    for var in &self.variables {
                        if is_struct_type(&var.type_name) {
                            if let Err(e) = gdb.request_struct_members(&var.name) {
                                tracing::error!("構造体メンバ要求エラー: {}", e);
                            }
                        }
                    }
                }
                GdbEvent::ArrayValue { name, value } => {
                    tracing::info!("evaluate-expression受信: {} = {:?}", name, value);
                    if let Some(var) = self.variables.iter_mut().find(|v| v.name == name) {
                        tracing::info!(
                            "evaluate-expression更新: name={} type={:?} old={:?} new={:?}",
                            name, var.type_name, var.value, value
                        );
                        // raw値をそのまま保持する。デコードは var_view.rs で一度だけ行う。
                        var.value = value;
                    }
                    // 全 ArrayValue が揃ったら display_variables を一括更新する
                    if self.pending_array_count > 0 {
                        self.pending_array_count -= 1;
                    }
                    if self.pending_array_count == 0 {
                        self.display_variables = self.variables.clone();
                    }
                }
                GdbEvent::StructMembers { var_name, members } => {
                    tracing::info!("StructMembers受信: {} ({} メンバ)", var_name, members.len());
                    // self.variables と display_variables 両方を更新する
                    if let Some(var) = self.variables.iter_mut().find(|v| v.name == var_name) {
                        var.members = Some(members.clone());
                    }
                    if let Some(var) = self.display_variables.iter_mut().find(|v| v.name == var_name) {
                        var.members = Some(members);
                    }
                }
                GdbEvent::CharArrayValue { var_name, member_name, value } => {
                    tracing::info!("CharArrayValue受信: {}.{} = {:?}", var_name, member_name, value);
                    // display_variables と variables 両方の該当メンバを更新する
                    if let Some(var) = self.display_variables.iter_mut().find(|v| v.name == var_name) {
                        if let Some(members) = &mut var.members {
                            update_member_value(members, &member_name, &value);
                        }
                    }
                    if let Some(var) = self.variables.iter_mut().find(|v| v.name == var_name) {
                        if let Some(members) = &mut var.members {
                            update_member_value(members, &member_name, &value);
                        }
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
                GdbEvent::StackDepth(depth) => {
                    let current_depth = depth;
                    let prev_depth = self.prev_stack_depth;

                    if current_depth > prev_depth {
                        // ステップイン：直前の停止位置（呼び出し元）をスタックに積む
                        if let Some(frame) = self.prev_stop_frame.take() {
                            self.frame_stack.push(frame);
                        }
                    } else if current_depth < prev_depth {
                        // ステップアウト：深さの差分だけポップする
                        let diff = prev_depth - current_depth;
                        for _ in 0..diff {
                            self.frame_stack.pop();
                        }
                    }
                    self.prev_stop_frame = None;
                    self.prev_stack_depth = current_depth;
                }
                GdbEvent::Exited => {
                    self.frame_stack.clear();
                    self.prev_stack_depth = 1;
                    self.prev_stop_frame = None;
                    self.program_running = false;
                    self.status_message = "プログラム終了（rキーで再起動）".to_string();
                    // display_variables は最終ステップ時点の値をそのまま保持する
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

    /// コンソール出力エリアの表示行数を計算する。
    /// ターミナル高さの約 30% からボーダー2行・入力行2行を引いた値。
    fn console_view_height(&self) -> usize {
        (self.terminal_height as usize)
            .saturating_mul(30)
            .saturating_div(100)
            .saturating_sub(4)
            .max(1)
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
                let view_height = self.console_view_height();
                if let Some(n) = self.console_scroll {
                    if n > 0 {
                        self.console_scroll = Some(n - 1);
                    }
                } else {
                    // 最下部から1行上へ
                    let max = self.console_lines.len().saturating_sub(view_height);
                    self.console_scroll = Some(max);
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
                let view_height = self.console_view_height();
                if let Some(n) = self.console_scroll {
                    let max = self.console_lines.len().saturating_sub(view_height);
                    if n + 1 >= max {
                        self.console_scroll = None; // 最下部で自動スクロールに戻る
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
        let view_height = self.console_view_height();
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
        let view_height = self.console_view_height();
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
        let Some(toggle_key) = self.cursor_collapse_key() else { return };
        self.status_message = format!("cursor={}, toggle={:?}", self.var_cursor, toggle_key);
        if self.collapsed_vars.contains(&toggle_key) {
            self.collapsed_vars.remove(&toggle_key);
        } else {
            self.collapsed_vars.insert(toggle_key);
        }
    }

    /// カーソル行のトグルキーを返す。展開不可の行なら None。
    fn cursor_collapse_key(&self) -> Option<String> {
        let (var_idx, member_path) = self.var_cursor_var_index()?;
        let var = &self.display_variables[var_idx];
        if let Some(path) = member_path {
            // メンバ行：typed members ツリーの中でパスに対応するノードを探す
            if let Some(ref members) = var.members {
                if find_member_at_path(members, &path, &var.name)
                    .map(|m| !m.children.is_empty())
                    .unwrap_or(false)
                {
                    return Some(path);
                }
            }
            None
        } else {
            // ヘッダ行
            let is_expandable = var.members.is_some()
                || (var.type_name.contains('[') && var.value.trim().starts_with('{'))
                || is_struct_value(&var.value);
            if is_expandable { Some(var.name.clone()) } else { None }
        }
    }

    /// カーソル行がどの変数のどの行かを返す（var_index, member_path）
    /// member_path: None = ヘッダ行、Some(path) = メンバ行のパス
    pub fn var_cursor_var_index(&self) -> Option<(usize, Option<String>)> {
        let mut row = 0usize;
        for (i, var) in self.display_variables.iter().enumerate() {
            if row == self.var_cursor {
                return Some((i, None));
            }
            row += 1;
            if !self.collapsed_vars.contains(&var.name) {
                if let Some(ref members) = var.members {
                    let result = find_cursor_in_members(
                        members,
                        &self.collapsed_vars,
                        &var.name,
                        &mut row,
                        self.var_cursor,
                        i,
                    );
                    if result.is_some() {
                        return result;
                    }
                } else if is_struct_value(&var.value) {
                    let child_count = count_struct_members(&var.value);
                    if self.var_cursor < row + child_count {
                        return Some((i, None));
                    }
                    row += child_count;
                } else if var.type_name.contains('[') && var.value.trim().starts_with('{') {
                    let child_count = count_array_elements(&var.value);
                    if self.var_cursor < row + child_count {
                        return Some((i, None));
                    }
                    row += child_count;
                }
            }
        }
        None
    }

    /// 変数ビューの総表示行数を返す
    pub fn var_render_rows(&self) -> usize {
        let mut count = 0;
        for var in &self.display_variables {
            count += 1;
            if !self.collapsed_vars.contains(&var.name) {
                if let Some(ref members) = var.members {
                    count += count_member_rows(members, &self.collapsed_vars, &var.name);
                } else if is_struct_value(&var.value) {
                    count += count_struct_members(&var.value);
                } else if var.type_name.contains('[') && var.value.trim().starts_with('{') {
                    count += count_array_elements(&var.value);
                }
            }
        }
        count
    }

    /// カーソル位置の変数の完全な値を返す（ステータスバー表示用）
    pub fn var_cursor_full_value(&self) -> Option<String> {
        let (var_idx, _) = self.var_cursor_var_index()?;
        Some(self.display_variables[var_idx].value.clone())
    }

    /// current_func の定義範囲を source_lines から推定して返す（1-origin）
    pub fn current_func_range(&self) -> Option<(usize, usize)> {
        let func_name = &self.current_func;
        if func_name.is_empty() {
            return None;
        }

        let lines = &self.source_lines;

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

    /// source_lines の line 行目（1-origin）が属する関数のスコープを返す（1-origin）
    pub fn func_range_at_line(&self, line: usize) -> Option<(usize, usize)> {
        let lines = &self.source_lines;

        // line 行目より前にある最後の関数定義を探す
        let mut func_start = None;
        for i in (0..line.min(lines.len())).rev() {
            let l = &lines[i];
            let trimmed = l.trim();
            if !l.starts_with(' ') && !l.starts_with('\t')
                && l.contains('(')
                && !trimmed.starts_with("if")
                && !trimmed.starts_with("for")
                && !trimmed.starts_with("while")
                && !trimmed.starts_with("switch")
                && !trimmed.starts_with("//")
                && !trimmed.starts_with('#')
            {
                func_start = Some(i);
                break;
            }
        }

        let func_start = func_start?;

        // func_start 以降で最初の { を探す
        let mut brace_start = None;
        for (i, l) in lines.iter().enumerate().skip(func_start) {
            if l.contains('{') {
                brace_start = Some(i);
                break;
            }
        }
        let brace_start = brace_start?;

        // 対応する } を深さを追って探す
        let mut depth = 0usize;
        let mut end_line = None;
        for (i, l) in lines.iter().enumerate().skip(brace_start) {
            for ch in l.chars() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        if depth > 0 { depth -= 1; }
                        if depth == 0 {
                            end_line = Some(i + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if end_line.is_some() { break; }
        }

        Some((func_start + 1, end_line.unwrap_or(lines.len())))
    }

    /// current_line が属する最も内側の {} ブロックの開始行と終了行を返す（1-origin）
    pub fn current_block_range(&self) -> Option<(usize, usize)> {
        let target = self.current_line? as usize;
        let lines = &self.source_lines;
        if lines.is_empty() {
            return None;
        }

        // 文字列・文字リテラル・行コメント内の {} を除いた (opens, closes) を返す
        fn net_braces(line: &str) -> (i32, i32) {
            let mut opens = 0i32;
            let mut closes = 0i32;
            let mut in_string = false;
            let mut in_char_lit = false;
            let mut prev = ' ';
            let chars: Vec<char> = line.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let ch = chars[i];
                match ch {
                    '"' if !in_char_lit && prev != '\\' => in_string = !in_string,
                    '\'' if !in_string && prev != '\\' => in_char_lit = !in_char_lit,
                    '/' if !in_string && !in_char_lit => {
                        if i + 1 < chars.len() && chars[i + 1] == '/' {
                            break;
                        }
                    }
                    '{' if !in_string && !in_char_lit => opens += 1,
                    '}' if !in_string && !in_char_lit => closes += 1,
                    _ => {}
                }
                prev = ch;
                i += 1;
            }
            (opens, closes)
        }

        // open_line 内の最後の { の文字位置を返す
        fn find_last_open_brace(line: &str) -> Option<usize> {
            let mut in_string = false;
            let mut in_char_lit = false;
            let mut prev = ' ';
            let mut last_pos = None;
            let chars: Vec<char> = line.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let ch = chars[i];
                match ch {
                    '"' if !in_char_lit && prev != '\\' => in_string = !in_string,
                    '\'' if !in_string && prev != '\\' => in_char_lit = !in_char_lit,
                    '/' if !in_string && !in_char_lit => {
                        if i + 1 < chars.len() && chars[i + 1] == '/' {
                            break;
                        }
                    }
                    '{' if !in_string && !in_char_lit => last_pos = Some(i),
                    _ => {}
                }
                prev = ch;
                i += 1;
            }
            last_pos
        }

        // start_line の start_char 以降から文字単位で前向き走査し、
        // depth=0 になった行を 1-origin で返す
        fn scan_for_close(
            lines: &[String],
            start_line: usize,
            start_char: usize,
        ) -> Option<usize> {
            let mut depth = 1i32;

            // start_line の start_char 以降を処理
            {
                let chars: Vec<char> = lines[start_line].chars().collect();
                let mut in_string = false;
                let mut in_char_lit = false;
                let mut prev = ' ';
                for j in start_char..chars.len() {
                    let ch = chars[j];
                    match ch {
                        '"' if !in_char_lit && prev != '\\' => in_string = !in_string,
                        '\'' if !in_string && prev != '\\' => in_char_lit = !in_char_lit,
                        '/' if !in_string && !in_char_lit => {
                            if j + 1 < chars.len() && chars[j + 1] == '/' {
                                break;
                            }
                        }
                        '{' if !in_string && !in_char_lit => depth += 1,
                        '}' if !in_string && !in_char_lit => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(start_line + 1); // 1-origin
                            }
                        }
                        _ => {}
                    }
                    prev = ch;
                }
            }

            // start_line+1 以降を行単位で処理
            for i in (start_line + 1)..lines.len() {
                let chars: Vec<char> = lines[i].chars().collect();
                let mut in_string = false;
                let mut in_char_lit = false;
                let mut prev = ' ';
                for j in 0..chars.len() {
                    let ch = chars[j];
                    match ch {
                        '"' if !in_char_lit && prev != '\\' => in_string = !in_string,
                        '\'' if !in_string && prev != '\\' => in_char_lit = !in_char_lit,
                        '/' if !in_string && !in_char_lit => {
                            if j + 1 < chars.len() && chars[j + 1] == '/' {
                                break;
                            }
                        }
                        '{' if !in_string && !in_char_lit => depth += 1,
                        '}' if !in_string && !in_char_lit => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(i + 1); // 1-origin
                            }
                        }
                        _ => {}
                    }
                    prev = ch;
                }
            }
            None
        }

        // ステップ1: target 行の1つ前から逆順走査で open_line を見つける
        let mut depth = 0i32;
        let mut open_line_0 = None;
        let mut open_brace_char = 0usize;

        for i in (0..target.saturating_sub(1)).rev() {
            let (opens, closes) = net_braces(&lines[i]);
            depth += closes - opens;
            if depth < 0 {
                open_line_0 = Some(i); // 0-indexed
                open_brace_char = find_last_open_brace(&lines[i])
                    .map(|p| p + 1) // { の次の文字から開始
                    .unwrap_or(0);
                break;
            }
        }

        let open_line_0 = open_line_0?;
        let open_line_1 = open_line_0 + 1; // 1-origin

        // ステップ2+3: open_line の { の直後から文字単位で前向き走査
        let close_line = scan_for_close(lines, open_line_0, open_brace_char)?;

        Some((open_line_1, close_line))
    }

    /// タイトルバー用のコールスタック文字列を返す。
    /// frame_stack が空なら現在のファイル名のみ、そうでなければ "func1 → func2 → ..." の形式。
    pub fn call_stack_title(&self) -> String {
        if self.frame_stack.is_empty() {
            self.current_file
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("(no file)")
                .to_string()
        } else {
            let mut parts: Vec<String> = self.frame_stack
                .iter()
                .map(|f| f.func_name.clone())
                .collect();
            parts.push(self.current_func.clone());
            parts.join(" → ")
        }
    }

    /// 左画面（呼び出し元）用のコールスタック文字列を返す。
    /// frame_stack の末尾を除いた関数名を "func1 → func2" の形式で返す。
    pub fn call_stack_title_frozen(&self) -> String {
        if self.frame_stack.is_empty() {
            return String::new();
        }
        let parts: Vec<String> = self.frame_stack
            .iter()
            .map(|f| f.func_name.clone())
            .collect();
        parts.join(" → ")
    }
}

/// typed members ツリーを再帰的に行数カウントする
fn count_member_rows(
    members: &[StructMember],
    collapsed_vars: &HashSet<String>,
    path: &str,
) -> usize {
    let mut count = 0;
    for member in members {
        count += 1;
        let member_path = format!("{}.{}", path, member.name);
        if !member.children.is_empty() && !collapsed_vars.contains(&member_path) {
            count += count_member_rows(&member.children, collapsed_vars, &member_path);
        }
    }
    count
}

/// typed members ツリーの中でカーソル行を再帰的に探す
fn find_cursor_in_members(
    members: &[StructMember],
    collapsed_vars: &HashSet<String>,
    path: &str,
    row: &mut usize,
    cursor: usize,
    var_idx: usize,
) -> Option<(usize, Option<String>)> {
    for member in members {
        let member_path = format!("{}.{}", path, member.name);
        if *row == cursor {
            return Some((var_idx, Some(member_path)));
        }
        *row += 1;
        if !member.children.is_empty() && !collapsed_vars.contains(&member_path) {
            let result = find_cursor_in_members(
                &member.children,
                collapsed_vars,
                &member_path,
                row,
                cursor,
                var_idx,
            );
            if result.is_some() {
                return result;
            }
        }
    }
    None
}

/// typed members ツリーの中でパスに対応するメンバを探す
fn find_member_at_path<'a>(
    members: &'a [StructMember],
    full_path: &str,
    current_prefix: &str,
) -> Option<&'a StructMember> {
    for member in members {
        let member_path = format!("{}.{}", current_prefix, member.name);
        if member_path == full_path {
            return Some(member);
        }
        if full_path.starts_with(&format!("{}.", member_path)) {
            return find_member_at_path(&member.children, full_path, &member_path);
        }
    }
    None
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

/// 値が構造体形式かどうか（"{" で始まり "name = value" パターンを含む）
fn is_struct_value(value: &str) -> bool {
    let trimmed = value.trim();
    if !trimmed.starts_with('{') {
        return false;
    }
    trimmed.contains(" = ")
}

/// 構造体値のメンバ数を返す（depth=1 のカンマ数 + 1）
fn count_struct_members(value: &str) -> usize {
    let mut depth = 0i32;
    let mut count = 1usize;
    for ch in value.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 1 => count += 1,
            _ => {}
        }
    }
    count
}
