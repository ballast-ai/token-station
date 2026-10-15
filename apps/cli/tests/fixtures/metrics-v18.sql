-- Metrics v18 schema from ef710743. Historical migration fixture.

CREATE TABLE IF NOT EXISTS requests (
    id                  INTEGER PRIMARY KEY,
    request_id          TEXT    NOT NULL DEFAULT '',
    agent_id            TEXT,
    running_revision    INTEGER,
    started_at_ms       INTEGER NOT NULL,
    latency_ms          INTEGER NOT NULL,
    protocol            TEXT    NOT NULL,
    requested_model     TEXT    NOT NULL,
    stream              INTEGER NOT NULL,
    status              INTEGER NOT NULL,
    error_code          TEXT,
    attempts            INTEGER NOT NULL,
    -- routing (NULL row-wise when the request failed before a decision)
    upstream            TEXT,
    model               TEXT,
    pool                TEXT,
    tier                TEXT,
    rule_id             TEXT,
    hint_kind           TEXT,
    hint_value          TEXT,
    heuristic_score     INTEGER,
    heuristic_threshold INTEGER,
    fallbacks           INTEGER,
    -- features (isomorphic to router-core's RequestFeatures)
    est_input_tokens    INTEGER,
    message_count       INTEGER,
    tool_count          INTEGER,
    has_images          INTEGER,
    requires_json_schema INTEGER,
    code_block_count    INTEGER,
    requested_max_output_tokens INTEGER,
    hint_count          INTEGER,
    -- usage (the protocol::Usage vocabulary; NULL means unreported, not zero)
    input_tokens        INTEGER,
    output_tokens       INTEGER,
    cache_read_tokens   INTEGER,
    cache_write_tokens  INTEGER,
    reasoning_tokens    INTEGER,
    usage_observation   TEXT,
    usage_semantics     TEXT NOT NULL DEFAULT 'canonical_total_v2'
        CHECK (usage_semantics IN ('provider_reported_v1', 'canonical_total_v2')),
    subscription_billing INTEGER NOT NULL DEFAULT 0 CHECK (subscription_billing IN (0, 1)),
    -- micro-units; NULL when the model has no price (unknown, not zero)
    cost_micros         INTEGER,
    -- actual/estimated/unknown; unknown never carries a numeric cost
    cost_kind           TEXT NOT NULL DEFAULT 'unknown'
        CHECK (cost_kind IN ('actual', 'estimated', 'unknown')),
    -- the price table version cost_micros was computed under (NULL if unpriced)
    price_version       INTEGER,
    request_method      TEXT,
    path_kind           TEXT NOT NULL DEFAULT 'unknown'
        CHECK (path_kind IN (
            'chat_completions', 'responses', 'messages',
            'gemini_generate_content', 'models', 'embeddings', 'admin',
            'unknown_agent_endpoint', 'unknown'
        ))
);
CREATE INDEX IF NOT EXISTS requests_started_at ON requests (started_at_ms);
-- A stable accounting id is unique: writing the same request twice (a derived
-- table rebuild) is idempotent. Empty ids (legacy rows) are exempt.
CREATE UNIQUE INDEX IF NOT EXISTS requests_request_id
    ON requests (request_id) WHERE request_id <> '';

CREATE TABLE IF NOT EXISTS classifier_inputs (
    request_id TEXT PRIMARY KEY,
    classifier TEXT NOT NULL CHECK (classifier IN ('scx', 'jev')),
    handling TEXT NOT NULL CHECK (handling IN ('full', 'reduced', 'skipped')),
    reason TEXT CHECK (reason IS NULL OR reason IN ('byte_limit', 'token_limit'))
);

CREATE TABLE IF NOT EXISTS decisions (
    request_id TEXT PRIMARY KEY,
    upstream TEXT NOT NULL,
    model TEXT NOT NULL,
    pool TEXT NOT NULL,
    decision_kind TEXT NOT NULL
        CHECK (decision_kind IN ('rule', 'hint', 'classifier', 'heuristic', 'default', 'exact_model', 'quota')),
    rule_id TEXT,
    hint_kind TEXT,
    hint_value TEXT,
    heuristic_score INTEGER,
    heuristic_threshold INTEGER,
    fallbacks INTEGER NOT NULL,
    est_input_tokens INTEGER NOT NULL,
    conversation_tokens INTEGER NOT NULL DEFAULT 0,
    task_tokens INTEGER,
    task_code_block_count INTEGER,
    task_message_count INTEGER,
    message_count INTEGER NOT NULL,
    tool_count INTEGER NOT NULL,
    has_images INTEGER NOT NULL,
    requires_json_schema INTEGER NOT NULL,
    code_block_count INTEGER NOT NULL,
    requested_max_output_tokens INTEGER,
    hint_count INTEGER NOT NULL,
    reasoning_marker_count INTEGER NOT NULL,
    technical_term_count INTEGER NOT NULL,
    simple_indicator_count INTEGER NOT NULL,
    code_keyword_count INTEGER NOT NULL,
    math_term_count INTEGER NOT NULL,
    creative_term_count INTEGER NOT NULL,
    multi_step_signal INTEGER NOT NULL,
    question_count INTEGER NOT NULL,
    system_format_hint INTEGER NOT NULL,
    -- quota-first decision snapshot (NULL for tiered routes)
    quota_reset_ms INTEGER,
    quota_remaining_permille INTEGER,
    quota_headroom_permille INTEGER,
    quota_pressured INTEGER,
    quota_exhausted INTEGER
);

CREATE TABLE IF NOT EXISTS attempts (
    request_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    upstream TEXT NOT NULL,
    model TEXT NOT NULL,
    latency_ms INTEGER NOT NULL,
    http_status INTEGER,
    error_code TEXT,
    stream_outcome TEXT CHECK (
        stream_outcome IS NULL OR stream_outcome IN (
            'complete', 'failed_after_partial', 'failed_before_output', 'client_cancelled'
        )
    ),
    provider_call_engine TEXT NOT NULL DEFAULT 'unknown'
        CHECK (provider_call_engine IN (
            'legacy', 'south_v1_buffered', 'south_v1_streaming', 'native', 'unknown'
        )),
    south_fallback_reason TEXT
        CHECK (south_fallback_reason IS NULL OR south_fallback_reason IN (
            'configured_legacy', 'buffered_mode_cannot_stream', 'unauthenticated_upstream',
            'no_provider_runtime', 'credential_resolver', 'provider_dialect',
            'provider_package_unapproved', 'api_dialect', 'egress', 'streaming', 'method',
            'auth', 'body', 'secret_source', 'response_metadata', 'headers'
        )),
    fallback_allowed INTEGER NOT NULL,
    PRIMARY KEY (request_id, ordinal)
);

CREATE TABLE IF NOT EXISTS conversion_reports (
    request_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    stage TEXT NOT NULL CHECK (stage IN (
        'inbound_normalize', 'provider_request', 'provider_response',
        'outbound_render', 'stream_translate'
    )),
    source_protocol TEXT NOT NULL,
    target_protocol TEXT NOT NULL,
    succeeded INTEGER NOT NULL,
    error_code TEXT,
    outcome TEXT NOT NULL DEFAULT 'unknown'
        CHECK (outcome IN ('succeeded', 'failed', 'cancelled', 'unknown')),
    reason_code TEXT CHECK (reason_code IS NULL OR reason_code IN (
        'unsupported_tool_type', 'provider_tool_unsupported',
        'stateful_chaining', 'structured_output', 'reasoning_item',
        'unsupported_media', 'invalid_json', 'invalid_protocol_shape',
        'adapter_failure'
    )),
    reason_detail TEXT CHECK (reason_detail IS NULL OR reason_detail IN (
        'local_shell', 'web_search', 'function_tool',
        'previous_response_id', 'json_schema', 'reasoning', 'image',
        'request_body', 'other_tool_type'
    )),
    PRIMARY KEY (request_id, ordinal)
);
PRAGMA user_version = 18;
