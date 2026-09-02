//! 客户端 surface 与 capability 的稳定类型。

use serde::{Deserialize, Serialize};

use crate::ClientId;

/// 连接 Sagent 的客户端表面类型。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientSurface {
    /// 交互式命令行。
    Cli,
    /// Ratatui 终端界面。
    Tui,
    /// 桌面应用。
    Desktop,
    /// Web 客户端。
    Web,
    /// Telegram/Discord 等消息通道。
    Channel,
    /// 程序化 API 客户端。
    Api,
}

/// 客户端向 Runtime 声明的能力快照。
///
/// capability 属于连接的 session，而不是 daemon 进程环境；这避免远程 TUI 或桌面端
/// 因服务进程的环境变量而错误获得 UI 专属能力。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// 客户端连接标识。
    pub client_id: ClientId,
    /// 客户端 surface。
    pub surface: ClientSurface,
    /// 是否能够展示并回答 approval 请求。
    pub interactive_approval: bool,
    /// 是否支持对同一消息进行流式编辑。
    pub supports_stream_edits: bool,
    /// 客户端支持的协议版本。
    pub protocol_version: u32,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ClientCapabilities, ClientSurface};
    use crate::ClientId;

    #[test]
    fn tui_capabilities_have_stable_json_fields() {
        let capabilities = ClientCapabilities {
            client_id: ClientId::new(),
            surface: ClientSurface::Tui,
            interactive_approval: true,
            supports_stream_edits: false,
            protocol_version: 1,
        };
        let value = serde_json::to_value(capabilities).expect("capability 应能序列化");

        assert_eq!(value["surface"], json!("tui"));
        assert_eq!(value["interactive_approval"], json!(true));
        assert_eq!(value["supports_stream_edits"], json!(false));
        assert_eq!(value["protocol_version"], json!(1));
    }
}
