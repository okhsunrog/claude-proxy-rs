CREATE TABLE IF NOT EXISTS auth (
    provider TEXT PRIMARY KEY,
    auth_type TEXT NOT NULL,
    access_token TEXT NOT NULL,
    refresh_token TEXT NOT NULL,
    expires_at BIGINT NOT NULL,
    account_id TEXT,
    enterprise_url TEXT
);

CREATE TABLE IF NOT EXISTS client_keys (
    id TEXT PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    enabled BIGINT NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    last_used_at BIGINT,
    five_hour_limit BIGINT,
    weekly_limit BIGINT,
    total_limit BIGINT,
    five_hour_reset_at BIGINT NOT NULL DEFAULT 0,
    weekly_reset_at BIGINT NOT NULL DEFAULT 0,
    five_hour_count_from BIGINT NOT NULL DEFAULT 0,
    weekly_count_from BIGINT NOT NULL DEFAULT 0,
    total_count_from BIGINT NOT NULL DEFAULT 0,
    allow_extra_usage BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS models (
    id TEXT PRIMARY KEY,
    sort_order BIGINT NOT NULL DEFAULT 0,
    enabled BIGINT NOT NULL DEFAULT 1,
    input_price DOUBLE PRECISION NOT NULL DEFAULT 0,
    output_price DOUBLE PRECISION NOT NULL DEFAULT 0,
    cache_read_price DOUBLE PRECISION NOT NULL DEFAULT 0,
    cache_write_price DOUBLE PRECISION NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS key_allowed_models (
    key_id TEXT NOT NULL REFERENCES client_keys(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    PRIMARY KEY (key_id, model)
);

CREATE TABLE IF NOT EXISTS admin_sessions (
    token TEXT PRIMARY KEY,
    expires_at BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS request_log (
    id BIGSERIAL PRIMARY KEY,
    key_id TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    cache_read_tokens BIGINT NOT NULL DEFAULT 0,
    cache_write_tokens BIGINT NOT NULL DEFAULT 0,
    cost_microdollars BIGINT NOT NULL DEFAULT 0,
    created_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_request_log_created_at ON request_log(created_at);
CREATE INDEX IF NOT EXISTS idx_request_log_key_created ON request_log(key_id, created_at);

CREATE TABLE IF NOT EXISTS key_model_limits (
    key_id TEXT NOT NULL REFERENCES client_keys(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    five_hour_limit BIGINT,
    weekly_limit BIGINT,
    total_limit BIGINT,
    count_from BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (key_id, model)
);
