use anyhow::Result;
use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, error};

use super::{Breakpoint, DebuggerState, StructMember, Variable};

/// GDB から通知されるイベント
#[derive(Debug, Clone)]
pub enum GdbEvent {
    /// プログラムが停止した（ステップ完了・ブレークポイント等）
    Stopped { file: PathBuf, line: u32, func: String },
    /// プログラムが実行中
    Running,
    /// 変数一覧が更新された（--simple-values の結果）
    VariablesUpdated(Vec<Variable>),
    /// 配列変数の値が取得できた（-data-evaluate-expression の結果）
    ArrayValue { name: String, value: String },
    /// 構造体メンバの型付き情報が取得できた（-var-list-children の結果）
    StructMembers { var_name: String, members: Vec<StructMember> },
    /// ブレークポイントが設定された
    BreakpointSet(Breakpoint),
    /// ブレークポイントが削除された
    BreakpointDeleted(u32),
    /// プログラムの標準出力
    ProgramOutput(String),
    /// エラー発生
    Error(String),
    /// スタック深さが取得できた（-stack-info-depth の結果）
    StackDepth(usize),
    /// プログラムが終了した（ファイル情報なしの停止）
    Exited,
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
    /// -data-evaluate-expression のトークン番号 → 変数名 のマッピング
    /// リーダータスクと共有するため Arc<Mutex<...>>
    pending_evals: Arc<Mutex<HashMap<u64, String>>>,
    /// -var-create のトークン番号 → 元の変数名 のマッピング
    pending_var_creates: Arc<Mutex<HashMap<u64, String>>>,
    /// -var-list-children のトークン番号 → (GDB var 名, 元の変数名) のマッピング
    pending_var_lists: Arc<Mutex<HashMap<u64, (String, String)>>>,
    /// 次に使うトークン番号（2 以上。1 は stack-list-variables 用）
    /// リーダータスクと共有するため Arc<Mutex<...>>
    next_token: Arc<Mutex<u64>>,
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

        let pending_evals: Arc<Mutex<HashMap<u64, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_evals_reader = Arc::clone(&pending_evals);

        let pending_var_creates: Arc<Mutex<HashMap<u64, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_var_creates_reader = Arc::clone(&pending_var_creates);

        let pending_var_lists: Arc<Mutex<HashMap<u64, (String, String)>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_var_lists_reader = Arc::clone(&pending_var_lists);

        let next_token_arc: Arc<Mutex<u64>> = Arc::new(Mutex::new(2));
        let next_token_reader = Arc::clone(&next_token_arc);

        // GDB stdout を非同期で読み続けるタスク
        let event_tx_reader = event_tx.clone();
        let cmd_tx_reader = cmd_tx.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        tracing::info!("gdb< {}", line);
                        if let Some(event) = parse_gdb_line(
                            &line,
                            &pending_evals_reader,
                            &pending_var_creates_reader,
                            &pending_var_lists_reader,
                            &next_token_reader,
                            &cmd_tx_reader,
                        ) {
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
            pending_evals,
            pending_var_creates,
            pending_var_lists,
            next_token: next_token_arc,
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

    /// 現在のスタックフレームの変数一覧を要求する（--simple-values）
    /// name + type + 単純型の value が得られる。配列型は value が省略される。
    pub fn request_variables(&self) -> Result<()> {
        self.send_command("1-stack-list-variables --simple-values")
    }

    /// スタック深さを -stack-info-depth で取得する
    pub fn request_stack_depth(&self) -> Result<()> {
        self.send_command("-stack-info-depth")
    }

    /// 構造体変数のメンバ型情報を -var-create / -var-list-children で取得する
    pub fn request_struct_members(&self, var_name: &str) -> Result<()> {
        let token = {
            let mut t = self.next_token.lock().unwrap();
            *t += 1;
            *t
        };
        {
            let mut map = self.pending_var_creates.lock().unwrap();
            map.insert(token, var_name.to_string());
        }
        self.send_command(&format!("{}-var-create - * {}", token, var_name))
    }

    /// 配列型変数の値を -data-evaluate-expression で個別取得する
    pub fn request_array_value(&self, var_name: &str) -> Result<()> {
        let token = {
            let mut t = self.next_token.lock().unwrap();
            *t += 1;
            *t
        };
        {
            let mut map = self.pending_evals.lock().unwrap();
            map.insert(token, var_name.to_string());
        }
        self.send_command(&format!(
            "{}-data-evaluate-expression \"{}\"",
            token, var_name
        ))
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
fn parse_gdb_line(
    line: &str,
    pending_evals: &Mutex<HashMap<u64, String>>,
    pending_var_creates: &Mutex<HashMap<u64, String>>,
    pending_var_lists: &Mutex<HashMap<u64, (String, String)>>,
    next_token: &Arc<Mutex<u64>>,
    cmd_tx: &mpsc::Sender<String>,
) -> Option<GdbEvent> {
    if line.starts_with("*stopped") {
        let file = extract_value(line, "fullname").map(PathBuf::from);
        let line_no = extract_value(line, "line").and_then(|s| s.parse::<u32>().ok());
        let func = extract_value(line, "func").unwrap_or_default();

        match (file, line_no) {
            (Some(file), Some(line)) => Some(GdbEvent::Stopped { file, line, func }),
            _ => {
                // ファイル情報がない停止（プログラム終了等）
                debug!("停止イベントにファイル情報なし: {}", line);
                Some(GdbEvent::Exited)
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
        // --simple-values レスポンス: name + type + 単純型の value
        let vars = parse_variables_response(line).unwrap_or_default();
        tracing::info!("simple-values応答: {:?}", vars);
        Some(GdbEvent::VariablesUpdated(vars))
    } else if line.contains("^done,depth=") {
        // -stack-info-depth レスポンス
        extract_value(line, "depth")
            .and_then(|s| s.parse::<usize>().ok())
            .map(GdbEvent::StackDepth)
    } else if line.contains(",children=[") {
        // -var-list-children レスポンス
        parse_var_list_children_response(line, pending_var_lists, cmd_tx)
    } else if line.contains("^done,name=") && line.contains(",numchild=") {
        // -var-create レスポンス（イベントは発行せず follow-up コマンドを送信）
        handle_var_create_response(line, pending_var_creates, pending_var_lists, next_token, cmd_tx);
        None
    } else if line.contains("^done,value=") {
        // -data-evaluate-expression レスポンス（配列値）
        parse_array_value_response(line, pending_evals)
    } else if line.starts_with('@') {
        // GDB/MI 経由でのプログラム出力（set inferior-tty 使用時は通常発生しない）
        parse_program_output(line).map(GdbEvent::ProgramOutput)
    } else {
        None
    }
}

/// `N^done,name="var1",numchild="K",...` をパースし、
/// pending_var_creates からトークン→変数名を解決して -var-list-children を送信する
fn handle_var_create_response(
    line: &str,
    pending_var_creates: &Mutex<HashMap<u64, String>>,
    pending_var_lists: &Mutex<HashMap<u64, (String, String)>>,
    next_token: &Arc<Mutex<u64>>,
    cmd_tx: &mpsc::Sender<String>,
) {
    let Some(caret) = line.find('^') else { return };
    let Ok(token) = line[..caret].parse::<u64>() else { return };

    let orig_var_name = {
        let mut map = pending_var_creates.lock().unwrap();
        match map.remove(&token) {
            Some(v) => v,
            None => return, // このトークンは pending_var_creates にない
        }
    };

    let gdb_var_name = match extract_value(line, "name") {
        Some(n) => n,
        None => return,
    };
    let numchild: usize = extract_value(line, "numchild")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    if numchild == 0 {
        // 子なし → var オブジェクトを削除して終了
        let _ = cmd_tx.try_send(format!("-var-delete {}", gdb_var_name));
        return;
    }

    // var-list-children 用の新しいトークンを割り当てる
    let list_token = {
        let mut t = next_token.lock().unwrap();
        *t += 1;
        *t
    };
    {
        let mut map = pending_var_lists.lock().unwrap();
        map.insert(list_token, (gdb_var_name.clone(), orig_var_name));
    }
    let _ = cmd_tx.try_send(format!(
        "{}-var-list-children --all-values {}",
        list_token, gdb_var_name
    ));
}

/// `M^done,numchild="K",children=[child={...},...]` をパースして StructMembers を返す
fn parse_var_list_children_response(
    line: &str,
    pending_var_lists: &Mutex<HashMap<u64, (String, String)>>,
    cmd_tx: &mpsc::Sender<String>,
) -> Option<GdbEvent> {
    let caret = line.find('^')?;
    let token: u64 = line[..caret].parse().ok()?;

    let (gdb_var_name, orig_var_name) = {
        let mut map = pending_var_lists.lock().unwrap();
        map.remove(&token)?
    };

    let members = parse_children(line).unwrap_or_default();

    // var オブジェクトを削除する
    let _ = cmd_tx.try_send(format!("-var-delete {}", gdb_var_name));

    tracing::info!(
        "var-list-children応答: {} → {} メンバ",
        orig_var_name,
        members.len()
    );
    Some(GdbEvent::StructMembers { var_name: orig_var_name, members })
}

/// `children=[child={name="v.x",exp="x",numchild="0",value="10",type="int"},...` をパースする
fn parse_children(line: &str) -> Option<Vec<StructMember>> {
    let start = line.find("children=[")? + "children=[".len();
    let list = &line[start..];

    let mut members = Vec::new();
    let mut remaining = list;

    loop {
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
                '\\' => { chars.next(); }
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

        // exp フィールドがメンバ名（"var1.x" でなく "x" の部分）
        let name = extract_quoted_value(block, "exp").unwrap_or_default();
        let type_name = extract_quoted_value(block, "type").unwrap_or_default();
        let value = extract_quoted_value(block, "value").unwrap_or_default();
        let num_children: usize = extract_quoted_value(block, "numchild")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        if !name.is_empty() {
            members.push(StructMember { name, type_name, value, num_children });
        }
    }

    Some(members)
}

/// `N^done,value="..."` をパースし、pending_evals からトークン→変数名を解決して ArrayValue を返す
fn parse_array_value_response(
    line: &str,
    pending_evals: &Mutex<HashMap<u64, String>>,
) -> Option<GdbEvent> {
    // トークン番号を行頭から抽出する
    let caret = line.find('^')?;
    let token: u64 = line[..caret].parse().ok()?;

    // pending_evals からトークンに対応する変数名を取り出す
    let var_name = {
        let mut map = pending_evals.lock().unwrap();
        map.remove(&token)?
    };

    // `^done,value="..."` の value をバックスラッシュ保持でraw取り出しする
    let value = extract_raw_array_value(line, caret)?;

    tracing::info!("配列値応答: {} = {}", var_name, value);
    Some(GdbEvent::ArrayValue { name: var_name, value })
}

/// GDB MI の `N^done,value="..."` から value をバックスラッシュを保持したまま取り出す。
/// 閉じクォートの後に続く `, '\NNN' <repeats N times>` 形式の繰り返し表記も展開する。
fn extract_raw_array_value(line: &str, caret: usize) -> Option<String> {
    let value_prefix = "^done,value=\"";
    let after_caret = &line[caret..];
    let inner_start = after_caret.find(value_prefix)? + value_prefix.len();
    let rest = &after_caret[inner_start..];

    let mut result = String::new();
    let mut char_iter = rest.char_indices();
    let mut close_idx = rest.len();

    // メインのクォート文字列をバックスラッシュ保持でそのまま取り出す
    loop {
        match char_iter.next() {
            None => break,
            Some((i, '"')) => {
                close_idx = i + 1;
                break;
            }
            Some((_, '\\')) => {
                // バックスラッシュと次の文字を両方保持する（8進数エスケープを保つため）
                result.push('\\');
                if let Some((_, c)) = char_iter.next() {
                    result.push(c);
                }
            }
            Some((_, c)) => result.push(c),
        }
    }

    // 閉じクォートの後に続く繰り返し表記を展開する
    // 例: `, '\000' <repeats 5 times>`
    append_gdb_repeat_notations(&mut result, &rest[close_idx..]);

    Some(result)
}

/// GDB の繰り返し表記 `, '\NNN' <repeats N times>` を解析して result に展開追記する。
/// 8進数エスケープはバックスラッシュを保持したまま追加し、decode_gdb_octal_string で後処理する。
fn append_gdb_repeat_notations(result: &mut String, s: &str) {
    let mut pos = 0;
    let bytes = s.as_bytes();

    loop {
        // 空白をスキップ
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }

        // `,` を期待
        if pos >= bytes.len() || bytes[pos] != b',' {
            break;
        }
        pos += 1;

        // 空白をスキップ
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }

        // `'` を期待
        if pos >= bytes.len() || bytes[pos] != b'\'' {
            break;
        }
        pos += 1;

        // エスケープパターンを raw 取り出し（例: \000 → "\\000"）
        let mut pattern = String::new();
        if pos < bytes.len() && bytes[pos] == b'\\' {
            pattern.push('\\');
            pos += 1;
            // 8進数の場合は最大3桁取得
            let mut digit_count = 0;
            while pos < bytes.len() && digit_count < 3 && bytes[pos] >= b'0' && bytes[pos] <= b'7'
            {
                pattern.push(bytes[pos] as char);
                pos += 1;
                digit_count += 1;
            }
            // 8進数以外のエスケープ（\n 等）は1文字取得
            if digit_count == 0 && pos < bytes.len() {
                pattern.push(bytes[pos] as char);
                pos += 1;
            }
        } else if pos < bytes.len() && bytes[pos] != b'\'' {
            pattern.push(bytes[pos] as char);
            pos += 1;
        }

        // 閉じ `'` を期待
        if pos >= bytes.len() || bytes[pos] != b'\'' {
            break;
        }
        pos += 1;

        // 空白をスキップ
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }

        // `<repeats ` を期待
        let repeats_prefix = b"<repeats ";
        if pos + repeats_prefix.len() > bytes.len()
            || &bytes[pos..pos + repeats_prefix.len()] != repeats_prefix
        {
            break;
        }
        pos += repeats_prefix.len();

        // 繰り返し回数を取得
        let count_start = pos;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            pos += 1;
        }
        let count: usize = match s[count_start..pos].parse() {
            Ok(n) => n,
            Err(_) => break,
        };

        // 空白をスキップ
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }

        // `times>` を期待
        let times_suffix = b"times>";
        if pos + times_suffix.len() > bytes.len()
            || &bytes[pos..pos + times_suffix.len()] != times_suffix
        {
            break;
        }
        pos += times_suffix.len();

        // パターンを count 回追加
        for _ in 0..count {
            result.push_str(&pattern);
        }
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
            vars.push(Variable { name, value, type_name, members: None });
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
