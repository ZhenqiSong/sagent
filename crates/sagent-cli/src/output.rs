//! CLI 输出格式与序列化。
//!
//! 作者：SongZQ

use std::io::{self, Write};

use anyhow::{Context, Result};
use clap::ValueEnum;
use serde::Serialize;

/// CLI 输出格式。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    /// 人类可读的终端文本。
    #[default]
    Text,
    /// 一行 JSON，适用于脚本和结构化调用方。
    Json,
}

/// 将值序列化为一行 JSON，供 stdout 输出和单元测试共用。
pub fn json_output<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).context("序列化 JSON 输出失败")
}

/// 以指定格式写入一次命令结果。
pub fn print_output<T: Serialize>(
    format: OutputFormat,
    value: &T,
    text_lines: impl IntoIterator<Item = String>,
) -> Result<()> {
    match format {
        OutputFormat::Text => {
            for line in text_lines {
                println!("{line}");
            }
        }
        OutputFormat::Json => {
            let stdout = io::stdout();
            let mut stdout = stdout.lock();
            write!(stdout, "{}", json_output(value)?)?;
            writeln!(stdout).context("写入 JSON 输出换行失败")?;
        }
    }
    Ok(())
}
