CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE intakes (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('bug', 'feature')),
    thread_id INTEGER NOT NULL,
    origin_channel_id INTEGER NOT NULL,
    origin_message_id INTEGER NOT NULL,
    reporter_id INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'reported')),
    last_activity_at TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX intakes_thread_id_status ON intakes (thread_id, status);

CREATE TABLE tool_turns (
    id INTEGER PRIMARY KEY,
    channel_id INTEGER NOT NULL,
    message_id INTEGER NOT NULL UNIQUE,
    messages TEXT NOT NULL,
    created_at TEXT NOT NULL
);
