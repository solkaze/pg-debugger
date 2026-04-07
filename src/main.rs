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
mod ui;

use app::App;

#[tokio::main]
async fn main() -> Result<()> {
    // ログはファイルに書き出す（TUI と stdout が競合しないよう）
    let log_file = std::fs::File::create("debug.log")?;
    tracing_subscriber::fmt().with_writer(log_file).init();

    // コマンドライン引数から実行ファイルパスを取得
    let args: Vec<String> = std::env::args().collect();

    // .c ファイルが渡された場合はコンパイルしてから起動する
    let source_file: Option<std::path::PathBuf> = args
        .get(1)
        .filter(|a| a.ends_with(".c"))
        .map(|a| std::path::PathBuf::from(a));

    let executable: Option<std::path::PathBuf> = if let Some(ref src) = source_file {
        match compiler::compile_c(src).await {
            Ok(bin) => Some(bin),
            Err(e) => {
                eprintln!("コンパイルエラー:\n{}\n終了します。", e);
                std::process::exit(1);
            }
        }
    } else {
        args.get(1).map(|a| std::path::PathBuf::from(a))
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, executable, source_file).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    executable: Option<std::path::PathBuf>,
    source_file: Option<std::path::PathBuf>,
) -> Result<()> {
    let mut app = App::new(executable, source_file).await?;

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
