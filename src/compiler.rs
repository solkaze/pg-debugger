use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// C ソースファイルを `gcc -g` でコンパイルし、出力バイナリのパスを返す。
/// コンパイルに失敗した場合は stderr の内容を含むエラーを返す。
pub async fn compile_c(source: &Path) -> Result<PathBuf> {
    let s = source.to_str().unwrap_or("output");
    compile_c_files(&[s]).await
}

/// 複数の C ソースファイルを `gcc -g` でコンパイルし、出力バイナリのパスを返す。
/// 出力ファイル名は最初のファイルの stem を使う。
/// コンパイルに失敗した場合は stderr の内容を含むエラーを返す。
pub async fn compile_c_files(sources: &[&str]) -> Result<PathBuf> {
    let stem = Path::new(sources[0])
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let pid = std::process::id();
    let out_path = PathBuf::from(format!("/tmp/pg-debugger-{}-{}", stem, pid));

    // main() より前に setvbuf を呼ぶ補助ファイルを生成する
    let init_src = PathBuf::from(format!("/tmp/pg-debugger-init-{}.c", pid));
    tokio::fs::write(
        &init_src,
        b"#include <stdio.h>\n\
          __attribute__((constructor))\n\
          static void pg_debugger_init(void) {\n\
              setvbuf(stdout, NULL, _IONBF, 0);\n\
              setvbuf(stderr, NULL, _IONBF, 0);\n\
          }\n",
    )
    .await?;

    let output = Command::new("gcc")
        .arg("-g")
        .arg("-D_FORTIFY_SOURCE=0")
        .arg("-o")
        .arg(&out_path)
        .args(sources)
        .arg(&init_src)
        .output()
        .await?;

    // 補助ファイルを削除（失敗しても無視）
    let _ = tokio::fs::remove_file(&init_src).await;

    if output.status.success() {
        Ok(out_path)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        bail!(stderr)
    }
}

/// Makefile を使ってビルドし、生成された実行ファイルのパスを返す。
/// target が None の場合は Makefile のデフォルトターゲットを使う。
pub async fn build_with_make(target: Option<&str>) -> Result<PathBuf> {
    if !Path::new("Makefile").exists() && !Path::new("makefile").exists() {
        bail!("Makefileが見つかりません");
    }

    let mut cmd = Command::new("make");
    if let Some(t) = target {
        cmd.arg(t);
    }
    let output = cmd.output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        bail!("makeエラー:\n{}", stderr);
    }

    let exe = if let Some(t) = target {
        PathBuf::from(format!("./{}", t))
    } else {
        find_makefile_default_target()?
    };

    if !exe.exists() {
        bail!("makeで生成された実行ファイルが見つかりません: {}", exe.display());
    }

    Ok(exe)
}

/// Makefile を読んで最初の non-.PHONY ターゲット名を返す。
fn find_makefile_default_target() -> Result<PathBuf> {
    let content = std::fs::read_to_string("Makefile")
        .or_else(|_| std::fs::read_to_string("makefile"))?;

    for line in content.lines() {
        if line.starts_with('#') || line.trim().is_empty() || line.starts_with('.') {
            continue;
        }
        if let Some(target) = line.split(':').next() {
            let target = target.trim();
            if !target.is_empty() && !target.contains(' ') {
                return Ok(PathBuf::from(format!("./{}", target)));
            }
        }
    }
    bail!("Makefileのデフォルトターゲットが見つかりません")
}
