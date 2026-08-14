//! stdio transport 模块。
//!
//! 提供 newline-delimited JSON 的 stdin 行读取和 stdout 行写入。
//! stdout 每行立即 flush，stderr 仅用于日志。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 7 stdio transport
//! @change   2026-08-14 增强：暴露超长输入判断供协议错误处理

use std::io::{self, BufRead, BufReader, Write};

/// 单行最大字节数（1 MiB）。
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// method 最大字节数。
pub const MAX_METHOD_BYTES: usize = 256;

/// request id 最大字节数（序列化后）。
pub const MAX_ID_BYTES: usize = 256;

const LINE_TOO_LARGE_PREFIX: &str = "line exceeds ";

/// stdio 行读取器。
///
/// 从 stdin 逐行读取，跳过空行，限制单行最大长度。
pub struct LineReader {
    reader: BufReader<io::Stdin>,
}

/// 判断读取错误是否表示单行超过协议限制。
pub fn is_line_too_large(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::InvalidData
        && error.to_string().starts_with(LINE_TOO_LARGE_PREFIX)
}

impl LineReader {
    /// 创建新的 stdin 行读取器。
    pub fn new() -> Self {
        Self {
            reader: BufReader::new(io::stdin()),
        }
    }

    /// 读取下一行非空内容。
    ///
    /// 忽略空行（仅包含空白字符的行）并继续等待。
    /// 返回 `None` 表示 stdin EOF。
    /// 返回 `Some(Err)` 表示读取错误或行超过限制。
    pub fn read_line(&mut self) -> Option<io::Result<String>> {
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None, // EOF
                Ok(_) => {
                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() {
                        continue; // 忽略空行
                    }
                    if trimmed.len() > MAX_LINE_BYTES {
                        return Some(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "line exceeds {} bytes limit (got {} bytes)",
                                MAX_LINE_BYTES,
                                trimmed.len()
                            ),
                        )));
                    }
                    return Some(Ok(trimmed));
                },
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

impl Default for LineReader {
    fn default() -> Self {
        Self::new()
    }
}

/// stdio 行写入器。
///
/// 每行输出后立即 flush，确保交互式客户端即时收到响应。
pub struct LineWriter {
    writer: io::Stdout,
}

impl LineWriter {
    /// 创建新的 stdout 行写入器。
    pub fn new() -> Self {
        Self {
            writer: io::stdout(),
        }
    }

    /// 写入一行 JSON 并立即 flush。
    ///
    /// 返回 `Ok(())` 表示成功；返回 `Err` 表示写入失败（如 BrokenPipe）。
    pub fn write_line(&mut self, json: &str) -> io::Result<()> {
        writeln!(self.writer, "{}", json)?;
        self.writer.flush()
    }

    /// 写入 serde_json::Value 为一行 JSON 并立即 flush。
    pub fn write_value(&mut self, value: &serde_json::Value) -> io::Result<()> {
        let json = serde_json::to_string(value).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("序列化失败: {}", e))
        })?;
        self.write_line(&json)
    }
}

impl Default for LineWriter {
    fn default() -> Self {
        Self::new()
    }
}
