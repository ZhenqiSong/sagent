-- 含中文与 emoji 的最小 FTS5 fixture。
CREATE TABLE messages (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL,
    content TEXT NOT NULL,
    active INTEGER NOT NULL,
    compacted INTEGER NOT NULL
);
CREATE VIRTUAL TABLE messages_fts USING fts5(content);
INSERT INTO messages VALUES (1, 'unicode-session', '中文消息与 emoji 🚀', 1, 0);
INSERT INTO messages_fts(rowid, content) VALUES (1, '中文消息与 emoji 🚀');
