use std::sync::OnceLock;
use std::env;
use regex::Regex;

/// 全局浅色模式检测结果缓存。
///
/// 通过 [`detect_light_mode`] 首次检测后写入，后续调用直接读取缓存值。
/// `OnceLock` 保证仅写入一次且线程安全。
static LIGHT_MODE_CACHE: OnceLock<bool> = OnceLock::new();
/// 布尔真值匹配模式。
const TRUE_RE: &str = r"^(1|true|on|yes|y)$";

/// 布尔假值匹配模式。
const FALSE_RE: &str = r"^(0|false|off|no|n)$";

/// 检测当前终端是否处于浅色模式，结果会被缓存。
///
/// 首次调用时执行实际检测并将结果写入 [`LIGHT_MODE_CACHE`]，
/// 后续调用直接返回缓存值。
pub fn detect_light_mode() -> bool {
    *LIGHT_MODE_CACHE.get_or_init(|| {
        // TODO: 实现终端浅色模式检测逻辑
        // 例如：检查 COLORFGBG 环境变量、终端背景色查询等

        let true_re = Regex::new(TRUE_RE).unwrap();
        let false_re = Regex::new(FALSE_RE).unwrap();

        // 检查环境变量
        for var in ["SAGENT_LIGHT", "SAGENT_TUI_LIGHT"] {
            let v = env::var(var)
                .unwrap_or_default()
                .trim()
                .to_lowercase();

            if true_re.is_match(&v) {
                return true;
            }
            if false_re.is_match(&v) {
                return false;
            }
        }

        // 主题
        if env::var("SAGENT_THEME").unwrap_or_default().trim().to_lowercase() == "light" {
            return true;
        }
        false
    })
}