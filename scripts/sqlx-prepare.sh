#!/usr/bin/env bash
# Generates sqlx offline query data (.sqlx/ directory) for CI builds.
#
# Usage: ./scripts/sqlx-prepare.sh
#
# This script:
# 1. Creates a temporary SQLite database
# 2. Runs all migrations against it
# 3. Runs `cargo sqlx prepare` to generate compile-time query metadata
# 4. Cleans up the temporary database
#
# The resulting .sqlx/ directory should be committed to the repository.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
DB_FILE="$PROJECT_DIR/.sqlx-prepare-tmp.db"

cleanup() {
    rm -f "$DB_FILE" "$DB_FILE-journal" "$DB_FILE-wal" "$DB_FILE-shm"
}
trap cleanup EXIT

echo "==> Creating temporary database at $DB_FILE"
export DATABASE_URL="sqlite://$DB_FILE?mode=rwc"

echo "==> Running migrations"
cargo sqlx database create
cargo sqlx migrate run --source "$PROJECT_DIR/migrations"

echo "==> Generating sqlx prepared queries"
cargo sqlx prepare --workspace

echo "==> Done. The .sqlx/ directory has been updated."
echo "    Remember to commit .sqlx/ to the repository."
