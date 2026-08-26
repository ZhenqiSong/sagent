//! stdio JSON-RPC server 主循环模块。
//!
//! 负责读取 stdin 请求、分发到 dispatcher 处理、将响应写入 stdout，
//! 并处理超长输入、EOF、BrokenPipe 等边界情况。
//!
//! @author   songzq
//! @created  2026-08-26

use sagent_config::Config;
use sagent_runtime::Runtime;
use sagent_types::version::Capabilities;
use tracing::{error, info, warn};

/// 运行 stdio JSON-RPC server 的主循环。
///
/// 处理流程：
/// 1. 从 stdin 逐行读取 JSON-RPC request/notification
/// 2. 分发到 dispatcher 处理
/// 3. 将 response 写入 stdout（单行 JSON，立即 flush）
/// 4. 错误写 stderr 日志
/// 5. stdin EOF 时正常退出（返回码 0）
///
/// stdout 写失败或 BrokenPipe 时有序退出，不 panic。
pub fn run_stdio_server() {
    let runtime = match Runtime::open(Config::default()) {
        Ok(runtime) => runtime,
        Err(error) => {
            error!(error = %error, "Runtime 初始化失败，拒绝接受 RPC");
            return;
        },
    };
    let caps = Capabilities::runtime_capabilities();
    let pv = caps.protocol_version();
    let async_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("创建 Tokio runtime 失败");
    let mut subscriptions = crate::dispatcher::Subscriptions::new();

    // 启动日志：记录协议版本、runtime 版本和 capabilities
    info!(
        protocol = %pv.protocol,
        version = pv.version,
        runtime_version = %pv.runtime_version,
        features = ?pv.features,
        "sagent stdio JSON-RPC server 启动"
    );

    let mut reader = crate::stdio::LineReader::new();
    let mut writer = crate::stdio::LineWriter::new();

    loop {
        // 读取下一行
        let line = match reader.read_line() {
            Some(Ok(line)) => line,
            Some(Err(e)) if crate::stdio::is_line_too_large(&e) => {
                error!(error = %e, error_code = -32003, "输入行超过协议限制，继续处理");
                let response = crate::dispatcher::build_error_response(
                    None,
                    &sagent_api::error::ErrorObject::payload_too_large(
                        "request line exceeds 1048576 bytes",
                    ),
                );
                if let Err(write_error) = writer.write_value(&response) {
                    warn!(error = %write_error, "超长输入错误响应写入失败，退出");
                    break;
                }
                continue;
            },
            Some(Err(e)) => {
                error!(
                    error = %e,
                    error_kind = ?e.kind(),
                    "stdin 读取错误，退出"
                );
                break;
            },
            None => {
                info!("stdin EOF，正常退出");
                break;
            },
        };

        // 先发出已排队的 live event，再处理下一条请求。
        for event in crate::dispatcher::drain_events(&mut subscriptions) {
            if let Err(error) = writer.write_value(&event) {
                warn!(error = %error, "事件写入失败，退出");
                break;
            }
        }

        // 分发处理
        let result = crate::dispatcher::dispatch_runtime(
            &line,
            &caps,
            &runtime,
            &async_runtime,
            &mut subscriptions,
        );
        match result {
            Ok(Some(response)) => {
                // 成功响应 → 写 stdout
                if let Err(e) = writer.write_value(&response) {
                    error!(
                        error = %e,
                        error_kind = ?e.kind(),
                        "stdout 写入失败（BrokenPipe 或 peer 断开），退出"
                    );
                    break;
                }
            },
            Ok(None) => {
                // notification → 不写 response（日志已在 dispatcher 中记录）
            },
            Err((id, err_obj)) => {
                // 错误响应 → 写 stdout（日志已在 dispatcher 中记录）
                let error_response = crate::dispatcher::build_error_response(id, &err_obj);
                if let Err(e) = writer.write_value(&error_response) {
                    // stdout 写失败（如 BrokenPipe），写 stderr 后退出
                    warn!(
                        error = %e,
                        error_kind = ?e.kind(),
                        error_code = err_obj.code,
                        "stdout 写入失败，无法返回错误响应，退出"
                    );
                    break;
                }
            },
        }
    }

    if let Err(error) = async_runtime.block_on(runtime.shutdown()) {
        warn!(error = %error, "Runtime shutdown 失败");
    }
    info!("sagent stdio JSON-RPC server 已停止");
}
