//! `config.read` 的非秘密协议 DTO。

use serde::{Deserialize, Serialize};

/// `config.read` 不接受参数，防止单个请求切换 Profile 或读取任意路径。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigReadParams {}

/// 当前固定 Profile 的公开配置摘要。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigReadResult {
    /// daemon 启动时绑定的 Profile 名称；不含磁盘路径，客户端不能通过它切换 Profile。
    pub profile: String,
    /// 已配置 Provider；未设置时为 `null`。
    pub provider: Option<String>,
    /// 可展示的模型名；不包含 endpoint 或 credential。
    pub model: Option<String>,
    /// 用户配置的自定义 Provider 名称。
    pub provider_names: Vec<String>,
    /// 当前版本未识别但未被删除的顶层 YAML 字段。
    pub unknown_fields: Vec<String>,
}
