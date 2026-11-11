CREATE TABLE IF NOT EXISTS sync_accounts (
    id uuid PRIMARY KEY,
    name text NOT NULL,
    token_sha256 text UNIQUE
);
ALTER TABLE sync_accounts ALTER COLUMN token_sha256 DROP NOT NULL;
ALTER TABLE sync_accounts ADD COLUMN IF NOT EXISTS microsoft_issuer text;
ALTER TABLE sync_accounts ADD COLUMN IF NOT EXISTS microsoft_subject text;
ALTER TABLE sync_accounts ADD COLUMN IF NOT EXISTS avatar text NOT NULL DEFAULT 'violet';
ALTER TABLE sync_accounts ADD COLUMN IF NOT EXISTS profile_complete boolean NOT NULL DEFAULT true;
ALTER TABLE sync_accounts ADD COLUMN IF NOT EXISTS access_code_sha256 text;
ALTER TABLE sync_accounts ADD COLUMN IF NOT EXISTS can_invite boolean NOT NULL DEFAULT false;
CREATE UNIQUE INDEX IF NOT EXISTS sync_microsoft_identity
    ON sync_accounts(microsoft_issuer, microsoft_subject);
CREATE UNIQUE INDEX IF NOT EXISTS sync_access_code_digest ON sync_accounts(access_code_sha256);
CREATE UNIQUE INDEX IF NOT EXISTS sync_single_founder ON sync_accounts(can_invite) WHERE can_invite;
CREATE TABLE IF NOT EXISTS sync_auth_limits (
    route text PRIMARY KEY,
    count integer NOT NULL CHECK (count > 0),
    resets_at bigint NOT NULL
);
CREATE TABLE IF NOT EXISTS sync_login_flows (
    state_sha256 text PRIMARY KEY,
    callback_url text NOT NULL,
    desktop_state text NOT NULL,
    desktop_challenge text NOT NULL,
    microsoft_verifier text NOT NULL,
    microsoft_nonce text NOT NULL,
    expires_at bigint NOT NULL
);
CREATE TABLE IF NOT EXISTS sync_login_handoffs (
    code_sha256 text PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES sync_accounts(id) ON DELETE CASCADE,
    desktop_challenge text NOT NULL,
    expires_at bigint NOT NULL
);
CREATE TABLE IF NOT EXISTS sync_sessions (
    token_sha256 text PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES sync_accounts(id) ON DELETE CASCADE,
    expires_at bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS sync_sessions_expiry ON sync_sessions(expires_at);
CREATE TABLE IF NOT EXISTS sync_packs (
    id uuid PRIMARY KEY,
    owner_id uuid NOT NULL REFERENCES sync_accounts(id),
    name text NOT NULL,
    target jsonb NOT NULL,
    server_address text,
    active_revision bigint CHECK (active_revision > 0)
);
CREATE TABLE IF NOT EXISTS sync_members (
    pack_id uuid NOT NULL REFERENCES sync_packs(id),
    account_id uuid NOT NULL REFERENCES sync_accounts(id),
    role text NOT NULL CHECK (role IN ('owner', 'editor', 'player')),
    PRIMARY KEY (pack_id, account_id)
);
CREATE INDEX IF NOT EXISTS sync_members_accounts ON sync_members(account_id);
CREATE TABLE IF NOT EXISTS sync_invites (
    id uuid PRIMARY KEY,
    pack_id uuid NOT NULL REFERENCES sync_packs(id),
    code_sha256 text UNIQUE NOT NULL,
    role text NOT NULL CHECK (role IN ('editor', 'player')),
    expires_at bigint NOT NULL,
    revoked boolean NOT NULL DEFAULT false
);
CREATE TABLE IF NOT EXISTS sync_submissions (
    id uuid PRIMARY KEY,
    pack_id uuid NOT NULL REFERENCES sync_packs(id),
    author_id uuid NOT NULL REFERENCES sync_accounts(id),
    payload jsonb NOT NULL,
    submitted_at bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS sync_submissions_by_pack ON sync_submissions(pack_id, submitted_at DESC);
CREATE TABLE IF NOT EXISTS sync_candidates (
    id uuid PRIMARY KEY,
    pack_id uuid NOT NULL REFERENCES sync_packs(id),
    payload jsonb NOT NULL,
    created_at bigint NOT NULL
);
CREATE TABLE IF NOT EXISTS sync_releases (
    pack_id uuid NOT NULL REFERENCES sync_packs(id),
    revision bigint NOT NULL CHECK (revision > 0),
    candidate_id uuid NOT NULL UNIQUE REFERENCES sync_candidates(id),
    signed jsonb NOT NULL,
    PRIMARY KEY (pack_id, revision)
);
CREATE TABLE IF NOT EXISTS sync_release_artifacts (
    pack_id uuid NOT NULL,
    revision bigint NOT NULL,
    sha256 text NOT NULL,
    PRIMARY KEY (pack_id, revision, sha256),
    FOREIGN KEY (pack_id, revision) REFERENCES sync_releases(pack_id, revision)
);
CREATE INDEX IF NOT EXISTS sync_release_artifacts_lookup ON sync_release_artifacts(pack_id, sha256);
