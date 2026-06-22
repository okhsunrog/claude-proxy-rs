# Changelog

## v2.0.0 - 2026-06-22

### Breaking Changes

- Replaced Turso storage with PostgreSQL via `sqlx`.
- Removed all Turso runtime code and migration helpers.
- Deployment now requires `CLAUDE_PROXY_DATABASE_URL` or `DATABASE_URL` pointing to PostgreSQL.
- Database migrations are now managed by `sqlx` migrations in `migrations/`.

### Migration Notes

- Back up existing data before upgrading.
- Ensure PostgreSQL is available and configured before starting the service.
- Existing deployments should run the included migrations on startup.
- `client_keys.enabled`, `client_keys.allow_extra_usage`, and `models.enabled` are now native PostgreSQL `BOOLEAN` columns.

### Added

- Compile-time checked `sqlx` queries with checked metadata in CI.
- Online SQLx migration/cache validation in CI.
- Stricter Clippy guardrails for production safety.
- PostgreSQL-backed usage history, model limits, client key limits, auth storage, and admin sessions.

### Changed

- Refactored database and usage-history code into smaller modules.
- Refactored admin session handling out of `main.rs`.
- Improved typed database error handling with `thiserror`.
- Shortened fully qualified paths and grouped imports consistently.

### Removed

- Turso dependencies, database code, and migration compatibility paths.
