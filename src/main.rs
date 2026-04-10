use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;

mod app;
mod compiler;
mod debugger;
mod gdb_utils;
mod ui;

use app::App;

#[tokio::main]
async fn main() -> Result<()> {
    // ログはファイルに書き出す（TUI と stdout が競合しないよう）
    let log_file = std::fs::File::create("debug.log")?;
    tracing_subscriber::fmt().with_writer(log_file).init();

    // コマンドライン引数から実行ファイルパスを取得
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        eprintln!("使い方: pg-debugger <ファイル.c> [ファイル2.c ...]");
        eprintln!("        pg-debugger --make [ターゲット]");
        std::process::exit(1);
    }

    let executable: Option<std::path::PathBuf>;
    let source_files: Vec<std::path::PathBuf>;
    let make_target: Option<Option<String>>;

    if args[0] == "--make" {
        // Makefile モード
        let target = args.get(1).cloned();
        make_target = Some(target.clone());
        source_files = vec![];
        match compiler::build_with_make(target.as_deref()).await {
            Ok(bin) => executable = Some(bin),
            Err(e) => {
                eprintln!("{}\n終了します。", e);
                std::process::exit(1);
            }
        }
    } else {
        make_target = None;
        let c_files: Vec<&str> = args.iter()
            .filter(|a| a.ends_with(".c"))
            .map(|s| s.as_str())
            .collect();

        if !c_files.is_empty() {
            // C ソースファイルをコンパイルして起動する
            source_files = c_files.iter().map(|s| std::path::PathBuf::from(s)).collect();
            match compiler::compile_c_files(&c_files).await {
                Ok(bin) => executable = Some(bin),
                Err(e) => {
                    eprintln!("コンパイルエラー:\n{}\n終了します。", e);
                    std::process::exit(1);
                }
            }
        } else {
            // コンパイル済み実行ファイルを直接起動する
            source_files = vec![];
            executable = args.first().map(|a| std::path::PathBuf::from(a));
        }
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, executable, source_files, make_target).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    executable: Option<std::path::PathBuf>,
    source_files: Vec<std::path::PathBuf>,
    make_target: Option<Option<String>>,
) -> Result<()> {
    let mut app = App::new(executable, source_files, make_target).await?;

    loop {
        // GDB からのイベントを処理してから描画する
        app.poll_gdb_events();

        terminal.draw(|f| ui::render(f, &app))?;

        // 再起動フラグを検知して非同期で処理する
        if app.restart_requested {
            app.restart_requested = false;
            app.restart().await;
        }

        if event::poll(std::time::Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    _ => app.handle_key(key),
                }
            }
        }
    }

    Ok(())
}
