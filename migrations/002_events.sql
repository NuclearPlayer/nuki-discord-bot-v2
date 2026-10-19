CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    at TEXT NOT NULL,
    kind TEXT NOT NULL,
    run_id TEXT,
    guild_id INTEGER,
    channel_id INTEGER,
    user_id INTEGER,
    data TEXT NOT NULL
);

CREATE INDEX events_kind_at ON events (kind, at);
CREATE INDEX events_run_id ON events (run_id);
CREATE INDEX events_guild_id ON events (guild_id);
CREATE INDEX events_channel_id ON events (channel_id);
CREATE INDEX events_user_id ON events (user_id);
