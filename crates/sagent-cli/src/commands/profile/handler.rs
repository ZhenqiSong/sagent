//! Profile CLI 命令处理器。
//!
//! 本模块负责将命令参数编排为 Profile 领域操作并适配 CLI 输出；Profile 索引和状态
//! 规则由 `sagent_config::Profile` 负责，目录/配置/存储初始化由 `ProfileService` 负责。
//!
//! 作者：SongZQ

use anyhow::Result;
use sagent_config::{Profile, paths::profile_root, resolve_paths};

use super::{ProfileCommand, ProfileService};
use crate::{
    commands::{Command, CommandContext, CommandHandler},
    output::print_output,
};

/// Profile 命令的领域处理器。
///
/// 处理器直接拥有本次 CLI 调用的上下文，但不保存待执行的命令值；Profile 目录由配置层
/// 索引，创建副作用交给 `ProfileService`，处理器只承担输入到输出的命令编排。
pub struct ProfileHandler {
    /// 当前 CLI 调用的共享参数与惰性存储装配。
    context: CommandContext,
    /// 根据 `CommandContext.home` 一次构建出的 Profile 索引。
    profile: Profile,
}

impl ProfileHandler {
    /// 创建拥有命令上下文的 Profile 处理器。
    ///
    /// CLI 先保留 `--home` 的稳定错误语义，再交给配置层解析环境优先级和 Profile 根路径；
    /// Profile 对象自身只接收已解析的绝对根目录。
    pub(crate) fn new(context: CommandContext) -> Result<Self> {
        if context
            .home
            .as_deref()
            .is_some_and(|home| !home.is_absolute())
        {
            anyhow::bail!("--home 必须是绝对路径");
        }
        let paths = resolve_paths(context.home.as_deref(), None)?;
        let root = profile_root(&paths.sagent_home);
        let profile = Profile::from_root(&root)?;
        Ok(Self { context, profile })
    }

    /// 执行传入的 Profile 子命令。
    fn execute_command(&mut self, command: ProfileCommand) -> Result<()> {
        match command {
            ProfileCommand::List => {
                let lines = self.list_lines()?;
                print_output(self.context.format, &lines, lines.clone())
            }
            ProfileCommand::Create { name } => {
                let path = ProfileService::create(self.profile.root_path()?, &name)?;
                self.profile.register(&name, path.clone())?;
                let path = path.display().to_string();
                let value = serde_json::json!({ "path": path.clone() });
                print_output(
                    self.context.format,
                    &value,
                    vec![format!("已创建 profile: {path}")],
                )
            }
            ProfileCommand::Use { name } => {
                let selected = self.profile.select(&name)?;
                let value = serde_json::json!({ "profile": selected.clone() });
                print_output(
                    self.context.format,
                    &value,
                    vec![format!("当前 profile: {selected}")],
                )
            }
        }
    }

    /// 将 Profile 快照格式化为稳定的文本列表；目录扫描已在 handler 构造阶段完成。
    fn list_lines(&self) -> Result<Vec<String>> {
        let mut lines = Vec::new();
        let mut entries = self.profile.entries().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.name().as_str().cmp(right.name().as_str()));

        // default 始终置顶，其余条目按名称排序，避免 HashMap 的随机遍历顺序泄漏到 CLI。
        if let Some(default) = entries
            .iter()
            .find(|profile| profile.name().as_str() == "default")
        {
            lines.push(self.format_profile_line(default.name().as_str(), default.is_active()));
        }
        lines.extend(
            entries
                .into_iter()
                .filter(|profile| profile.name().as_str() != "default")
                .map(|profile| {
                    self.format_profile_line(profile.name().as_str(), profile.is_active())
                }),
        );
        Ok(lines)
    }

    /// 按激活标识格式化一条列表输出；展示层只依赖名称和状态，不接触路径元数据。
    fn format_profile_line(&self, name: &str, is_active: bool) -> String {
        if is_active {
            format!("* {name}")
        } else {
            format!("  {name}")
        }
    }
}

impl CommandHandler for ProfileHandler {
    /// 执行传入的顶层 Profile 命令，拒绝错误的领域类型。
    fn execute(&mut self, command: Command) -> Result<()> {
        let Command::Profile { command } = command else {
            anyhow::bail!("ProfileHandler 收到非 profile 命令");
        };
        self.execute_command(command)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::Profile;

    fn test_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sagent-cli-profile-{name}-{}", std::process::id()))
    }

    #[test]
    fn lists_profiles_in_stable_order_and_marks_active_one() {
        let root_dir = test_root("list");
        let _ = fs::remove_dir_all(&root_dir);
        fs::create_dir_all(root_dir.join("profiles").join("writer")).expect("应能创建 writer");
        fs::create_dir_all(root_dir.join("profiles").join("coder")).expect("应能创建 coder");

        let profile = Profile::from_root(&root_dir).expect("应能创建 profile 对象");

        let mut handler = super::ProfileHandler {
            context: super::CommandContext::new(
                Some(root_dir.clone()),
                None,
                crate::output::OutputFormat::Text,
            ),
            profile,
        };
        assert_eq!(
            handler.list_lines().expect("应能列出 profile"),
            vec!["* default", "  coder", "  writer"]
        );
        handler.profile.select("coder").expect("应能选择 profile");
        assert_eq!(
            handler.list_lines().expect("应能标记当前 profile"),
            vec!["  default", "* coder", "  writer"]
        );
        fs::remove_dir_all(root_dir).expect("应能清理测试目录");
    }

    #[test]
    fn rejects_relative_root_and_does_not_overwrite_existing_profile() {
        assert!(Profile::from_root(Path::new("relative")).is_err());

        let root_dir = test_root("duplicate");
        let profile_dir = root_dir.join("profiles").join("coder");
        let _ = fs::remove_dir_all(&root_dir);
        fs::create_dir_all(&profile_dir).expect("应能创建既有 profile");
        fs::write(profile_dir.join("keep.txt"), "keep").expect("应能写入哨兵文件");
        let profile = Profile::from_root(&root_dir).expect("应能创建 profile 对象");
        assert!(super::ProfileService::create(profile.root_path().unwrap(), "coder").is_err());
        assert_eq!(
            fs::read_to_string(profile_dir.join("keep.txt")).unwrap(),
            "keep"
        );
        fs::remove_dir_all(root_dir).expect("应能清理测试目录");
    }
}
