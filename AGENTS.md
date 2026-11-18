# Repository instructions

- Keep each explicit Rust type or language feature in its own focused file.
- Follow SOLID and keep each job domain in its own package.
- Do not introduce traits or interface abstractions unless a caller requires them.
- PostgreSQL remains the source of truth; do not add an outbox table or periodic polling loop.
- Preserve existing schemas and job eligibility, lease, retry, cancellation, tenancy, and terminal-state behavior.
- Use `develop` for changes and pull requests to `main`.
- Keep all package versions aligned with the coordinated release version.
