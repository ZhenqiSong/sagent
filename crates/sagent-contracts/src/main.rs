/// CLI 只负责将契约失败转换为非零退出码；行为实现与 fixture 解释集中在库中，供测试复用。
fn main() -> anyhow::Result<()> {
    sagent_contracts::run_all()
}
