#!/usr/bin/env nu

# dev.nu — Start a local dawnstore-api dev/test session.
#
# Usage:
#   nu dawnstore-api/scripts/dev.nu
#
# What it does:
#   1. Starts an ephemeral PostgreSQL 17 container (port 5436, separate from the
#      test DB on 5435).
#   2. Generates a fresh P-384 JWT keypair via `--generate-keys`.
#   3. Runs the API with DATABASE_URL and JWT keys wired up.
#   4. Stops the container when the API exits (Ctrl-C or otherwise).

let pg_container = "dawnstore-api-dev"
let pg_port      = 5436        # avoids collision with the test DB on 5435
let pg_password  = "devpassword"
let pg_user      = "postgres"
let pg_db        = "postgres"
let database_url = $"postgres://($pg_user):($pg_password)@localhost:($pg_port)/($pg_db)"

# ── PostgreSQL ────────────────────────────────────────────────────────────────

# Remove a leftover container from a previous crashed session, if any.
let existing = (^docker ps -aq --filter $"name=($pg_container)" | str trim)
if ($existing | str length) > 0 {
    print $"Removing leftover container ($pg_container)..."
    ^docker rm -f $pg_container
}

print "Starting PostgreSQL container..."
(^docker run --rm --name $pg_container
    -e $"POSTGRES_PASSWORD=($pg_password)"
    -p $"($pg_port):5432"
    -d postgres:17)

print "Waiting for PostgreSQL to be ready..."
mut ready = false
for _ in 1..30 {
    let result = (do { ^docker exec $pg_container pg_isready -U $pg_user } | complete)
    if $result.exit_code == 0 {
        $ready = true
        break
    }
    sleep 1sec
}

if not $ready {
    print "ERROR: PostgreSQL did not become ready in time."
    ^docker stop $pg_container
    exit 1
}

print "PostgreSQL is ready."

# ── JWT keypair ───────────────────────────────────────────────────────────────

print "Generating JWT keypair..."
let key_lines = (^cargo run -p dawnstore-api -- --generate-keys | lines)

let private_key = (
    $key_lines
    | where { |l| $l | str starts-with "JWT_PRIVATE_KEY_B64=" }
    | first
    | str replace "JWT_PRIVATE_KEY_B64=" ""
)
let public_key = (
    $key_lines
    | where { |l| $l | str starts-with "JWT_PUBLIC_KEY_B64=" }
    | first
    | str replace "JWT_PUBLIC_KEY_B64=" ""
)

print $"Private key length: ($private_key | str length) chars"
print $"Public key length:  ($public_key  | str length) chars"

# ── API ───────────────────────────────────────────────────────────────────────

print ""
print $"DATABASE_URL: ($database_url)"
print "Starting dawnstore-api on :8080  (Ctrl-C to stop)"
print ""

try {
    with-env {
        DATABASE_URL:        $database_url
        JWT_PRIVATE_KEY_B64: $private_key
        JWT_PUBLIC_KEY_B64:  $public_key
    } {
        ^cargo run -p dawnstore-api
    }
} catch {|e|
    # Ctrl-C / non-zero exit — fall through to cleanup.
}

# ── Cleanup ───────────────────────────────────────────────────────────────────

print ""
print "Stopping PostgreSQL container..."
^docker stop $pg_container
