-- 可读取但没有 FTS5 虚拟表的数据库。
CREATE TABLE schema_version (version INTEGER NOT NULL);
INSERT INTO schema_version(version) VALUES (1);
CREATE TABLE messages (
    id INTEGER PRIMARY KEY,
    content TEXT NOT NULL
);
INSERT INTO messages(id, content) VALUES (1, 'plain text only');
