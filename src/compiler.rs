use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// C ソースファイルを `gcc -g` でコンパイルし、出力バイナリのパスを返す。
/// コンパイルに失敗した場合は stderr の内容を含むエラーを返す。
pub async fn compile_c(source: &Path) -> Result<PathBuf> {
    let stem = source
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
        .arg(source)
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
