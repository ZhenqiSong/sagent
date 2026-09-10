//! Store 会话生命周期、上下文分支与回退恢复契约。

use super::*;

#[test]
fn manages_session_lifecycle_and_list_visibility() {
    let path = test_path("lifecycle");
    remove_if_exists(&path);
    let session_id = SessionId::new("lifecycle-session");
    let mut store = Store::open_readwrite(&path).expect("应能创建数据库");
    store
        .create_session(&NewSession {
            id: session_id.clone(),
            source: Some("cli".to_owned()),
            model: None,
            title: None,
            started_at: "2026-08-30T10:00:00Z".to_owned(),
        })
        .expect("应能创建会话");

    assert!(
        store
            .update_session_title(&session_id, Some("重命名后的会话"), "2026-08-30T10:01:00Z",)
            .expect("应能更新标题")
    );
    assert!(
        store
            .finish_session(&session_id, "completed", "2026-08-30T10:02:00Z")
            .expect("应能结束会话")
    );
    assert!(
        store
            .set_session_archived(&session_id, true, "2026-08-30T10:03:00Z")
            .expect("应能归档会话")
    );

    assert!(
        store
            .list_sessions(20, 0)
            .expect("应能读取会话列表")
            .is_empty(),
        "归档会话不应显示在普通列表"
    );
    let session = store
        .get_session(&session_id)
        .expect("应能精确读取归档会话")
        .expect("归档会话应保留");
    assert_eq!(session.title.as_deref(), Some("重命名后的会话"));
    assert_eq!(session.ended_at.as_deref(), Some("2026-08-30T10:02:00Z"));
    assert_eq!(session.end_reason.as_deref(), Some("completed"));

    assert!(
        store
            .set_session_archived(&session_id, false, "2026-08-30T10:04:00Z")
            .expect("应能取消归档")
    );
    assert!(
        store
            .set_session_hidden(&session_id, true, "2026-08-30T10:05:00Z")
            .expect("应能隐藏会话")
    );
    assert!(
        store
            .list_sessions(20, 0)
            .expect("应能读取会话列表")
            .is_empty(),
        "隐藏会话不应显示在普通列表"
    );
    assert!(
        store
            .set_session_hidden(&session_id, false, "2026-08-30T10:06:00Z")
            .expect("应能取消隐藏")
    );
    assert_eq!(
        store.list_sessions(20, 0).expect("应能读取会话列表").len(),
        1
    );

    assert!(
        !store
            .update_session_title(
                &SessionId::new("missing-session"),
                Some("不会写入"),
                "2026-08-30T10:07:00Z",
            )
            .expect("未知会话不应导致 SQL 错误")
    );
    assert!(
        store
            .finish_session(&session_id, "", "2026-08-30T10:07:00Z")
            .is_err(),
        "结束原因不能为空"
    );
    remove_if_exists(&path);
}

#[test]
fn rewinds_a_user_turn_and_preserves_auditable_history() {
    let path = test_path("rewind");
    remove_if_exists(&path);
    let session_id = SessionId::new("rewind-session");
    let mut store = Store::open_readwrite(&path).expect("应能创建数据库");
    store
        .create_session(&NewSession {
            id: session_id.clone(),
            source: None,
            model: None,
            title: None,
            started_at: "2026-08-30T10:00:00Z".to_owned(),
        })
        .expect("应能创建会话");
    for (role, content, timestamp) in [
        ("user", "第一条提问", "2026-08-30T10:01:00Z"),
        ("assistant", "第一条回答", "2026-08-30T10:02:00Z"),
        ("user", "第二条 second 提问", "2026-08-30T10:03:00Z"),
        ("assistant", "第二条 second 回答", "2026-08-30T10:04:00Z"),
    ] {
        store
            .append_message(&NewMessage::new(
                session_id.clone(),
                role,
                content,
                timestamp,
            ))
            .expect("应能追加测试消息");
    }

    assert!(
        store
            .rewind_to_message(&session_id, MessageId::new(2), "2026-08-30T10:05:00Z")
            .is_err(),
        "assistant 消息不能作为回退目标"
    );
    let result = store
        .rewind_to_message(&session_id, MessageId::new(3), "2026-08-30T10:05:00Z")
        .expect("应能回退用户消息");
    assert_eq!(result.rewound_count, 2);
    assert_eq!(result.target_message.content, "第二条 second 提问");
    assert_eq!(result.new_head_id.as_ref().map(MessageId::get), Some(2));
    let active = store
        .get_messages(&session_id, &MessageQuery::default())
        .expect("应能读取活动消息");
    assert_eq!(
        active
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    let all_messages = store
        .get_messages(
            &session_id,
            &MessageQuery {
                include_inactive: true,
                ..MessageQuery::default()
            },
        )
        .expect("审计模式应能读取回退历史");
    assert_eq!(
        all_messages
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert!(
        store
            .search_messages(&MessageSearchQuery::new("second"))
            .expect("默认搜索应成功")
            .is_empty(),
        "默认搜索不能显示普通回退历史"
    );
    let mut audit_search = MessageSearchQuery::new("second");
    audit_search.include_inactive = true;
    assert_eq!(
        store
            .search_messages(&audit_search)
            .expect("审计搜索应成功")
            .len(),
        2
    );
    assert_eq!(
        store
            .get_session(&session_id)
            .expect("应能读取会话")
            .expect("会话应存在")
            .message_count,
        2
    );
    let rewind_count: i64 = store
        .connection
        .query_row(
            "SELECT rewind_count FROM sessions WHERE id = ?1",
            [session_id.as_str()],
            |row| row.get(0),
        )
        .expect("应能读取回退次数");
    assert_eq!(rewind_count, 1);

    assert_eq!(
        store
            .restore_rewound_from(&session_id, MessageId::new(3), "2026-08-30T10:06:00Z")
            .expect("应能恢复回退消息"),
        RestoreResult {
            restored_count: 2,
            new_head_id: Some(MessageId::new(4)),
        }
    );
    assert_eq!(
        store
            .get_messages(&session_id, &MessageQuery::default())
            .expect("应能读取恢复后的活动消息")
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(
        store
            .get_session(&session_id)
            .expect("应能读取会话")
            .expect("会话应存在")
            .message_count,
        4
    );
    assert_eq!(
        store
            .search_messages(&MessageSearchQuery::new("second"))
            .expect("恢复后默认搜索应成功")
            .len(),
        2
    );

    store
        .rewind_to_message(&session_id, MessageId::new(3), "2026-08-30T10:07:00Z")
        .expect("应能再次回退用户消息");
    store
        .append_message(&NewMessage::new(
            session_id.clone(),
            "user",
            "新的分支消息",
            "2026-08-30T10:08:00Z",
        ))
        .expect("应能追加新分支消息");
    let error = store
        .restore_rewound_from(&session_id, MessageId::new(3), "2026-08-30T10:09:00Z")
        .expect_err("新分支存在时必须拒绝恢复旧分支");
    assert!(error.to_string().contains("新的活动消息"));
    assert_eq!(
        store
            .get_messages(&session_id, &MessageQuery::default())
            .expect("拒绝恢复后仍应能读取活动消息")
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![1, 2, 5]
    );
    remove_if_exists(&path);
}

#[test]
fn replaces_active_messages_without_losing_auditable_history() {
    let path = test_path("replace-active");
    remove_if_exists(&path);
    let session_id = SessionId::new("replace-session");
    let mut store = Store::open_readwrite(&path).expect("应能创建数据库");
    store
        .create_session(&NewSession {
            id: session_id.clone(),
            source: None,
            model: None,
            title: None,
            started_at: "2026-08-30T10:00:00Z".to_owned(),
        })
        .expect("应能创建会话");
    for (role, content, timestamp) in [
        ("user", "oldbranch question", "2026-08-30T10:01:00Z"),
        ("assistant", "oldbranch answer", "2026-08-30T10:02:00Z"),
    ] {
        store
            .append_message(&NewMessage::new(
                session_id.clone(),
                role,
                content,
                timestamp,
            ))
            .expect("应能追加原活动消息");
    }

    let replacements = [
        NewMessage::new(
            session_id.clone(),
            "user",
            "newbranch question",
            "2026-08-30T10:03:00Z",
        ),
        NewMessage::new(
            session_id.clone(),
            "assistant",
            "newbranch answer",
            "2026-08-30T10:04:00Z",
        ),
    ];
    let inserted_ids = store
        .replace_active_messages(&session_id, &replacements, "2026-08-30T10:05:00Z")
        .expect("应能替换活动消息");
    assert_eq!(
        inserted_ids.iter().map(MessageId::get).collect::<Vec<_>>(),
        vec![3, 4]
    );
    assert_eq!(
        store
            .get_messages(&session_id, &MessageQuery::default())
            .expect("应能读取新活动消息")
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![3, 4]
    );
    assert_eq!(
        store
            .get_messages(
                &session_id,
                &MessageQuery {
                    include_inactive: true,
                    ..MessageQuery::default()
                },
            )
            .expect("审计模式应能读取所有分支")
            .len(),
        4
    );
    assert!(
        store
            .search_messages(&MessageSearchQuery::new("oldbranch"))
            .expect("默认搜索应成功")
            .is_empty()
    );
    let mut audit_search = MessageSearchQuery::new("oldbranch");
    audit_search.include_inactive = true;
    assert_eq!(
        store
            .search_messages(&audit_search)
            .expect("审计搜索应能找到旧分支")
            .len(),
        2
    );
    assert_eq!(
        store
            .get_session(&session_id)
            .expect("应能读取会话")
            .expect("会话应存在")
            .message_count,
        2
    );

    let invalid_replacements = [NewMessage::new(
        SessionId::new("other-session"),
        "user",
        "错误的会话消息",
        "2026-08-30T10:06:00Z",
    )];
    assert!(
        store
            .replace_active_messages(&session_id, &invalid_replacements, "2026-08-30T10:06:00Z",)
            .is_err(),
        "跨会话替换必须在写入前失败"
    );
    assert_eq!(
        store
            .get_messages(&session_id, &MessageQuery::default())
            .expect("失败后活动消息不应改变")
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![3, 4]
    );
    remove_if_exists(&path);
}

#[test]
fn retries_only_the_latest_assistant_message_with_a_checkpoint() {
    let path = test_path("retry");
    remove_if_exists(&path);
    let session_id = SessionId::new("retry-session");
    let mut store = Store::open_readwrite(&path).expect("应能创建数据库");
    store
        .create_session(&NewSession {
            id: session_id.clone(),
            source: None,
            model: None,
            title: None,
            started_at: "2026-08-30T10:00:00Z".to_owned(),
        })
        .expect("应能创建会话");
    store
        .append_message(&NewMessage::new(
            session_id.clone(),
            "user",
            "retry question",
            "2026-08-30T10:01:00Z",
        ))
        .expect("应能写入用户消息");
    store
        .append_message(&NewMessage::new(
            session_id.clone(),
            "assistant",
            "oldretry answer",
            "2026-08-30T10:02:00Z",
        ))
        .expect("应能写入旧回答");

    assert!(
        store.prepare_retry(&session_id, MessageId::new(1)).is_err(),
        "用户消息不能重试"
    );
    let checkpoint = store
        .prepare_retry(&session_id, MessageId::new(2))
        .expect("最新 assistant 消息应可重试");
    assert_eq!(checkpoint.expected_active_head_id.get(), 2);
    assert_eq!(
        store
            .apply_retry(
                &checkpoint,
                &NewMessage::new(
                    session_id.clone(),
                    "assistant",
                    "newretry answer",
                    "2026-08-30T10:03:00Z",
                ),
                "2026-08-30T10:03:00Z",
            )
            .expect("应能写入重试回答")
            .get(),
        3
    );
    assert_eq!(
        store
            .get_messages(&session_id, &MessageQuery::default())
            .expect("应能读取活动分支")
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert!(
        store
            .search_messages(&MessageSearchQuery::new("oldretry"))
            .expect("默认搜索应成功")
            .is_empty()
    );
    let mut audit_search = MessageSearchQuery::new("oldretry");
    audit_search.include_inactive = true;
    assert_eq!(
        store
            .search_messages(&audit_search)
            .expect("审计搜索应找到旧回答")
            .len(),
        1
    );

    let stale_checkpoint = store
        .prepare_retry(&session_id, MessageId::new(3))
        .expect("新回答仍是最新 assistant 消息");
    store
        .append_message(&NewMessage::new(
            session_id.clone(),
            "user",
            "concurrent follow-up",
            "2026-08-30T10:04:00Z",
        ))
        .expect("应能模拟并发的新消息");
    assert!(
        store
            .apply_retry(
                &stale_checkpoint,
                &NewMessage::new(
                    session_id.clone(),
                    "assistant",
                    "must not persist",
                    "2026-08-30T10:05:00Z",
                ),
                "2026-08-30T10:05:00Z",
            )
            .is_err(),
        "检查点失效后必须拒绝重试"
    );
    assert_eq!(
        store
            .get_messages(&session_id, &MessageQuery::default())
            .expect("失败后活动分支不应改变")
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![1, 3, 4]
    );
    remove_if_exists(&path);
}

#[test]
fn archives_compacted_history_but_keeps_it_searchable() {
    let path = test_path("compact");
    remove_if_exists(&path);
    let session_id = SessionId::new("compact-session");
    let mut store = Store::open_readwrite(&path).expect("应能创建数据库");
    store
        .create_session(&NewSession {
            id: session_id.clone(),
            source: None,
            model: None,
            title: None,
            started_at: "2026-08-30T10:00:00Z".to_owned(),
        })
        .expect("应能创建会话");
    for (role, content, timestamp) in [
        ("user", "archiveknowledge question", "2026-08-30T10:01:00Z"),
        (
            "assistant",
            "archiveknowledge answer",
            "2026-08-30T10:02:00Z",
        ),
    ] {
        store
            .append_message(&NewMessage::new(
                session_id.clone(),
                role,
                content,
                timestamp,
            ))
            .expect("应能写入压缩前消息");
    }

    let compacted_messages = [NewMessage::compressed_summary(
        session_id.clone(),
        "assistant",
        "历史摘要：已讨论 archiveknowledge。",
        "2026-08-30T10:03:00Z",
    )];
    assert_eq!(
        store
            .archive_and_compact(&session_id, &compacted_messages, "2026-08-30T10:03:00Z")
            .expect("应能压缩活动上下文")
            .iter()
            .map(MessageId::get)
            .collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(
        store
            .get_messages_for_model(&session_id, &MessageQuery::default())
            .expect("默认读取应只返回压缩后的上下文")
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(
        store
            .get_messages_for_model(&session_id, &MessageQuery::default())
            .expect("压缩摘要应保留给模型")
            .first()
            .and_then(|message| message.display_kind.as_deref()),
        Some("hidden")
    );
    assert_eq!(
        store
            .get_messages_for_display(&session_id, &MessageQuery::default())
            .expect("压缩展示读取应保留历史")
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        store
            .search_messages(&MessageSearchQuery::new("archiveknowledge"))
            .expect("默认搜索仍应命中压缩前知识")
            .len(),
        3
    );
    assert_eq!(
        store
            .get_session(&session_id)
            .expect("应能读取会话")
            .expect("会话应存在")
            .message_count,
        1
    );

    assert!(
        store
            .archive_and_compact(&session_id, &[], "2026-08-30T10:04:00Z")
            .is_err(),
        "空压缩结果必须在修改旧消息前被拒绝"
    );
    assert_eq!(
        store
            .get_messages(&session_id, &MessageQuery::default())
            .expect("失败后活动上下文不应改变")
            .len(),
        1
    );
    remove_if_exists(&path);
}
