//! Profile 标识、目录索引与 active 状态管理。
//!
//! 本模块负责 Profile 名称不变量、根目录下的 Profile 快照以及 active-profile 原子读写；
//! 不创建配置内容或数据库，也不依赖 CLI/Runtime 编排。
//!
//! 作者：SongZQ
//! 创建日期：2026-08-29

use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

/// 已规范化、可安全用于 Profile 路径解析的名称。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileName(String);

impl ProfileName {
    /// 返回内部字符串的只读引用（零拷贝，不转移所有权）。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 单个 Profile 的不可变描述信息。
///
/// 名称同时保存在 Profile 集合的 key 和这里，是为了让从集合中取出的条目自描述；路径
/// 是该 Profile 的实际目录，`default` 指向根目录，命名 Profile 指向 `profiles/<name>`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileInfo {
    /// Profile 的规范化名称，也是 `Profile` 集合中的索引键。
    name: ProfileName,
    /// Profile 配置和状态文件所在的绝对目录。
    path: PathBuf,
    /// 是否为当前活动 Profile；一个集合中始终最多且应当恰好有一个为 true。
    active: bool,
}

impl ProfileInfo {
    /// 返回 Profile 的规范化名称。
    pub fn name(&self) -> &ProfileName {
        &self.name
    }

    /// 返回 Profile 配置和状态文件所在的绝对目录。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 返回该 Profile 是否为当前活动项。
    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// Profile 集合及其目录索引。
///
/// 该对象只持有一个 map，避免同时缓存根目录、活动名称和目录列表等相互独立的状态。
/// 根目录通过不可变的 `default` 条目派生；目录发现和活动标记由 `from_root` 在构造时
/// 完成一次，后续调用只使用这份快照。
pub struct Profile {
    /// 以规范化名称索引的 Profile 描述信息。
    profiles: HashMap<String, ProfileInfo>,
}

impl Profile {
    /// 从已经解析且必须为绝对路径的 Sagent 根目录创建 Profile 索引。
    ///
    /// 调用方负责在更高层解析 home/environment；本领域对象不读取环境变量，避免与
    /// `resolve_paths` 重复解析同一份启动参数。
    pub fn from_root(root: &Path) -> Result<Self> {
        if !root.is_absolute() {
            bail!("Sagent 根目录必须是绝对路径");
        }

        let active = read_active_profile_marker(root)?;
        let mut profiles = list_profile_names(root)?
            .into_iter()
            .map(|name| {
                let key = name.as_str().to_owned();
                let path = if name.as_str() == "default" {
                    root.to_path_buf()
                } else {
                    root.join("profiles").join(name.as_str())
                };
                let info = ProfileInfo {
                    name,
                    path,
                    // 先统一初始化为 false，下面只把 active-profile 指向的条目设为 true。
                    active: false,
                };
                (key, info)
            })
            .collect::<HashMap<_, _>>();

        let Some(active_profile) = profiles.get_mut(active.as_str()) else {
            bail!("当前 active profile 不在 Profile 索引中");
        };
        active_profile.active = true;
        Ok(Self { profiles })
    }

    /// 返回集合中的所有 Profile 条目；HashMap 不承诺遍历顺序，展示层应自行排序。
    pub fn entries(&self) -> impl Iterator<Item = &ProfileInfo> {
        self.profiles.values()
    }

    /// 从 `default` 条目派生共享的 Profile 根目录。
    ///
    /// `from_root` 始终插入该条目；如果未来出现破坏内部不变量的构造路径，这里返回带
    /// 上下文的错误而不是让后续文件操作静默使用错误目录。
    pub fn root_path(&self) -> Result<&Path> {
        self.profiles
            .get("default")
            .map(ProfileInfo::path)
            .context("Profile 集合缺少 default 条目")
    }

    /// 返回构造时从 active-profile 文件读取并冻结的当前活动 Profile。
    ///
    /// 该方法只读取 map 中的激活标识，不再次访问文件系统；运行中的 Profile 快照因此
    /// 不会因外部文件变化而出现部分更新。
    pub fn read_active_profile(&self) -> Result<ProfileName> {
        Ok(self.active_info()?.name().clone())
    }

    /// 返回当前 Profile 根目录下的 active-profile 标记路径。
    ///
    /// 路径从 `default` 条目派生，避免调用方同时维护另一份根目录状态；返回错误表示
    /// Profile 索引不满足构造时建立的 default 不变量。
    pub fn active_profile_path(&self) -> Result<PathBuf> {
        Ok(active_profile_marker_path(self.root_path()?))
    }

    /// 将刚成功创建的 Profile 注册到索引中。
    ///
    /// 只有目录和初始存储都初始化成功后才应调用此方法；因此失败回滚不会在集合中
    /// 留下半成品条目。
    pub fn register(&mut self, name: &str, path: PathBuf) -> Result<()> {
        let profile = normalize_profile_name(name)?;
        if profile.as_str() == "default" {
            bail!("default profile 使用根目录，无需注册");
        }
        if !path.is_absolute() {
            bail!("Profile 路径必须是绝对路径");
        }
        let expected_path = self.root_path()?.join("profiles").join(profile.as_str());
        if path != expected_path {
            bail!("Profile 路径与名称不匹配");
        }
        if self.profiles.contains_key(profile.as_str()) {
            bail!("profile '{}' 已存在", profile.as_str());
        }

        let name = profile.as_str().to_owned();
        self.profiles.insert(
            name.clone(),
            ProfileInfo {
                name: profile,
                path,
                active: false,
            },
        );
        Ok(())
    }

    /// 选择一个已在索引中的 Profile，并返回规范化后的名称。
    pub fn select(&mut self, name: &str) -> Result<String> {
        let profile = normalize_profile_name(name)?;
        if !self.profiles.contains_key(profile.as_str()) {
            bail!("profile '{}' 不存在", profile.as_str());
        }

        // `from_root` 已校验根路径并建立了 Profile 快照；这里直接使用快照派生的根目录，
        // 只保留切换所需的标记写入和目标目录检查，避免重复校验同一不变量。
        let root = self.root_path()?.to_path_buf();
        let marker = active_profile_marker_path(&root);
        if profile.as_str() == "default" {
            match fs::remove_file(&marker) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("清除当前 profile 失败：{}", marker.display()));
                }
            }
        } else {
            if !root.join("profiles").join(profile.as_str()).is_dir() {
                bail!("profile '{}' 不存在", profile.as_str());
            }

            let temporary = create_active_profile_temp_file(&root, profile.as_str())?;
            if let Err(error) = fs::rename(&temporary, &marker) {
                let _ = fs::remove_file(&temporary);
                return Err(error)
                    .with_context(|| format!("发布当前 profile 失败：{}", marker.display()));
            }
        }

        // 只有标记文件成功发布后才更新内存快照，避免磁盘写失败时出现内存与磁盘不一致。
        for info in self.profiles.values_mut() {
            info.active = info.name.as_str() == profile.as_str();
        }
        Ok(profile.as_str().to_owned())
    }

    /// 返回唯一的活动 Profile，检测并拒绝零个或多个活动项的不变量破坏。
    fn active_info(&self) -> Result<&ProfileInfo> {
        let mut active = self.profiles.values().filter(|profile| profile.active);
        let Some(profile) = active.next() else {
            bail!("Profile 集合缺少活动项");
        };
        if active.next().is_some() {
            bail!("Profile 集合不能包含多个活动项");
        }
        Ok(profile)
    }
}

/// 根目录中保存当前命名 profile 的选择文件名。
const ACTIVE_PROFILE_FILE: &str = "active-profile";

/// 根据已校验的根目录计算 active-profile 标记路径。
fn active_profile_marker_path(root: &Path) -> PathBuf {
    root.join(ACTIVE_PROFILE_FILE)
}

/// 从根目录读取当前选中的 Profile，供 `Profile::from_root` 初始化快照。
///
/// 缺少选择文件时默认使用 default。对于命名 profile，同时验证目录仍存在，
/// 让损坏的选择状态在路径解析前就能得到明确错误。
fn read_active_profile_marker(root: &Path) -> Result<ProfileName> {
    if !root.is_absolute() {
        bail!("Sagent 根目录必须是绝对路径");
    }

    let marker = active_profile_marker_path(root);
    let content = match fs::read_to_string(&marker) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProfileName("default".to_owned()));
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取当前 profile 失败：{}", marker.display()));
        }
    };
    let profile = normalize_profile_name(&content)
        .with_context(|| format!("当前 profile 文件内容无效：{}", marker.display()))?;
    if profile.as_str() != "default" && !root.join("profiles").join(profile.as_str()).is_dir() {
        bail!("当前 profile '{}' 的目录不存在", profile.as_str());
    }
    Ok(profile)
}

/// 在选择文件的同目录创建并同步临时内容，以支持原子发布。
fn create_active_profile_temp_file(root: &Path, profile: &str) -> Result<PathBuf> {
    for attempt in 0..100 {
        let temporary = root.join(format!(
            ".{ACTIVE_PROFILE_FILE}-{}-{attempt}.tmp",
            std::process::id()
        ));
        let opened = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary);
        let Ok(mut file) = opened else {
            continue;
        };
        if let Err(error) = file
            .write_all(format!("{profile}\n").as_bytes())
            .and_then(|()| file.sync_all())
        {
            let _ = fs::remove_file(&temporary);
            return Err(error).with_context(|| {
                format!("写入当前 profile 临时文件失败：{}", temporary.display())
            });
        }
        return Ok(temporary);
    }
    bail!("无法创建当前 profile 临时文件")
}

/// 返回指定 Sagent 根目录中可用的 profile。
///
/// default 永远存在于结果首位，它直接对应根目录；命名 profile 则来自
/// `&lt;root&gt;/profiles/` 下名称合法的直接子目录。无效目录不会作为 profile
/// 暴露给调用方，避免历史残留或手工创建的路径绕过名称校验。
fn list_profile_names(root: &Path) -> Result<Vec<ProfileName>> {
    if !root.is_absolute() {
        bail!("Sagent 根目录必须是绝对路径");
    }

    let mut profiles = vec![ProfileName("default".to_owned())];
    let profiles_dir = root.join("profiles");
    if !profiles_dir.exists() {
        return Ok(profiles);
    }

    let entries = fs::read_dir(&profiles_dir)
        .with_context(|| format!("读取 profile 目录失败：{}", profiles_dir.display()))?;
    for entry in entries {
        let entry = entry.context("读取 profile 目录项失败")?;
        if !entry
            .file_type()
            .context("读取 profile 目录项类型失败")?
            .is_dir()
        {
            continue;
        }

        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(profile) = normalize_profile_name(&name) else {
            continue;
        };
        if profile.as_str() != "default" {
            profiles.push(profile);
        }
    }

    profiles[1..].sort_by(|left, right| left.as_str().cmp(right.as_str()));
    Ok(profiles)
}

/// 规范化 profile 名称并校验合法性。
///
/// 先去除首尾空白并转为小写；空名称非法；`default` 为保留名称直接放行。
/// 其余名称必须满足：长度不超过 64，只能由小写字母、数字组成，
/// `_` 和 `-` 允许出现但不能作为首字符。
///
/// 成功返回规范化后的 `ProfileName`，失败返回错误。
pub fn normalize_profile_name(value: &str) -> Result<ProfileName> {
    // Profile 名称最终作为目录名使用，统一小写可避免跨平台的大小写歧义。
    let normalized = value.trim().to_lowercase();

    if normalized.is_empty() {
        bail!("profile 名称不能为空");
    }

    if normalized == "default" {
        // default 对应根数据目录，而不是 `profiles/default` 子目录。
        return Ok(ProfileName(normalized));
    }

    let valid = normalized.len() <= 64
        && normalized.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || (index > 0 && matches!(ch, '_' | '-'))
        });

    if !valid {
        // 拒绝路径分隔符、空格和非 ASCII 字符，避免目录穿越及跨平台路径行为不一致。
        bail!("profile 名称只能包含小写字母、数字、下划线和连字符，且长度不能超过 64 个字符")
    }

    Ok(ProfileName(normalized))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Profile, list_profile_names, normalize_profile_name};

    #[test]
    fn normalizes_whitespace_and_ascii_case() {
        let profile = normalize_profile_name("  Coder-01  ").expect("名称应合法");

        assert_eq!(profile.as_str(), "coder-01");
    }

    #[test]
    fn accepts_default_case_insensitively() {
        let profile = normalize_profile_name(" DEFAULT ").expect("default 应合法");

        assert_eq!(profile.as_str(), "default");
    }

    #[test]
    fn rejects_empty_and_unsafe_names() {
        for value in [
            "",
            "   ",
            "-coder",
            "_coder",
            "coder/name",
            "coder name",
            "中文",
        ] {
            assert!(normalize_profile_name(value).is_err(), "{value:?} 应被拒绝");
        }
    }

    #[test]
    fn rejects_names_longer_than_sixty_four_bytes() {
        let value = "a".repeat(65);

        assert!(normalize_profile_name(&value).is_err());
    }

    #[test]
    fn lists_default_and_valid_named_profile_directories() {
        let root = std::env::temp_dir().join(format!("sagent-profile-list-{}", std::process::id()));
        let profiles = root.join("profiles");
        fs::create_dir_all(profiles.join("Zebra")).expect("应能创建 profile 目录");
        fs::create_dir_all(profiles.join("coder")).expect("应能创建 profile 目录");
        fs::create_dir_all(profiles.join("-unsafe")).expect("应能创建无效目录");
        fs::write(profiles.join("not-a-profile"), "file").expect("应能创建普通文件");

        let names = list_profile_names(&root).expect("应能列出 profile");

        assert_eq!(
            names.iter().map(|name| name.as_str()).collect::<Vec<_>>(),
            vec!["default", "coder", "zebra"]
        );
        fs::remove_dir_all(root).expect("应能清理 profile 测试目录");
    }

    #[test]
    fn profile_index_snapshots_names_and_paths_and_registers_new_profile() {
        // Arrange：磁盘上已有一个命名 Profile，构造索引后再注册一个刚完成初始化的目录。
        let root =
            std::env::temp_dir().join(format!("sagent-profile-index-{}", std::process::id()));
        let coder_path = root.join("profiles").join("coder");
        let writer_path = root.join("profiles").join("writer");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&coder_path).expect("应能创建 coder profile");

        // Act：索引只扫描一次，成功创建后的条目通过 register 写回同一份 map。
        let mut profile = Profile::from_root(&root).expect("应能创建 Profile 索引");
        fs::create_dir_all(&writer_path).expect("应能创建 writer profile");
        profile
            .register("Writer", writer_path.clone())
            .expect("应能注册新 profile");

        // Assert：default 与两个命名 Profile 均可按名称找到正确路径。
        let mut entries = profile.entries().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.name().as_str().cmp(right.name().as_str()));
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.name().as_str(), entry.path()))
                .collect::<Vec<_>>(),
            vec![
                ("coder", coder_path.as_path()),
                ("default", root.as_path()),
                ("writer", writer_path.as_path()),
            ]
        );
        let active = profile
            .entries()
            .filter(|entry| entry.is_active())
            .map(|entry| entry.name().as_str())
            .collect::<Vec<_>>();
        assert_eq!(active, vec!["default"]);
        profile.select("writer").expect("应能切换活动 profile");
        let active = profile
            .entries()
            .filter(|entry| entry.is_active())
            .map(|entry| entry.name().as_str())
            .collect::<Vec<_>>();
        assert_eq!(active, vec!["writer"]);
        fs::remove_dir_all(root).expect("应能清理 profile 测试目录");
    }

    #[test]
    fn listing_a_new_root_still_returns_default_profile() {
        let root =
            std::env::temp_dir().join(format!("sagent-profile-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");

        let names = list_profile_names(&root).expect("应能列出默认 profile");

        assert_eq!(
            names.iter().map(|name| name.as_str()).collect::<Vec<_>>(),
            vec!["default"]
        );
        fs::remove_dir_all(root).expect("应能清理 profile 测试目录");
    }

    #[test]
    fn active_profile_defaults_to_default_and_can_switch_back() {
        let root =
            std::env::temp_dir().join(format!("sagent-active-profile-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("profiles").join("coder")).expect("应能创建 profile");

        let mut profile = Profile::from_root(&root).expect("应能创建 Profile 索引");
        assert_eq!(
            profile
                .read_active_profile()
                .expect("缺少选择文件时应使用 default")
                .as_str(),
            "default"
        );
        profile.select("coder").expect("应能选择已有 profile");
        let mut profile = Profile::from_root(&root).expect("应能刷新 Profile 索引");
        assert_eq!(
            fs::read_to_string(profile.active_profile_path().expect("应能解析选择文件路径"))
                .expect("应能读取选择文件"),
            "coder\n"
        );
        assert_eq!(
            profile
                .read_active_profile()
                .expect("应能读取当前 profile")
                .as_str(),
            "coder"
        );

        profile.select("default").expect("应能切回 default");
        assert!(
            !profile
                .active_profile_path()
                .expect("应能解析选择文件路径")
                .exists()
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn active_profile_rejects_missing_target_and_invalid_marker() {
        let root =
            std::env::temp_dir().join(format!("sagent-active-invalid-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");

        let missing = normalize_profile_name("missing").expect("名称应合法");
        let mut profile = Profile::from_root(&root).expect("应能创建 Profile 索引");
        assert!(profile.select(missing.as_str()).is_err());

        fs::write(
            profile.active_profile_path().expect("应能解析选择文件路径"),
            "../unsafe",
        )
        .expect("应能写入损坏选择文件");
        assert!(Profile::from_root(&root).is_err());
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }
}
