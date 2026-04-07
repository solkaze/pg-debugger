# pg-debugger

C言語向けのターミナルUIデバッガーです。GDBをバックエンドに、ソースコードの表示・ステップ実行・変数確認をターミナル上でインタラクティブに行えます。

## 必要な環境

- **gdb** — デバッガーバックエンド
- **gcc** — Cソースファイルを渡した場合のコンパイルに使用

```sh
# Ubuntu / Debian
sudo apt-get install gdb gcc

# macOS (Homebrew)
brew install gdb gcc
```

## インストール

[Releases](../../releases) から環境に合ったバイナリをダウンロードして、PATHの通ったディレクトリに配置してください。

```sh
# Linux
curl -L -o pg-debugger https://github.com/solkaze/pg-debugger/releases/latest/download/pg-debugger-linux-x86_64
chmod +x pg-debugger
sudo mv pg-debugger /usr/local/bin/

# macOS
curl -L -o pg-debugger https://github.com/solkaze/pg-debugger/releases/latest/download/pg-debugger-macos-x86_64
chmod +x pg-debugger
sudo mv pg-debugger /usr/local/bin/
```

## 使い方

```sh
# Cソースファイルを指定（自動でコンパイルしてデバッグ開始）
./pg-debugger example.c

# コンパイル済み実行ファイルを指定
./pg-debugger ./a.out
```

## キーバインド

### デバッグ操作

| キー       | 動作                                 |
| ---------- | ------------------------------------ |
| `n` / F10  | ステップオーバー（次の行へ）         |
| `s`        | ステップイン（関数内に入る）         |
| `f`        | ステップアウト（現在の関数を抜ける） |
| `c` / F5   | 実行継続（次のブレークポイントまで） |
| `r`        | 再起動（ソースがある場合は再コンパイル） |

### ブレークポイント

| キー | 動作                                       |
| ---- | ------------------------------------------ |
| `b`  | カーソル行のブレークポイントをトグル       |
| `B`  | 行番号を入力してブレークポイントをトグル   |

### 移動

| キー           | 動作                         |
| -------------- | ---------------------------- |
| `g`            | 行番号を入力してジャンプ     |
| `↑` / `↓`     | カーソル移動 / スクロール    |
| `PageUp` / `PageDown` | コンソールをページ単位でスクロール |

### パネル・入力

| キー      | 動作                                                   |
| --------- | ------------------------------------------------------ |
| `Tab`     | フォーカスを Source → 変数 → コンソール の順に切替 |
| `i`       | プログラムの標準入力に文字列を送信                     |
| `q` / `Ctrl+C` | 終了                                             |

## ライセンス

MIT
