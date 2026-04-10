# pg-debugger

C言語向けのターミナルUIデバッガーです。GDBをバックエンドに、ソースコードの表示・ステップ実行・変数確認をターミナル上でインタラクティブに行えます。

## 必要な環境

- **gdb** — デバッガーバックエンド
- **gcc** — Cソースファイルを渡した場合のコンパイルに使用

```sh
# Ubuntu / Debian
sudo apt-get install gdb gcc
```

```sh
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
```

```sh
# macOS
curl -L -o pg-debugger https://github.com/solkaze/pg-debugger/releases/latest/download/pg-debugger-macos-x86_64
chmod +x pg-debugger
sudo mv pg-debugger /usr/local/bin/
```

### gdbがインストールできないとき

```sh
# ホームディレクトリにローカルインストール
mkdir -p ~/.local
cd /tmp
wget https://ftp.gnu.org/gnu/gdb/gdb-14.2.tar.gz
tar xzf gdb-14.2.tar.gz
cd gdb-14.2
./configure --prefix=$HOME/.local
make -j$(nproc)
make install

# PATHに追加
echo 'export PATH=$HOME/.local/bin:$PATH' >> ~/.bashrc
source ~/.bashrc
```

この手順でgdbをインストールした場合、pg-debuggerは自動的に`$HOME/.local/bin/gdb`を使用します。

## 使い方

```sh
# Cソースファイルを指定（自動でコンパイルしてデバッグ開始）
./pg-debugger example.c

# 複数のCソースファイルを指定
./pg-debugger main.c sub.c utils.c

# Makefileを使ってビルド（デフォルトターゲット）
./pg-debugger --make

# Makefileで特定のターゲットをビルド
./pg-debugger --make myprogram

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
| `c` / F5   | 実行継続（次のブレイクポイントまで） |
| `r`        | 再起動（ソースがある場合は再コンパイル） |

### ブレイクポイント

| キー | 動作                                       |
| ---- | ------------------------------------------ |
| `b`  | カーソル行のブレイクポイントを設置/削除       |
| `B`  | 行番号を入力してブレイクポイントを設置/削除   |

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
