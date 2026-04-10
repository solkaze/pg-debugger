/// GDB の繰り返し表記 `", '\NNN' <repeats N times>"` を除去する。
/// 例: `"\343\201\202", '\000' <repeats 13 times>` → `"\343\201\202"`
fn strip_gdb_repeat_notation(s: &str) -> String {
    if let Some(pos) = s.find(", '\\") {
        s[..pos].to_string()
    } else {
        s.to_string()
    }
}

/// GDB の 8進数エスケープ文字列 `"\343\201\202..."` をUTF-8文字列にデコードする。
/// 外側のクォートを除去してデコードし、クォートなしの文字列を返す。
/// 呼び出し側で必要に応じて `"..."` を付けること。
pub fn decode_gdb_octal_string(value: &str) -> String {
    let value = strip_gdb_repeat_notation(value.trim());
    let s = value.trim();
    // GDB MI エンコード由来の `\"..."\"` と bare `"..."` の両方に対応する
    let s = if s.starts_with("\\\"") { &s[2..] } else { s.strip_prefix('"').unwrap_or(s) };
    let s = if s.ends_with("\\\"") { &s[..s.len()-2] } else { s.strip_suffix('"').unwrap_or(s) };

    let mut bytes: Vec<u8> = Vec::new();
    let mut chars = s.chars().peekable();

    loop {
        match chars.next() {
            None => break,
            Some('\\') => match chars.next() {
                Some(c @ '0'..='7') => {
                    // 8進数エスケープ \NNN（最大3桁）
                    let mut n = c as u32 - '0' as u32;
                    for _ in 0..2 {
                        match chars.peek() {
                            Some(&d) if ('0'..='7').contains(&d) => {
                                chars.next();
                                n = n * 8 + (d as u32 - '0' as u32);
                            }
                            _ => break,
                        }
                    }
                    bytes.push(n as u8);
                }
                Some('n') => bytes.push(b'\n'),
                Some('t') => bytes.push(b'\t'),
                Some('\\') => bytes.push(b'\\'),
                Some('"') => bytes.push(b'"'),
                Some(c) => bytes.extend_from_slice(c.to_string().as_bytes()),
                None => break,
            },
            Some('\0') => break,
            Some(c) => bytes.extend_from_slice(c.to_string().as_bytes()),
        }
    }

    match String::from_utf8(bytes.clone()) {
        Ok(s) => s.split('\0').next().unwrap_or("").to_string(),
        Err(_) => {
            // UTF-8でない場合はASCII範囲のみ表示
            bytes
                .iter()
                .take_while(|&&b| b != 0)
                .map(|&b| if (32..=126).contains(&b) { b as char } else { '?' })
                .collect()
        }
    }
}
