-- Test table. Also created by the app on startup (CREATE TABLE IF NOT EXISTS).
CREATE TABLE IF NOT EXISTS wfl_test (
    id         BIGSERIAL PRIMARY KEY,
    task_id    INT NOT NULL,
    payload    TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
