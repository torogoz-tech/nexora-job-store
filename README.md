# Nexora Job Store

Shared PostgreSQL persistence packages for durable Nexora worker jobs. This workspace separates analysis, release, knowledge, and notification operations into independently selectable Cargo packages. `nexora-knowledge-store` owns knowledge-job persistence, API-compatible OpenAI runtime settings, encrypted credential reads, and deterministic source chunking. `nexora-notification-store` owns SMTP notification claiming, retry state, authorization checks, email rendering, and decryption of installation-managed SMTP and action-token secrets.

PostgreSQL is authoritative for job state. RabbitMQ wake-up messages only notify workers to drain eligible work. Preserve existing lease fencing, retry limits, cancellation rules, project/tenant scope, recipient authorization, secret-encryption formats, and terminal states while extracting the stores.

All packages in this repository are released together at version `0.4.0`.

## Repository status

The `0.4.0` extraction is in progress on `develop`.

## Development

Use the Rust toolchain declared by the workspace. Run `cargo fmt --all -- --check`, `cargo check --workspace --locked`, `cargo test --workspace --locked`, and `cargo clippy --workspace --all-targets --locked -- -D warnings` before submitting changes. PostgreSQL integration tests require the documented disposable test database.

See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), and [CHANGELOG.md](CHANGELOG.md).
