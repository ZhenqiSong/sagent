use sagent_tools::{ReadFileLimits, ReadFileRequest, ReadFileService, WorkspaceRoot};
use sagent_types::ToolCallId;
use std::fs;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "current_thread")]
async fn fixture_cjk_and_emoji_round_trip_as_utf8() {
    let root_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("read_file");
    let service = ReadFileService::new(WorkspaceRoot::new(root_path).unwrap(), Default::default());
    let result = service
        .read(
            ToolCallId::new(),
            ReadFileRequest::new("chinese.txt", 1, 2000),
            CancellationToken::new(),
        )
        .await;
    assert!(result.ok);
    assert!(result.content.contains("你好，Sagent 🦀"));
    assert!(!result.truncated);
}

#[tokio::test(flavor = "current_thread")]
async fn output_limit_is_bounded_without_store_or_shell_side_effects() {
    let directory = temporary_directory();
    fs::write(
        directory.join("long.txt"),
        "第一行很长很长\n第二行很长很长\n第三行",
    )
    .unwrap();
    let service = ReadFileService::new(
        WorkspaceRoot::new(&directory).unwrap(),
        ReadFileLimits {
            max_output_chars: 18,
            ..Default::default()
        },
    );
    let result = service
        .read(
            ToolCallId::new(),
            ReadFileRequest::new("long.txt", 1, 2000),
            CancellationToken::new(),
        )
        .await;
    assert!(result.ok);
    assert!(result.truncated);
    assert!(result.content.chars().count() <= 18);
    assert!(result.content.contains("输出已截断"));
    cleanup(directory);
}

#[tokio::test(flavor = "current_thread")]
async fn pre_cancelled_read_never_opens_file() {
    let directory = temporary_directory();
    fs::write(directory.join("secret.txt"), "not returned").unwrap();
    let service = ReadFileService::new(WorkspaceRoot::new(&directory).unwrap(), Default::default());
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let result = service
        .read(
            ToolCallId::new(),
            ReadFileRequest::new("secret.txt", 1, 10),
            cancellation,
        )
        .await;
    assert_eq!(result.error_kind.as_deref(), Some("cancelled"));
    assert!(!result.ok);
    cleanup(directory);
}

fn temporary_directory() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sagent-tools-read-file-integration-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn cleanup(path: std::path::PathBuf) {
    let _ = fs::remove_dir_all(path);
}
