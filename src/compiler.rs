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

    let output = Command::new("gcc")
        .arg("-g")
        .arg("-o")
        .arg(&out_path)
        .arg(source)
        .output()
        .await?;

    if output.status.success() {
        Ok(out_path)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        bail!(stderr)
    }
}
