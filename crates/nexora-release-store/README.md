# Nexora Release Store

`nexora-release-store` is the PostgreSQL-backed persistence and composition boundary for the Nexora release worker. PostgreSQL remains authoritative for queued release composition jobs and their evidence.

The store reloads the original session or API-key context at execution time, checks current project publication authority and task visibility, validates pinned task revisions and provider-verified commit links, and revalidates repository ownership before and after pushing a release branch. Git transfer and composition use the bounded operations from `nexora-source`.

`ReleaseStore::compose_one_release` claims one eligible job. A per-release PostgreSQL advisory lock prevents concurrent composers from publishing the same release revision. A crashed worker leaves the job reclaimable after the stale-running interval. The operation is idempotent: an already existing release branch is accepted only when its head equals the deterministic composition; any other existing head causes a conflict and is never overwritten.

The crate is part of the `0.4.0` workspace release. See the repository [README](../../README.md), [security guidance](../../SECURITY.md), and [changelog](../../CHANGELOG.md).
