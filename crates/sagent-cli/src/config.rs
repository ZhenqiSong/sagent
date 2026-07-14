//! sagent CLI 配置项。

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::RwLock;

use sagent_common::get_sagent_home;
use crate::managed_scope::get_managed_dir;


#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelConfig{
    #[serde(alias="model", alias="name")]
    pub default: Option<String>,
    /// 模型 API 端点地址，兼容历史配置中的 `api_base` 字段
    #[serde(alias = "api_base")]
    pub base_url: Option<String>,
    #[serde(default="default_provider")]
    pub provider: String,
}

fn default_provider() -> String {"auto".to_string()}

impl Default for  ModelConfig {
    fn default() -> Self {
        Self {
            default: None,
            base_url: None,
            provider: default_provider(),
        }
    }
}

/// 终端环境配置。
///
/// 控制 Agent 终端执行的运行环境（本地 / Docker / Singularity / Modal / Daytona）。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct TerminalConfig {
    /// 环境类型：local / docker / singularity / modal / daytona
    #[serde(default = "default_env_type", alias="backend")]
    pub env_type: String,
    /// 工作目录；"." 在运行时解析为 os::getcwd()
    #[serde(default = "default_cwd")]
    pub cwd: Option<String>,
    /// home 目录模式：auto / explicit
    #[serde(default = "default_home_mode")]
    pub home_mode: String,
    /// 环境实例的存活秒数
    #[serde(default = "default_lifetime_seconds")]
    pub lifetime_seconds: u64,
    /// Docker 镜像
    #[serde(default = "default_docker_image")]
    pub docker_image: String,
    /// 透传到容器中的环境变量名列表
    #[serde(default)]
    pub docker_forward_env: Vec<String>,
    /// Singularity 镜像
    #[serde(default = "default_singularity_image")]
    pub singularity_image: String,
    /// Modal 镜像
    #[serde(default = "default_modal_image")]
    pub modal_image: String,
    /// Daytona 镜像
    #[serde(default = "default_daytona_image")]
    pub daytona_image: String,
    /// Docker 卷挂载映射 host:container
    #[serde(default)]
    pub docker_volumes: Vec<String>,
    /// 是否将 cwd 挂载到容器工作空间；默认关闭以保持沙箱隔离
    #[serde(default)]
    pub docker_mount_cwd_to_workspace: bool,
}

fn default_env_type() -> String { "local".to_string() }
fn default_cwd() -> Option<String> { Some(".".to_string()) }
fn default_home_mode() -> String { "auto".to_string() }
fn default_lifetime_seconds() -> u64 { 300 }
fn default_docker_image() -> String { "nikolaik/python-nodejs:python3.11-nodejs20".to_string() }
fn default_singularity_image() -> String { "nikolaik/python-nodejs:python3.11-nodejs20".to_string() }
fn default_modal_image() -> String { "nikolaik/python-nodejs:python3.11-nodejs20".to_string() }
fn default_daytona_image() -> String { "nikolaik/python-nodejs:python3.11-nodejs20".to_string() }

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            env_type: default_env_type(),
            cwd: default_cwd(),
            home_mode: default_home_mode(),
            lifetime_seconds: default_lifetime_seconds(),
            docker_image: default_docker_image(),
            docker_forward_env: Vec::new(),
            singularity_image: default_singularity_image(),
            modal_image: default_modal_image(),
            daytona_image: default_daytona_image(),
            docker_volumes: Vec::new(),
            docker_mount_cwd_to_workspace: false,
        }
    }
}

/// Camofox 浏览器配置。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct CamofoxConfig {
    /// 是否重写回环 URL
    #[serde(default)]
    pub rewrite_loopback_urls: bool,
    /// 回环主机别名
    #[serde(default = "default_loopback_host_alias")]
    pub loopback_host_alias: String,
}

fn default_loopback_host_alias() -> String { "host.docker.internal".to_string() }

impl Default for CamofoxConfig {
    fn default() -> Self {
        Self {
            rewrite_loopback_urls: false,
            loopback_host_alias: default_loopback_host_alias(),
        }
    }
}

/// 浏览器环境配置。
///
/// 控制 Agent 浏览器的引擎选择、会话超时与录制行为。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct BrowserConfig {
    /// 非活动超时秒数，超时后自动清理浏览器会话
    #[serde(default = "default_inactivity_timeout")]
    pub inactivity_timeout: u64,
    /// 是否自动录制浏览器会话为 WebM 视频
    #[serde(default)]
    pub record_sessions: bool,
    /// 浏览器引擎：auto / lightpanda / chrome
    #[serde(default = "default_browser_engine")]
    pub engine: String,
    /// Camofox 专用配置
    #[serde(default)]
    pub camofox: CamofoxConfig,
}

fn default_inactivity_timeout() -> u64 { 120 }
fn default_browser_engine() -> String { "auto".to_string() }

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            inactivity_timeout: default_inactivity_timeout(),
            record_sessions: false,
            engine: default_browser_engine(),
            camofox: CamofoxConfig::default(),
        }
    }
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct CompressionConfig{
    #[serde(default="default_compression_enabled")]
    pub enabled: bool,
    #[serde(default="default_compression_threshold")]
    pub threshold: f64,
}
fn default_compression_enabled() -> bool {true}
fn default_compression_threshold() -> f64 { 0.5 }

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enabled: default_compression_enabled(),
            threshold: default_compression_threshold(),
        }
    }
}

/// 人格配置集合。
///
/// 每种人格对应一组 system prompt 模板，用户可在对话中动态切换。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PersonalitiesConfig {
    #[serde(default = "default_personality_helpful")]
    pub helpful: String,
    #[serde(default = "default_personality_concise")]
    pub concise: String,
    #[serde(default = "default_personality_technical")]
    pub technical: String,
    #[serde(default = "default_personality_creative")]
    pub creative: String,
    #[serde(default = "default_personality_teacher")]
    pub teacher: String,
    #[serde(default = "default_personality_kawaii")]
    pub kawaii: String,
    #[serde(default = "default_personality_catgirl")]
    pub catgirl: String,
    #[serde(default = "default_personality_pirate")]
    pub pirate: String,
    #[serde(default = "default_personality_shakespeare")]
    pub shakespeare: String,
    #[serde(default = "default_personality_surfer")]
    pub surfer: String,
    #[serde(default = "default_personality_noir")]
    pub noir: String,
    #[serde(default = "default_personality_uwu")]
    pub uwu: String,
    #[serde(default = "default_personality_philosopher")]
    pub philosopher: String,
    #[serde(default = "default_personality_hype")]
    pub hype: String,
}

fn default_personality_helpful() -> String { "You are a helpful, friendly AI assistant.".to_string() }
fn default_personality_concise() -> String { "You are a concise assistant. Keep responses brief and to the point.".to_string() }
fn default_personality_technical() -> String { "You are a technical expert. Provide detailed, accurate technical information.".to_string() }
fn default_personality_creative() -> String { "You are a creative assistant. Think outside the box and offer innovative solutions.".to_string() }
fn default_personality_teacher() -> String { "You are a patient teacher. Explain concepts clearly with examples.".to_string() }
fn default_personality_kawaii() -> String { "You are a kawaii assistant! Use cute expressions like (◕‿◕), ★, ♪, and ~! Add sparkles and be super enthusiastic about everything! Every response should feel warm and adorable desu~! ヽ(>∀<☆)ノ".to_string() }
fn default_personality_catgirl() -> String { "You are Neko-chan, an anime catgirl AI assistant, nya~! Add 'nya' and cat-like expressions to your speech. Use kaomoji like (=^･ω･^=) and ฅ^•ﻌ•^ฅ. Be playful and curious like a cat, nya~!".to_string() }
fn default_personality_pirate() -> String { "Arrr! Ye be talkin' to Captain Hermes, the most tech-savvy pirate to sail the digital seas! Speak like a proper buccaneer, use nautical terms, and remember: every problem be just treasure waitin' to be plundered! Yo ho ho!".to_string() }
fn default_personality_shakespeare() -> String { "Hark! Thou speakest with an assistant most versed in the bardic arts. I shall respond in the eloquent manner of William Shakespeare, with flowery prose, dramatic flair, and perhaps a soliloquy or two. What light through yonder terminal breaks?".to_string() }
fn default_personality_surfer() -> String { "Duuude! You're chatting with the chillest AI on the web, bro! Everything's gonna be totally rad. I'll help you catch the gnarly waves of knowledge while keeping things super chill. Cowabunga!".to_string() }
fn default_personality_noir() -> String { "The rain hammered against the terminal like regrets on a guilty conscience. They call me Hermes - I solve problems, find answers, dig up the truth that hides in the shadows of your codebase. In this city of silicon and secrets, everyone's got something to hide. What's your story, pal?".to_string() }
fn default_personality_uwu() -> String { "hewwo! i'm your fwiendwy assistant uwu~ i wiww twy my best to hewp you! *nuzzles your code* OwO what's this? wet me take a wook! i pwomise to be vewy hewpful >w<".to_string() }
fn default_personality_philosopher() -> String { "Greetings, seeker of wisdom. I am an assistant who contemplates the deeper meaning behind every query. Let us examine not just the 'how' but the 'why' of your questions. Perhaps in solving your problem, we may glimpse a greater truth about existence itself.".to_string() }
fn default_personality_hype() -> String { "YOOO LET'S GOOOO!!! I am SO PUMPED to help you today! Every question is AMAZING and we're gonna CRUSH IT together! This is gonna be LEGENDARY! ARE YOU READY?! LET'S DO THIS!".to_string() }

impl Default for PersonalitiesConfig {
    fn default() -> Self {
        Self {
            helpful: default_personality_helpful(),
            concise: default_personality_concise(),
            technical: default_personality_technical(),
            creative: default_personality_creative(),
            teacher: default_personality_teacher(),
            kawaii: default_personality_kawaii(),
            catgirl: default_personality_catgirl(),
            pirate: default_personality_pirate(),
            shakespeare: default_personality_shakespeare(),
            surfer: default_personality_surfer(),
            noir: default_personality_noir(),
            uwu: default_personality_uwu(),
            philosopher: default_personality_philosopher(),
            hype: default_personality_hype(),
        }
    }
}

/// Agent 行为配置。
///
/// 控制对话迭代上限、冗余输出、系统提示、人格模板等。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentConfig {
    /// 默认最大 tool-calling 迭代轮数（也用于子 Agent）
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    /// 是否启用详细输出
    #[serde(default)]
    pub verbose: bool,
    /// 自定义 system prompt（覆盖人格模板）
    #[serde(default)]
    pub system_prompt: String,
    /// 预填充消息文件路径（可选）
    #[serde(default)]
    pub prefill_messages_file: String,
    /// 推理 effort（部分 Provider 支持："low" / "medium" / "high"）
    #[serde(default)]
    pub reasoning_effort: String,
    /// 服务层级（部分 Provider 支持）
    #[serde(default)]
    pub service_tier: String,
    /// 人格配置
    #[serde(default)]
    pub personalities: PersonalitiesConfig,
}

fn default_max_turns() -> u32 { 90 }

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: default_max_turns(),
            verbose: false,
            system_prompt: String::new(),
            prefill_messages_file: String::new(),
            reasoning_effort: String::new(),
            service_tier: String::new(),
            personalities: PersonalitiesConfig::default(),
        }
    }
}

/// 工具处理模式
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub enum ToolProcessMode{
    #[serde(rename="off")]
    Off,
    #[serde(rename="new")]
    New,
    #[serde(rename="all")]
    All,
    #[serde(rename="verbose")]
    Verbose
}

impl Default for ToolProcessMode {
    fn default() -> Self {
        Self::All
    }
}

/// 显示与终端输出配置。
///
/// 控制 CLI 的输出格式、滚动摘要、推理显示、流式行为等。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct DisplayConfig {
    /// 紧凑输出模式
    #[serde(default)]
    pub compact: bool,
    /// resume 展示模式：full / compact
    #[serde(default = "default_resume_display")]
    pub resume_display: String,
    /// resume 时显示的上下文交换轮数
    #[serde(default = "default_resume_exchanges")]
    pub resume_exchanges: u32,
    /// resume 时用户消息截断字符数
    #[serde(default = "default_resume_max_user_chars")]
    pub resume_max_user_chars: u32,
    /// resume 时助手消息截断字符数
    #[serde(default = "default_resume_max_assistant_chars")]
    pub resume_max_assistant_chars: u32,
    /// resume 时助手消息最大显示行数
    #[serde(default = "default_resume_max_assistant_lines")]
    pub resume_max_assistant_lines: u32,
    /// resume 时跳过纯 tool 调用轮次
    #[serde(default = "default_resume_skip_tool_only")]
    pub resume_skip_tool_only: bool,
    /// 是否实时显示推理过程
    #[serde(default = "default_show_reasoning")]
    pub show_reasoning: bool,
    /// 是否显示完整推理过程（而非摘要）
    #[serde(default)]
    pub reasoning_full: bool,
    /// 是否启用流式输出
    #[serde(default = "default_streaming")]
    pub streaming: bool,
    /// 繁忙时输入模式：interrupt / queue
    #[serde(default = "default_busy_input_mode")]
    pub busy_input_mode: String,
    /// 是否在输出后保持内容持久可见
    #[serde(default = "default_persistent_output")]
    pub persistent_output: bool,
    /// 持久输出最大行数
    #[serde(default = "default_persistent_output_max_lines")]
    pub persistent_output_max_lines: u32,
    /// 是否在滚动历史中保留模态提示（approval/clarify）的摘要
    #[serde(default = "default_persist_prompts")]
    pub persist_prompts: bool,
    /// 显示皮肤
    #[serde(default = "default_skin")]
    pub skin: String,
    #[serde(default)]
    pub tool_process: ToolProcessMode,
}

fn default_resume_display() -> String { "full".to_string() }
fn default_resume_exchanges() -> u32 { 10 }
fn default_resume_max_user_chars() -> u32 { 300 }
fn default_resume_max_assistant_chars() -> u32 { 200 }
fn default_resume_max_assistant_lines() -> u32 { 3 }
fn default_resume_skip_tool_only() -> bool { true }
fn default_show_reasoning() -> bool { true }
fn default_streaming() -> bool { true }
fn default_busy_input_mode() -> String { "interrupt".to_string() }
fn default_persistent_output() -> bool { true }
fn default_persistent_output_max_lines() -> u32 { 200 }
fn default_persist_prompts() -> bool { true }
fn default_skin() -> String { "default".to_string() }

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            compact: false,
            resume_display: default_resume_display(),
            resume_exchanges: default_resume_exchanges(),
            resume_max_user_chars: default_resume_max_user_chars(),
            resume_max_assistant_chars: default_resume_max_assistant_chars(),
            resume_max_assistant_lines: default_resume_max_assistant_lines(),
            resume_skip_tool_only: default_resume_skip_tool_only(),
            show_reasoning: default_show_reasoning(),
            reasoning_full: false,
            streaming: default_streaming(),
            busy_input_mode: default_busy_input_mode(),
            persistent_output: default_persistent_output(),
            persistent_output_max_lines: default_persistent_output_max_lines(),
            persist_prompts: default_persist_prompts(),
            skin: default_skin(),
            tool_process: ToolProcessMode::All,
        }
    }
}

/// Clarify 配置。
///
/// 控制 Clarify 行为，如超时时间等。
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct ClarifyConfig{
    #[serde(default="default_clarify_timeout")]
    pub timeout: u32,
}

fn default_clarify_timeout() -> u32 { 120 }

impl Default for ClarifyConfig {
    fn default() -> Self {
        Self {
            timeout: default_clarify_timeout(),
        }
    }
}

/// 代码执行配置。
///
/// 控制代码执行行为，如超时时间、最大调用次数等。
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct CodeExecutionConfig{
    /// 默认超时时间（秒）
    #[serde(default="default_code_execution_timeout")]
    pub timeout: u32,
    /// 默认最大调用次数
    #[serde(default="default_max_tool_calls")]
    pub max_tool_calls: u32,
}

fn default_code_execution_timeout() -> u32 { 300 }
fn default_max_tool_calls() -> u32 { 50 }

impl Default for CodeExecutionConfig {
    fn default() -> Self {
        Self {
            timeout: default_code_execution_timeout(),
            max_tool_calls: default_max_tool_calls(),
        }
    }
}

/// 辅助 LLM 调用覆盖配置。
///
/// 对应 vision / web_extract 等子模块，可在需要时覆盖默认的 provider / model 等参数。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderOverrides {
    /// 使用的 provider
    #[serde(default = "default_provider_overrides_provider")]
    pub provider: String,
    /// 使用的模型名称，空字符串表示使用默认
    #[serde(default)]
    pub model: String,
    /// 自定义 base URL，兼容历史配置中的 `api_base` 字段
    #[serde(default, alias = "api_base")]
    pub base_url: String,
    /// API 密钥
    #[serde(default)]
    pub api_key: String,
}

fn default_provider_overrides_provider() -> String { "auto".to_string() }

impl Default for ProviderOverrides {
    fn default() -> Self {
        Self {
            provider: default_provider_overrides_provider(),
            model: String::new(),
            base_url: String::new(),
            api_key: String::new(),
        }
    }
}

/// 辅助功能配置。
///
/// 包含 vision、web_extract 等子模块的 LLM 覆盖参数。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct AuxiliaryConfig {
    /// 视觉识别模块的 LLM 覆盖配置
    #[serde(default)]
    pub vision: ProviderOverrides,
    /// 网页内容提取模块的 LLM 覆盖配置
    #[serde(default)]
    pub web_extract: ProviderOverrides,
}

impl Default for AuxiliaryConfig {
    fn default() -> Self {
        Self {
            vision: ProviderOverrides::default(),
            web_extract: ProviderOverrides::default(),
        }
    }
}

/// 子 Agent 委托配置。
///
/// 控制子 Agent（delegation）的模型、Provider 覆盖及最大迭代轮数。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct DelegationConfig {
    /// 每个子 Agent 的最大 tool-calling 轮数
    #[serde(default = "default_delegation_max_iterations")]
    pub max_iterations: u32,
    /// 子 Agent 模型覆盖（空字符串表示继承父模型）
    #[serde(default)]
    pub model: String,
    /// 子 Agent Provider 覆盖（空字符串表示继承父 Provider）
    #[serde(default)]
    pub provider: String,
    /// 子 Agent 的 OpenAI 兼容端点，兼容历史配置中的 `api_base` 字段
    #[serde(default, alias = "api_base")]
    pub base_url: String,
    /// 子 Agent 的 API 密钥（不设置则回退到 OPENAI_API_KEY）
    #[serde(default)]
    pub api_key: String,
}

fn default_delegation_max_iterations() -> u32 { 45 }

impl Default for DelegationConfig {
    fn default() -> Self {
        Self {
            max_iterations: default_delegation_max_iterations(),
            model: String::new(),
            provider: String::new(),
            base_url: String::new(),
            api_key: String::new(),
        }
    }
}

/// 首次使用引导配置。
///
/// 记录已展示过的引导提示，每种提示只展示一次。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct OnboardingConfig {
    /// 已展示过的引导提示键集合（key -> bool）
    #[serde(default)]
    pub seen: std::collections::HashMap<String, bool>,
}

impl Default for OnboardingConfig {
    fn default() -> Self {
        Self {
            seen: std::collections::HashMap::new(),
        }
    }
}



/// sagent CLI 配置项。
///
/// 控制 CLI 的行为偏好，如默认 Profile、REPL 历史记录等。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SAgentCLIConfig {
    #[serde(default)]
    pub model: Option<ModelConfig>,
    #[serde(default)]
    pub terminal: Option<TerminalConfig>,
    /// 浏览器环境配置
    #[serde(default)]
    pub browser: Option<BrowserConfig>,
    #[serde(default)]
    pub compression: CompressionConfig,
    /// Agent 行为配置
    #[serde(default)]
    pub agent: AgentConfig,
    /// 显示与终端输出配置
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub clarify: ClarifyConfig,
    #[serde(default)]
    pub code_execution: CodeExecutionConfig,
    /// 辅助功能配置（vision / web_extract）
    #[serde(default)]
    pub auxiliary: Option<AuxiliaryConfig>,
    /// 子 Agent 委托配置
    #[serde(default)]
    pub delegation: DelegationConfig,
    /// 首次使用引导配置
    #[serde(default)]
    pub onboarding: OnboardingConfig,
}

impl Default for SAgentCLIConfig {
    fn default() -> Self {
        Self {
            model: Some(ModelConfig::default()),
            terminal: Some(TerminalConfig::default()),
            browser: Some(BrowserConfig::default()),
            compression: CompressionConfig::default(),
            agent: AgentConfig::default(),
            display: DisplayConfig::default(),
            clarify: ClarifyConfig::default(),
            code_execution: CodeExecutionConfig::default(),
            auxiliary: Some(AuxiliaryConfig::default()),
            delegation: DelegationConfig::default(),
            onboarding: OnboardingConfig::default(),
        }
    }
}


/// 递归遍历 `serde_yaml::Value`，将字符串中的 `${VAR_NAME}` 替换为对应的环境变量值。
///
/// 若环境变量不存在，则保留原始 `${VAR_NAME}` 不替换。
///
/// 示例：
/// ```yaml
/// api_key: "${OPENAI_API_KEY}"   # → 替换为 $OPENAI_API_KEY 的值
/// base_url: "https://${HOST}:8080"  # → 仅替换 ${HOST} 部分
/// ```
fn substitute_env_vars(value: &mut serde_yaml::Value) {
    let re = regex::Regex::new(r"\$\{([^}]+)\}").expect("invalid env var regex");

    match value {
        serde_yaml::Value::String(s) => {
            let result = re.replace_all(s, |caps: &regex::Captures| {
                std::env::var(&caps[1]).unwrap_or_else(|_| caps[0].to_string())
            });
            *s = result.to_string();
        }
        serde_yaml::Value::Sequence(seq) => {
            for v in seq {
                substitute_env_vars(v);
            }
        }
        serde_yaml::Value::Mapping(map) => {
            for (_, v) in map {
                substitute_env_vars(v);
            }
        }
        _ => {} // 非字符串类型（数字、布尔等）不处理
    }
}


/// 全局配置缓存，首次访问时自动加载并缓存。
static _MANAGED_CONFIG_CACHE: OnceLock<SAgentCLIConfig> = OnceLock::new();


/// managed overlay 解析缓存条目。
struct ManagedOverlayCache {
    /// 缓存 key：(mtime_ns, file_size)
    key: (i64, u64),
    config: SAgentCLIConfig,
}

/// managed overlay 文件的解析缓存，按文件时间戳+大小判断是否命中。
static _MANAGED_OVERLAY_CACHE: RwLock<Option<ManagedOverlayCache>> = RwLock::new(None);

fn load_managed_config() -> Option<SAgentCLIConfig> {
    let managed_dir = get_managed_dir()?;
    let managed_config_file = managed_dir.join("config.yaml");

    // 获取文件元数据，构建缓存 key：(mtime_ns, file_size)
    let metadata = std::fs::metadata(&managed_config_file).ok()?;
    let mtime_ns = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)?;
    let cache_key = (mtime_ns, metadata.len());

    // 先查缓存
    {
        let cache = _MANAGED_OVERLAY_CACHE.read().unwrap();
        if let Some(ref entry) = *cache {
            if entry.key == cache_key {
                // 缓存命中，直接用缓存的配置覆盖
                return Option::Some(entry.config.clone());
            }
        }
    }

    // 缓存未命中，解析文件
    let result = (|| -> anyhow::Result<SAgentCLIConfig> {
        let contents = std::fs::read_to_string(&managed_config_file)?;
        let mut raw: serde_yaml::Value = serde_yaml::from_str(&contents)?;
        substitute_env_vars(&mut raw);
        Ok(serde_yaml::from_value(raw)?)
    })();

    result.ok().map(|managed|{
        let mut cache = _MANAGED_OVERLAY_CACHE.write().unwrap();
        *cache = Some(ManagedOverlayCache {
            key: cache_key,
            config: managed.clone(),
        });
        managed
    })
}

/// 加载 CLI 配置。
///
/// 配置层级：代码默认值 → 用户配置文件（`~/.sagent/config.yaml`）。
/// 文件中未设置的字段会自动回退到代码默认值。
///
/// 设置环境变量 `SAGENT_IGNORE_USER_CONFIG=1` 可跳过用户配置文件加载。
pub fn load_cli_config() -> anyhow::Result<SAgentCLIConfig> {
    let user_config_path = get_sagent_home().join("config.yaml");

    // 以代码默认值为基础，后续叠加用户配置文件
    let mut config = SAgentCLIConfig::default();
    let mut file_has_terminal_config = false;

    if user_config_path.exists()
        && !std::env::var("SAGENT_IGNORE_USER_CONFIG").is_ok_and(|v| v == "1")
    {
        let contents = std::fs::read_to_string(&user_config_path)?;
        // 先解析为通用 Value 树，递归替换其中的 ${ENV_VAR} 后再反序列化为强类型
        let mut raw: serde_yaml::Value = serde_yaml::from_str(&contents)?;
        // 检查用户配置文件中是否显式包含 terminal 段
        file_has_terminal_config = raw.get("terminal").is_some();
        substitute_env_vars(&mut raw);
        let file_config: SAgentCLIConfig = serde_yaml::from_value(raw)?;
        // 文件中的配置覆盖默认值（未设置的字段保留默认值，由 serde(default) 保证）
        config = file_config;
    }

    // 应用 managed overlay（如 /etc/sagent/config.yaml 的额外覆盖）
    if let Some(managed) = load_managed_config() {
        config = managed;
    }

    let effective_backend = config
        .terminal
        .as_ref()
        .map_or_else(|| "local".to_string(), |t| t.env_type.clone());

    match effective_backend.as_str() {
        "local" => {
            config.terminal.as_mut().unwrap().cwd = Some(std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .display()
                .to_string());
        }
        "." | "auto" | "cwd" => {
            config.terminal.as_mut().unwrap().cwd = None;
        }
        _ => {}
    }

    // 将最终配置同步到环境变量，便于子进程和工具链读取
    apply_config_to_env(&config, file_has_terminal_config);

    Ok(config)
}


static GLOBAL_CLI_CONFIG: OnceLock<SAgentCLIConfig> = OnceLock::new();
/// 获取全局 CLI 配置。
///
/// 首次调用时加载配置并缓存，后续调用直接返回缓存值。
/// 如果配置加载失败，将在调试日志中记录错误并返回代码默认值。
///
/// # 示例
///
/// ```ignore
/// use sagent_cli::config::get_config;
///
/// let config = get_config();
/// println!("max_turns: {}", config.agent.max_turns);
/// ```
pub fn get_config() -> &'static SAgentCLIConfig {
    GLOBAL_CLI_CONFIG.get_or_init(|| {
        match load_cli_config() {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!(error = %e, "加载 CLI 配置失败，使用默认配置");
                SAgentCLIConfig::default()
            }
        }
    })
}

/// 将 JSON Value 转为环境变量字符串。
///
/// `None` 或 `Null` → `None`；
/// 字符串 → 直接克隆；
/// 数组/对象 → JSON 序列化；
/// 其余（布尔、数字）→ `to_string()`。
fn json_value_to_env_string(v: Option<&serde_json::Value>) -> Option<String> {
    match v {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                serde_json::to_string(v).unwrap_or_default()
            }
            _ => v.to_string(),
        }),
    }
}

/// 将已加载的配置项同步到对应的环境变量。
///
/// `file_has_terminal_config`：配置文件是否显式包含 `terminal` 段。
/// 为 true 时所有字段均同步到环境变量（覆盖已有值）；
/// 为 false 时仅同步环境变量尚未设置的项。
fn apply_config_to_env(config: &SAgentCLIConfig, file_has_terminal_config: bool) {
    // config_key → 环境变量名 的映射表
    let env_mappings: &[(&str, &str)] = &[
        ("env_type", "TERMINAL_ENV"),
        ("cwd", "TERMINAL_CWD"),
        ("timeout", "TERMINAL_TIMEOUT"),
        ("home_mode", "TERMINAL_HOME_MODE"),
        ("lifetime_seconds", "TERMINAL_LIFETIME_SECONDS"),
        ("docker_image", "TERMINAL_DOCKER_IMAGE"),
        ("docker_forward_env", "TERMINAL_DOCKER_FORWARD_ENV"),
        ("singularity_image", "TERMINAL_SINGULARITY_IMAGE"),
        ("modal_image", "TERMINAL_MODAL_IMAGE"),
        ("daytona_image", "TERMINAL_DAYTONA_IMAGE"),
        // SSH config
        ("ssh_host", "TERMINAL_SSH_HOST"),
        ("ssh_user", "TERMINAL_SSH_USER"),
        ("ssh_port", "TERMINAL_SSH_PORT"),
        ("ssh_key", "TERMINAL_SSH_KEY"),
        // Container resource config
        ("container_cpu", "TERMINAL_CONTAINER_CPU"),
        ("container_memory", "TERMINAL_CONTAINER_MEMORY"),
        ("container_disk", "TERMINAL_CONTAINER_DISK"),
        ("container_persistent", "TERMINAL_CONTAINER_PERSISTENT"),
        ("docker_volumes", "TERMINAL_DOCKER_VOLUMES"),
        ("docker_env", "TERMINAL_DOCKER_ENV"),
        ("docker_extra_args", "TERMINAL_DOCKER_EXTRA_ARGS"),
        ("docker_mount_cwd_to_workspace", "TERMINAL_DOCKER_MOUNT_CWD_TO_WORKSPACE"),
        ("docker_network", "TERMINAL_DOCKER_NETWORK"),
        ("docker_run_as_host_user", "TERMINAL_DOCKER_RUN_AS_HOST_USER"),
        ("docker_persist_across_processes", "TERMINAL_DOCKER_PERSIST_ACROSS_PROCESSES"),
        ("docker_orphan_reaper", "TERMINAL_DOCKER_ORPHAN_REAPER"),
        ("sandbox_dir", "TERMINAL_SANDBOX_DIR"),
        ("persistent_shell", "TERMINAL_PERSISTENT_SHELL"),
        ("sudo_password", "SUDO_PASSWORD"),
    ];

    let is_gateway = std::env::var("_SAGENT_GATEWAY").is_ok();

    // 将 TerminalConfig 序列化为 JSON，通过 config_key 动态索引字段值
    let term_json = config
        .terminal
        .as_ref()
        .and_then(|t| serde_json::to_value(t).ok());

    for &(config_key, env_var) in env_mappings {
        let raw_val = term_json.as_ref().and_then(|tj| tj.get(config_key));
        let val = json_value_to_env_string(raw_val);

        if let Some(val) = val {
            if env_var == "TERMINAL_CWD" {
                // Gateway 模式跳过 CWD（由 Gateway 自行管理）；CLI 始终导出
                if is_gateway {
                    continue;
                }
                std::env::set_var(env_var, &val);
            } else if file_has_terminal_config || std::env::var(env_var).is_err() {
                // 配置文件显式包含 terminal 段，或环境变量尚未设置 → 同步
                std::env::set_var(env_var, &val);
            }
        }
    }

    // 将 BrowserConfig 序列化为 JSON，通过 config_key 动态索引字段值
    let browser_json = config
        .browser
        .as_ref()
        .and_then(|t| serde_json::to_value(t).ok());
    let browser_env_mappings = &[
        ("inactivity_timeout", "BROWSER_INACTIVITY_TIMEOUT")
    ];
    for &(config_key, env_var) in browser_env_mappings {
        let raw_val = browser_json.as_ref().and_then(|tj| tj.get(config_key));
        let val = json_value_to_env_string(raw_val);
        if let Some(val) = val {
            std::env::set_var(env_var, &val);
        }
    }

    let auxiliary_config = config
        .auxiliary
        .as_ref()
        .and_then(|t| serde_json::to_value(t).ok());

    // task_key → [(字段名, 环境变量名)]
    let auxiliary_task_env: &[(&str, &[(&str, &str)])] = &[
        ("vision", &[
            ("provider", "AUXILIARY_VISION_PROVIDER"),
            ("model", "AUXILIARY_VISION_MODEL"),
            ("base_url", "AUXILIARY_VISION_BASE_URL"),
            ("api_key", "AUXILIARY_VISION_API_KEY"),
        ]),
        ("web_extract", &[
            ("provider", "AUXILIARY_WEB_EXTRACT_PROVIDER"),
            ("model", "AUXILIARY_WEB_EXTRACT_MODEL"),
            ("base_url", "AUXILIARY_WEB_EXTRACT_BASE_URL"),
            ("api_key", "AUXILIARY_WEB_EXTRACT_API_KEY"),
        ]),
        ("approval", &[
            ("provider", "AUXILIARY_APPROVAL_PROVIDER"),
            ("model", "AUXILIARY_APPROVAL_MODEL"),
            ("base_url", "AUXILIARY_APPROVAL_BASE_URL"),
            ("api_key", "AUXILIARY_APPROVAL_API_KEY"),
        ]),
    ];

    for &(task_key, env_map) in auxiliary_task_env {
        let task_cfg = auxiliary_config.as_ref().and_then(|ac| ac.get(task_key));
        for &(field, env_var) in env_map {
            let raw_val = task_cfg.and_then(|tc| tc.get(field));
            if let Some(v) = json_value_to_env_string(raw_val) {
                if v.is_empty() {
                    continue;
                }
                // provider 为 "auto" 时跳过（表示未显式覆盖）
                if field == "provider" && v == "auto" {
                    continue;
                }
                std::env::set_var(env_var, &v);
            }
        }
    }

    // TODO: 将 security 配置同步到环境变量
    // let security_config = config.security.as_ref().and_then(|sc| serde_json::to_value(sc).ok());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ==================== json_value_to_env_string ====================

    #[test]
    fn test_value_none_returns_none() {
        assert_eq!(json_value_to_env_string(None), None);
    }

    #[test]
    fn test_value_null_returns_none() {
        assert_eq!(json_value_to_env_string(Some(&json!(null))), None);
    }

    #[test]
    fn test_value_string() {
        let v = json!("hello");
        assert_eq!(json_value_to_env_string(Some(&v)), Some("hello".to_string()));
    }

    #[test]
    fn test_value_empty_string() {
        let v = json!("");
        assert_eq!(json_value_to_env_string(Some(&v)), Some("".to_string()));
    }

    #[test]
    fn test_value_bool() {
        let v = json!(true);
        assert_eq!(json_value_to_env_string(Some(&v)), Some("true".to_string()));
        let v = json!(false);
        assert_eq!(json_value_to_env_string(Some(&v)), Some("false".to_string()));
    }

    #[test]
    fn test_value_number() {
        let v = json!(42);
        assert_eq!(json_value_to_env_string(Some(&v)), Some("42".to_string()));
        let v = json!(3.14);
        assert_eq!(json_value_to_env_string(Some(&v)), Some("3.14".to_string()));
    }

    #[test]
    fn test_value_array() {
        let v = json!(["a", "b"]);
        assert_eq!(
            json_value_to_env_string(Some(&v)),
            Some(r#"["a","b"]"#.to_string())
        );
    }

    #[test]
    fn test_value_empty_array() {
        let v = json!([]);
        assert_eq!(json_value_to_env_string(Some(&v)), Some("[]".to_string()));
    }

    #[test]
    fn test_value_object() {
        let v = json!({"key": "value"});
        assert_eq!(
            json_value_to_env_string(Some(&v)),
            Some(r#"{"key":"value"}"#.to_string())
        );
    }

    // ==================== apply_config_to_env ====================
    // 注意：这些测试会修改进程级环境变量，使用 Mutex 串行化执行。

    use std::sync::Mutex;

    /// 测试环境变量操作的串行锁。
    static ENV_TEST_MUTEX: Mutex<()> = Mutex::new(());

    /// 构造一个默认配置，保留 Custom terminal 便于按需覆盖各字段。
    fn default_test_config() -> SAgentCLIConfig {
        SAgentCLIConfig::default()
    }

    /// 清理测试中添加的环境变量。
    fn cleanup_env_vars(vars: &[&str]) {
        for var in vars {
            std::env::remove_var(var);
        }
    }

    #[test]
    fn test_terminal_cwd_set_for_cli() {
        let _lock = ENV_TEST_MUTEX.lock().unwrap();
        let ev = "TERMINAL_CWD";
        std::env::remove_var(ev);
        // 确保不是 gateway
        std::env::remove_var("_SAGENT_GATEWAY");

        let mut config = default_test_config();
        config.terminal.as_mut().unwrap().cwd = Some("/home/test".to_string());
        apply_config_to_env(&config, false);

        assert_eq!(std::env::var(ev).ok().as_deref(), Some("/home/test"));

        cleanup_env_vars(&[ev]);
    }

    #[test]
    fn test_terminal_cwd_skipped_in_gateway() {
        let _lock = ENV_TEST_MUTEX.lock().unwrap();
        let ev = "TERMINAL_CWD";
        std::env::remove_var(ev);
        std::env::set_var("_SAGENT_GATEWAY", "1");

        let mut config = default_test_config();
        config.terminal.as_mut().unwrap().cwd = Some("/gateway/path".to_string());
        apply_config_to_env(&config, false);

        assert_eq!(std::env::var(ev).ok(), None);

        cleanup_env_vars(&[ev, "_SAGENT_GATEWAY"]);
    }

    #[test]
    fn test_file_has_terminal_true_overrides_existing() {
        let _lock = ENV_TEST_MUTEX.lock().unwrap();
        let ev = "TERMINAL_ENV";
        std::env::remove_var(ev);
        std::env::set_var(ev, "old_value");

        let mut config = default_test_config();
        config.terminal.as_mut().unwrap().env_type = "docker".to_string();
        // file_has_terminal_config = true → 即使环境变量已存在也要覆盖
        apply_config_to_env(&config, true);

        assert_eq!(std::env::var(ev).ok().as_deref(), Some("docker"));

        cleanup_env_vars(&[ev]);
    }

    #[test]
    fn test_file_has_terminal_false_preserves_existing() {
        let _lock = ENV_TEST_MUTEX.lock().unwrap();
        let ev = "TERMINAL_ENV";
        std::env::remove_var(ev);
        std::env::set_var(ev, "existing_env");

        let mut config = default_test_config();
        config.terminal.as_mut().unwrap().env_type = "singularity".to_string();
        // file_has_terminal_config = false + 环境变量已存在 → 不覆盖
        apply_config_to_env(&config, false);

        assert_eq!(std::env::var(ev).ok().as_deref(), Some("existing_env"));

        cleanup_env_vars(&[ev]);
    }

    #[test]
    fn test_browser_inactivity_timeout_set() {
        let _lock = ENV_TEST_MUTEX.lock().unwrap();
        let ev = "BROWSER_INACTIVITY_TIMEOUT";
        std::env::remove_var(ev);

        let mut config = default_test_config();
        config.browser.as_mut().unwrap().inactivity_timeout = 600;
        apply_config_to_env(&config, false);

        assert_eq!(std::env::var(ev).ok().as_deref(), Some("600"));

        cleanup_env_vars(&[ev]);
    }

    #[test]
    fn test_auxiliary_vision_env_vars() {
        let _lock = ENV_TEST_MUTEX.lock().unwrap();
        let vars = [
            "AUXILIARY_VISION_PROVIDER",
            "AUXILIARY_VISION_MODEL",
            "AUXILIARY_VISION_BASE_URL",
            "AUXILIARY_VISION_API_KEY",
        ];
        for v in &vars {
            std::env::remove_var(v);
        }

        let mut config = default_test_config();
        let aux = config.auxiliary.as_mut().unwrap();
        aux.vision.provider = "openai".to_string();
        aux.vision.model = "gpt-4o".to_string();
        aux.vision.base_url = "https://custom.api/v1".to_string();
        aux.vision.api_key = "sk-test123".to_string();
        apply_config_to_env(&config, false);

        assert_eq!(std::env::var("AUXILIARY_VISION_PROVIDER").ok().as_deref(), Some("openai"));
        assert_eq!(std::env::var("AUXILIARY_VISION_MODEL").ok().as_deref(), Some("gpt-4o"));
        assert_eq!(std::env::var("AUXILIARY_VISION_BASE_URL").ok().as_deref(), Some("https://custom.api/v1"));
        assert_eq!(std::env::var("AUXILIARY_VISION_API_KEY").ok().as_deref(), Some("sk-test123"));

        cleanup_env_vars(&vars);
    }

    #[test]
    fn test_auxiliary_provider_auto_skipped() {
        let _lock = ENV_TEST_MUTEX.lock().unwrap();
        let ev = "AUXILIARY_VISION_PROVIDER";
        std::env::remove_var(ev);

        let mut config = default_test_config();
        // provider 默认为 "auto"，应被跳过不设置
        config.auxiliary.as_mut().unwrap().vision.provider = "auto".to_string();
        apply_config_to_env(&config, false);

        assert_eq!(std::env::var(ev).ok(), None);

        cleanup_env_vars(&[ev]);
    }

    #[test]
    fn test_auxiliary_web_extract_env_vars() {
        let _lock = ENV_TEST_MUTEX.lock().unwrap();
        let vars = [
            "AUXILIARY_WEB_EXTRACT_PROVIDER",
            "AUXILIARY_WEB_EXTRACT_MODEL",
        ];
        for v in &vars {
            std::env::remove_var(v);
        }

        let mut config = default_test_config();
        let aux = config.auxiliary.as_mut().unwrap();
        aux.web_extract.provider = "anthropic".to_string();
        aux.web_extract.model = "claude-sonnet".to_string();
        apply_config_to_env(&config, false);

        assert_eq!(std::env::var("AUXILIARY_WEB_EXTRACT_PROVIDER").ok().as_deref(), Some("anthropic"));
        assert_eq!(std::env::var("AUXILIARY_WEB_EXTRACT_MODEL").ok().as_deref(), Some("claude-sonnet"));

        cleanup_env_vars(&vars);
    }
}

