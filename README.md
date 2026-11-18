# Nexora Job Store

Shared PostgreSQL persistence packages for durable Nexora worker jobs. This workspace separates analysis, release, knowledge, and notification operations into independently selectable Cargo packages. `nexora-runtime` owns snapshot, graph, query, cache, and knowledge runtime adapters. `nexora-analysis-store` owns provider-backed repository analysis access and bounded Git source plans. `nexora-release-store` owns authorized release composition jobs, provider branch verification, and durable composition evidence. `nexora-knowledge-store` owns knowledge-job persistence, API-compatible OpenAI runtime settings, encrypted credential reads, and deterministic source chunking. `nexora-notification-store` owns SMTP notification claiming, retry state, authorization checks, email rendering, and decryption of installation-managed SMTP and action-token secrets.

`nexora-git-policy` owns the shared Git network policy used by repository analysis and task transitions. `nexora-task-workflow` provides PostgreSQL-backed task transitions, durable AI review and contributor-rating jobs, claim fencing, retry state, and worker recovery operations. The API owns schema creation and migrations and records user-requested transitions; the knowledge worker processes AI jobs, and the release worker resumes waiting transitions. The package connects to the existing platform schema and accepts the same Git policy JSON as `nexora-analysis-store`.

PostgreSQL is authoritative for job state. RabbitMQ wake-up messages only notify workers to drain eligible work. Preserve existing lease fencing, retry limits, cancellation rules, project/tenant scope, recipient authorization, secret-encryption formats, and terminal states while extracting the stores.

All packages in this repository are released together at version `0.4.0`.

## Repository status

The `0.4.0` extraction is in progress on `develop`.

## Development

Use the Rust toolchain declared by the workspace. Run `cargo fmt --all -- --check`, `cargo check --workspace --locked`, `cargo test --workspace --locked`, and `cargo clippy --workspace --all-targets --locked -- -D warnings` before submitting changes. PostgreSQL integration tests require the documented disposable test database.

See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), and [CHANGELOG.md](CHANGELOG.md).

The ignored runtime integration tests require the isolated PostgreSQL, Dragonfly, and Neo4j services described by each test. Set `NEXORA_TEST_CONNECTIONS` to a protected database-connections JSON file and `NEXORA_TEST_LANGUAGE_PROFILE` to a JavaScript profile checkout before running them with `cargo test -p nexora-runtime -- --ignored`. The connections file must point to disposable test services.
