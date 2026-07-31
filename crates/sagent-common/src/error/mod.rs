pub mod failover;
pub mod classified;
pub mod sagent;

// 重导出到 error 模块层级，保持外部 `use sagent_common::error::FailoverReason` 等路径不变
pub use failover::FailoverReason;
pub use classified::ClassifiedError;
pub use sagent::SagentError;
