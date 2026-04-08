use anyhow::Result;
use std::os::fd::OwnedFd;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, error};

use super::{Breakpoint, DebuggerState, Variable};

/// GDB から通知されるイベント
#[derive(Debug, Clone)]
pub enum GdbEvent {
    /// プログラムが停止した（ステップ完了・ブレークポイント等）
    Stopped { file: PathBuf, line: u32 },
    /// プログラムが実行中
    Running,
    /// 変数の型情報を取得した（--no-values の結果）: (name, type_name) のリスト
    VariableTypesReceived(Vec<(String, String)>),
    /// 変数一覧が更新された
    VariablesUpdated(Vec<Variable>),
    /// ブレークポイントが設定された
    BreakpointSet(Breakpoint),
    /// ブレークポイントが削除された
    BreakpointDeleted(u32),
    /// プログラムの標準出力
    ProgramOutput(String),
    /// エラー発生
    Error(String),
}

/// GDB/MI バックエンド
pub struct GdbBackend {
    /// GDB へのコマンド送信チャネル
    cmd_tx: mpsc::Sender<String>,
    /// GDB からのイベント受信チャネル
    event_rx: mpsc::Receiver<GdbEvent>,
    state: DebuggerState,
    /// inferior に割り当てた PTY のスレーブデバイスパス（/dev/pts/XX）
    pts_path: PathBuf,
    /// スレーブ fd を保持（ドロップすると inferior が EIO を受け取る）
    _slave_fd: OwnedFd,
    /// PTY マスターへの書き込みハンドル（inferior の stdin に送信する）
    pty_master_write: Mutex<std::fs::File>,
}

impl GdbBackend {
    /// GDB を子プロセスとして起動し、非同期読み取りタスクを開始する
    pub async fn new(executable: &Path) -> Result<Self> {
        // PTY のマスター/スレーブペアを作成する
        let pty = nix::pty::openpty(None, None)?;

        // スレーブ fd 番号から /dev/pts/XX のパスを解決する
        let slave_fd_no = pty.slave.as_raw_fd();
        let pts_path = std::fs::read_link(format!("/proc/self/fd/{}", slave_fd_no))?;

        let mut child = Command::new("gdb")
            .arg("-i=mi")
            .arg(executable)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take().expect("stdin が取得できない");
        let stdout = child.stdout.take().expect("stdout が取得できない");

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(32);
        let (event_tx, event_rx) = mpsc::channel::<GdbEvent>(32);

        // GDB stdout を非同期で読み続けるタスク
        let event_tx_reader = event_tx.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        tracing::info!("gdb< {}", line);
                        if let Some(event) = parse_gdb_line(&line) {
                            if event_tx_reader.send(event).await.is_err() {
                                break;
                            }
                        }
                    }
                    Ok(None) => {
                        debug!("GDB stdout が閉じられた");
                        break;
                    }
                    Err(e) => {
                        error!("GDB stdout 読み取りエラー: {}", e);
                        break;
                    }
                }
            }
        });

        // GDB stdin へコマンドを書き込むタスク
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(cmd) = cmd_rx.recv().await {
                debug!("gdb> {}", cmd);
                let line = format!("{}\n", cmd);
                if let Err(e) = stdin.write_all(line.as_bytes()).await {
                    error!("GDB stdin 書き込みエラー: {}", e);
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        // GDB プロセスの終了を監視するタスク
        tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => debug!("GDB プロセス終了: {}", status),
                Err(e) => error!("GDB 待機エラー: {}", e),
            }
        });

        // PTY マスターから行を読み続け、ProgramOutput イベントを送る。
        // ブロッキング読み取りなので専用スレッドで動かす。
        // まずマスター OwnedFd を File に変換し、書き込み用に複製する。
        let master_file = unsafe { std::fs::File::from_raw_fd(pty.master.into_raw_fd()) };
        let pty_master_write = master_file.try_clone()?;

        let event_tx_pty = event_tx.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut master_file = master_file;
            let mut buf = [0u8; 256];
            loop {
                match master_file.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                        if event_tx_pty
                            .blocking_send(GdbEvent::ProgramOutput(chunk))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            cmd_tx,
            event_rx,
            state: DebuggerState::new(),
            pts_path,
            _slave_fd: pty.slave,
            pty_master_write: Mutex::new(pty_master_write),
        })
    }

    /// GDB にコマンドを送信する（非ブロッキング）
    fn send_command(&self, cmd: &str) -> Result<()> {
        self.cmd_tx
            .try_send(cmd.to_string())
            .map_err(|e| anyhow::anyhow!("GDB コマンド送信失敗: {}", e))
    }

    /// inferior の TTY を PTY スレーブに設定してから実行開始する
    pub fn start(&self) -> Result<()> {
        self.send_command(&format!(
            "-interpreter-exec console \"set inferior-tty {}\"",
            self.pts_path.display()
        ))?;
        // シェル経由でなく直接プログラムを起動することで LD_PRELOAD 等の干渉を防ぐ
        self.send_command("-interpreter-exec console \"set startup-with-shell off\"")?;
        self.send_command("-break-insert main")?;
        self.send_command("-exec-run")?;
        Ok(())
    }

    /// -exec-next（ステップオーバー）を送信する
    pub fn next(&self) -> Result<()> {
        self.send_command("-exec-next")
    }

    /// -exec-continue（実行継続）を送信する
    pub fn continue_exec(&self) -> Result<()> {
        self.send_command("-exec-continue")
    }

    /// -exec-step（ステップイン）を送信する
    pub fn step(&self) -> Result<()> {
        self.send_command("-exec-step")
    }

    /// -exec-finish（現在の関数を最後まで実行）を送信する
    pub fn finish(&self) -> Result<()> {
        self.send_command("-exec-finish")
    }

    /// -break-insert でブレークポイントを設定する
    pub fn break_insert(&self, file: &Path, line: u32) -> Result<()> {
        self.send_command(&format!("-break-insert {}:{}", file.display(), line))
    }

    /// -break-delete でブレークポイントを削除する
    pub fn break_delete(&self, id: u32) -> Result<()> {
        self.send_command(&format!("-break-delete {}", id))
    }

    /// 一時ブレークポイントを挿入して -exec-continue する（行ジャンプ）
    pub fn goto_line(&self, file: &Path, line: usize) -> Result<()> {
        self.send_command(&format!("-break-insert -t {}:{}", file.display(), line))?;
        self.send_command("-exec-continue")
    }

    /// 現在のスタックフレームの変数一覧を要求する
    /// 2 段階で取得する:
    ///   トークン 1: --no-values  → name + type を取得
    ///   トークン 2: --all-values → name + value を取得（配列含む）
    /// App 側でマージして VariablesUpdated を生成する。
    pub fn request_variables(&self) -> Result<()> {
        self.send_command("1-stack-list-variables --no-values")?;
        self.send_command("2-stack-list-variables --all-values")
    }

    /// inferior の stdin にテキストを送信する（PTY マスターに書き込む）
    pub fn send_input(&self, text: &str) -> Result<()> {
        use std::io::Write;
        let mut file = self.pty_master_write.lock().unwrap();
        writeln!(file, "{}", text)?;
        file.flush()?;
        Ok(())
    }

    /// 届いている GDB イベントを 1 件取り出す（なければ None）
    pub fn try_recv_event(&mut self) -> Option<GdbEvent> {
        self.event_rx.try_recv().ok()
    }

    /// 現在の停止位置をステートに保存する
    pub fn update_location(&mut self, file: PathBuf, line: u32) {
        self.state.file = Some(file);
        self.state.line = Some(line);
    }

    pub fn get_state(&self) -> &DebuggerState {
        &self.state
    }
}

/// GDB/MI の 1 行出力を解析し、対応するイベントを返す
fn parse_gdb_line(line: &str) -> Option<GdbEvent> {
    if line.starts_with("*stopped") {
        let file = extract_value(line, "fullname").map(PathBuf::from);
        let line_no = extract_value(line, "line").and_then(|s| s.parse::<u32>().ok());

        match (file, line_no) {
            (Some(file), Some(line)) => Some(GdbEvent::Stopped { file, line }),
            _ => {
                // ファイル情報がない停止（プログラム終了等）
                debug!("停止イベントにファイル情報なし: {}", line);
                None
            }
        }
    } else if line.starts_with("*running") {
        Some(GdbEvent::Running)
    } else if line.starts_with("^done,bkpt=") {
        parse_breakpoint_response(line).map(GdbEvent::BreakpointSet)
    } else if line.starts_with("=breakpoint-deleted") {
        extract_value(line, "id")
            .and_then(|s| s.parse::<u32>().ok())
            .map(GdbEvent::BreakpointDeleted)
    } else if line.starts_with("1^done,variables=") {
        // --no-values レスポンス: name と type のみ
        let types = parse_variables_response(line)
            .unwrap_or_default()
            .into_iter()
            .map(|v| (v.name, v.type_name))
            .collect();
        Some(GdbEvent::VariableTypesReceived(types))
    } else if line.starts_with("2^done,variables=") {
        // --all-values レスポンス: name と value のみ（型は別途マージ）
        let vars = parse_variables_response(line).unwrap_or_default();
        Some(GdbEvent::VariablesUpdated(vars))
    } else if line.starts_with('@') {
        // GDB/MI 経由でのプログラム出力（set inferior-tty 使用時は通常発生しない）
        parse_program_output(line).map(GdbEvent::ProgramOutput)
    } else {
        None
    }
}

/// `^done,bkpt={number="1",...,fullname="...",...,line="4",...}` をパースして Breakpoint を返す
fn parse_breakpoint_response(line: &str) -> Option<Breakpoint> {
    let id = extract_value(line, "number").and_then(|s| s.parse::<u32>().ok())?;
    let file = extract_value(line, "fullname").map(PathBuf::from)?;
    let line_no = extract_value(line, "line").and_then(|s| s.parse::<u32>().ok())?;
    Some(Breakpoint { id, file, line: line_no, enabled: true })
}

/// GDB/MI 出力行から `key="value"` 形式の値を取り出す（エスケープ非対応・単純版）
fn extract_value(line: &str, key: &str) -> Option<String> {
    let pattern = format!("{}=\"", key);
    let start = line.find(&pattern)? + pattern.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// `^done,variables=[{name="x",type="int",value="1"},...]` をパースして
/// Variable のベクタを返す
fn parse_variables_response(line: &str) -> Option<Vec<Variable>> {
    let start = line.find("variables=[")? + "variables=[".len();
    let list = &line[start..];

    let mut vars = Vec::new();
    let mut remaining = list;

    loop {
        // 次の `{` を探す
        let brace_open = match remaining.find('{') {
            Some(i) => i,
            None => break,
        };
        remaining = &remaining[brace_open + 1..];

        // 対応する `}` を深さを追って探す
        let mut depth = 1usize;
        let mut end = None;
        let mut chars = remaining.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => { chars.next(); } // エスケープをスキップ
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }

        let end = match end {
            Some(e) => e,
            None => break,
        };

        let block = &remaining[..end];
        remaining = &remaining[end + 1..];

        // ブロック内から name / type / value を抽出
        let name = extract_quoted_value(block, "name").unwrap_or_default();
        let type_name = extract_quoted_value(block, "type").unwrap_or_default();
        let value = extract_quoted_value(block, "value").unwrap_or("...".to_string());

        if !name.is_empty() {
            vars.push(Variable { name, value, type_name });
        }
    }

    Some(vars)
}

/// GDB/MI のプログラム出力行 `@"..."` をパースしてテキストを返す
fn parse_program_output(line: &str) -> Option<String> {
    let rest = line.strip_prefix('@')?;
    let rest = rest.strip_prefix('"')?;
    let rest = rest.strip_suffix('"').unwrap_or(rest);

    let mut result = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next() {
            None => break,
            Some('\\') => match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('\\') => result.push('\\'),
                Some('"') => result.push('"'),
                Some(c) => { result.push('\\'); result.push(c); }
                None => break,
            },
            Some(c) => result.push(c),
        }
    }
    Some(result)
}

/// エスケープを考慮した `key="..."` 値の抽出
fn extract_quoted_value(text: &str, key: &str) -> Option<String> {
    let pattern = format!("{}=\"", key);
    let start = text.find(&pattern)? + pattern.len();
    let rest = &text[start..];

    let mut result = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next()? {
            '\\' => {
                // エスケープされた文字をそのまま取り込む
                if let Some(c) = chars.next() {
                    result.push(c);
                }
            }
            '"' => break,
            c => result.push(c),
        }
    }
    Some(result)
}
