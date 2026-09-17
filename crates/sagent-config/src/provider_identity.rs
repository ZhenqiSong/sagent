//! Provider 配置中需要稳定不变量的领域值对象。
//!
//! 这些类型把 parser 之后仍会跨模块流动的标识从普通字符串中区分出来。它们只负责
//! 文本归一化、格式校验和脱敏安全的 revision 表示，不读取文件、环境变量或网络。

use std::{borrow::Borrow, fmt};

use anyhow::{Result, bail};
use sha2::{Digest, Sha256};

/// 已归一化的 Provider kind 或自定义 Provider 名称。
#[derive(Debug, Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProviderKind(String);

impl ProviderKind {
    /// 归一化并校验 Provider kind；kind 比较不区分 ASCII 大小写。
    pub fn try_new(value: impl AsRef<str>) -> Result<Self> {
        let value = value.as_ref().trim().to_ascii_lowercase();
        validate_token(&value, "Provider kind")?;
        Ok(Self(value))
    }

    /// 返回用于路由、日志和表查找的 canonical kind。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for ProviderKind {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 已校验的模型标识。
#[derive(Debug, Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModelId(String);

impl ModelId {
    /// 归一化并校验模型标识；保留模型服务通常区分大小写的正文。
    pub fn try_new(value: impl AsRef<str>) -> Result<Self> {
        let value = value.as_ref().trim().to_owned();
        validate_token(&value, "model id")?;
        Ok(Self(value))
    }

    /// 返回用于 Provider 请求和公开摘要的模型标识。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for ModelId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 只表示 secret 的取得方式，不包含 secret 值本身。
#[derive(Debug, Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CredentialReference(String);

impl CredentialReference {
    /// 归一化并校验凭据引用；引用不得包含空白，避免 `.env` 查找歧义。
    pub fn try_new(value: impl AsRef<str>) -> Result<Self> {
        let value = value.as_ref().trim().to_owned();
        validate_token(&value, "credential reference")?;
        Ok(Self(value))
    }

    /// 返回环境变量或其他凭据存储的引用名，不返回 secret。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for CredentialReference {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for CredentialReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Profile descriptor 的稳定 revision；格式为 `sha256:` 加 64 位十六进制摘要。
#[derive(Debug, Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DescriptorRevision(String);

impl DescriptorRevision {
    /// 从外部 revision 文本构造值对象并校验格式。
    pub fn try_new(value: impl AsRef<str>) -> Result<Self> {
        let value = value.as_ref();
        let digest = value.strip_prefix("sha256:").unwrap_or_default();
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("descriptor revision 必须是 sha256: 加 64 位十六进制摘要");
        }
        Ok(Self(value.to_owned()))
    }

    /// 返回不含配置正文的稳定 revision 文本。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 对 canonical descriptor 字节计算 revision；调用方必须先完成字段排序和脱敏。
    pub(crate) fn from_canonical_bytes(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        Self(format!("sha256:{digest:x}"))
    }
}

impl fmt::Display for DescriptorRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn validate_token(value: &str, field: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{field} 不能为空");
    }
    if value.chars().any(char::is_whitespace) {
        bail!("{field} 不能包含空白");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CredentialReference, DescriptorRevision, ModelId, ProviderKind};

    #[test]
    fn provider_kind_is_case_insensitive_but_model_id_is_not_changed() {
        assert_eq!(
            ProviderKind::try_new(" OpenAI-Compatible ")
                .unwrap()
                .as_str(),
            "openai-compatible"
        );
        assert_eq!(ModelId::try_new(" model-v1 ").unwrap().as_str(), "model-v1");
    }

    #[test]
    fn credential_reference_rejects_whitespace_and_revision_requires_sha256() {
        assert!(CredentialReference::try_new("KEY NAME").is_err());
        assert!(DescriptorRevision::try_new("revision-1").is_err());
        let revision = DescriptorRevision::try_new(format!("sha256:{}", "a".repeat(64)))
            .expect("sha256 revision 应合法");
        assert!(revision.as_str().starts_with("sha256:"));
    }
}
