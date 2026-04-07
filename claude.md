# tui-debugger

RustとRatatuiで構築するCLIデバッガーTUIフロントエンド。
現在はC言語を対象とし、バックエンドにGDB（Machine Interface）を使用する。

## アーキテクチャ
```
┌─────────────────────────────────────┐
│  TUI Layer (Ratatui)                │
│  - SourceView / VarView / StatusBar │
├─────────────────────────────────────┤
│  Debugger Abstraction Layer         │
│  - trait Debugger                   │
│  - GdbBackend (現在の実装)           │
├─────────────────────────────────────┤
│  GDB/MI Protocol Layer              │
│  - stdin/stdout pipe で GDB と通信  │
└─────────────────────────────────────┘
```

## ディレクトリ構成
```
src/
  main.rs           - エントリポイント、TUIループ
  app.rs            - アプリケーション状態管理
  ui/
    mod.rs
    source_view.rs  - ソースコード表示ウィジェット
    var_view.rs     - 変数一覧ウィジェット
    status_bar.rs   - 下部ステータス・キーバインド表示
  debugger/
    mod.rs          - trait Debugger 定義
    gdb.rs          - GDB/MI バックエンド実装
    types.rs        - BreakPoint, Variable, Frame 等の型定義
```

## 開発コマンド
```bash
cargo build
cargo run -- <実行ファイル>   # 例: cargo run -- ./a.out
cargo clippy
cargo test
```

## 依存クレート

- ratatui = "0.29"
- crossterm = "0.28"
- tokio = { version = "1", features = ["full"] }
- anyhow = "1"
- serde = { version = "1", features = ["derive"] }
- serde_json = "1"
- tracing / tracing-subscriber (ログはファイルに書き出す)

## GDB/MI について

GDBは `-i=mi` フラグで起動し、stdin/stdoutをパイプで繋ぐ。
コマンド例:
- `-exec-next` → step over (1行)
- `-exec-step` → step into (関数に入る)
- `-exec-finish` → step out
- `-exec-continue` → continue
- `-stack-list-variables --all-values` → 変数一覧
- `-break-insert <file>:<line>` → ブレークポイント設定

## 実装の注意

- GDBとの通信は別スレッド（tokio task）で非同期に行う
- UIスレッドとGDBスレッドはmpscチャネルで通信
- ソースファイルの読み込みはGDBから `fullname` を取得してキャッシュ
- 日本語パス対応を忘れずに（PathBufを使う）

## キーバインド

| キー | 動作 |
|------|------|
| n / F10 | Next（step over）|
| s / F11 | Step（step into）|
| f / F12 | Finish（step out）|
| c | Continue |
| b | カーソル行にブレークポイントトグル |
| g | 指定行にジャンプ実行（入力プロンプト）|
| l | ループ変数条件ジャンプ（例: i==10）|
| q | 終了 |
| Tab | パネルフォーカス切り替え |
| ↑↓ | スクロール |

## フェーズ管理

- Phase 1: GDB/MI接続、next/step/finish/continue、ソース表示、変数表示
- Phase 2: ブレークポイント、条件ジャンプ、ウォッチ式
- Phase 3: マルチスレッド対応、Python/Go等への拡張

## 会話のガイドライン

- 常に日本語で会話する
- 技術的な説明も日本語で行う
- コード内のコメントは日本語で記述
- エラーメッセージの解説は日本語で
- README.mdなどのドキュメントも日本語で作成