-- Sagent v1 最小历史结构：没有 rewind_count 列。
CREATE TABLE schema_version (version INTEGER NOT NULL);
INSERT INTO schema_version(version) VALUES (1);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    started_at TEXT NOT NULL
);
