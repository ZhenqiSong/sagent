//! Profile 凭据引用的读取实现。
//!
//! 本模块只负责按既定优先级读取 `.env` 和进程环境中的值；它不解析 Provider 选择、
//! 不创建 HTTP client，也不把读取到的秘密放进公开配置或 Debug 输出。

use std::{env, fs, path::Path};

use anyhow::{Context, Result, bail};

use crate::SagentPaths;

/// 读取凭据环境变量，让 Profile `.env` 文件值优先于同名进程环境变量。
pub(crate) fn read_env_value(paths: &SagentPaths, name: &str) -> Result<Option<String>> {
    if name.trim().is_empty() {
        bail!("API key 环境变量名不能为空");
    }
    if let Some(value) = read_dotenv_value(&paths.env_file, name)?
        && !value.trim().is_empty()
    {
        return Ok(Some(value));
    }
    Ok(env::var(name).ok().filter(|value| !value.trim().is_empty()))
}

/// 从 dotenv 文本中读取一个键，跳过注释、空行和无法识别的行。
fn read_dotenv_value(path: &Path, name: &str) -> Result<Option<String>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("读取凭据文件失败：{}", path.display()));
        }
    };
    for raw_line in content.lines() {
        let line = raw_line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != name {
            continue;
        }
        let value = raw_value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value)
            .to_owned();
        return Ok(Some(value));
    }
    Ok(None)
}
