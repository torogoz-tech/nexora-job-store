# Task 3 report: continuation authority and durable transition state

## Scope delivered

- Added package-private credential reload matching the API's original session/API-key lookup behavior. It does not touch session activity, checks the current actor/grants/key scope, and compares the reloaded actor with the actor persisted on the run.
- Added task/project scope reads with live credential, capability, project, task visibility, relation, coordination-role, and projectless-assignee checks against the API-owned schema.
- Added provider authority reads for live project, actor, Git repository binding, connection owner/delegation, source generation, credential/connection revisions, and locked development-policy revision.
- Added durable run save and advanced-checkpoint updates using the existing SQL shape. Added the existing failure classification, status/message mapping, task history sequence, task history row, and matching audit event in one transaction.
- Added continuation selection for `ready`, stale `waiting_external`, and stale `running` rows. It scans at most 64 candidates, attempts the engine's exact session advisory lock (`task-transition:{task_id}`), rereads each acquired run by ID/task/status and its original `updated_at`, and releases the lock if the row changed or became ineligible. A lock held by another pod is skipped; if no candidate lock can be acquired, selection returns empty rather than spinning.
- Added a `request_started` provider-intent predicate. Task 4 must reconcile such persisted intent through the provider before replaying any external mutation.
- No worker-facing resume operation is exposed. SQL/schema migrations and API source were not modified.

## Tests and checks

Exact commands and outcomes:

- `cargo fmt --all -- --check` — passed.
- `cargo test -p nexora-task-workflow --locked` — passed: 14 tests passed (8 unit, 2 namespace, 4 Git policy); 1 PostgreSQL integration test ignored; 0 failed.
- `cargo check -p nexora-task-workflow --locked` — passed.
- `cargo clippy -p nexora-task-workflow --locked --all-targets -- -D warnings` — passed.
- `cargo tree -e normal -p nexora-task-workflow --locked` exclusion check — passed; no `nexora-platform-store`, `nexora-indexer`, `nexora-runtime`, `nexora-analysis-store`, `nexora-reasoning`, or `nexora-retrieval` in the normal dependency tree.
- `git diff --check` — passed.

The ignored integration test, `task_transition_session_lock_serializes_worker_pods`, requires a separately provisioned PostgreSQL database with API migration 0088 applied. It was not run; no PostgreSQL behavior is claimed as verified in this environment.

## Files

- `crates/nexora-task-workflow/Cargo.toml`, `Cargo.lock`
- `crates/nexora-task-workflow/src/lib.rs`
- `crates/nexora-task-workflow/src/task_workflow_store.rs`
- `crates/nexora-task-workflow/src/task_transition_{audit,authority,continuation,provider_authority,run,scope}.rs`
- `crates/nexora-task-workflow/src/task_workflow_secret_id.rs`
- `crates/nexora-task-workflow/tests/task_transition_continuation.rs`

The pre-existing untracked `docs/` directory at the repository root was left untouched. The approved plan, spec, and progress ledger were not changed.

## Review-fix round 1 (`199a9fa`)

- Changed provider authority lock order to read the repository binding without a lock, lock the Git connection with `FOR SHARE`, then re-read and lock the repository with `FOR SHARE`. The locked binding must preserve the observed connection, Git source kind, and repository revision. Existing metadata, connection, credential, and source-generation revision checks remain in place.
- Replaced the single 64-row continuation scan with 64-row keyset pages ordered by `(updated_at,id)`. The selector advances past rows whose task advisory lock belongs to another pod, retains the same advisory lock, rereads status and `updated_at`, and releases a stale lock. It holds only one bounded page at a time and does not spin on a busy task.
- Projectless transition failure now revalidates the current assignee and eligible role under `FOR SHARE OF t,r` in the same transaction before it updates the run or writes history/audit. The check is shared with initial projectless transition authorization.
- Moved `PendingTaskTransition` and its provider-intent tests to the focused `task_transition_pending.rs` file.
- Added ignored PostgreSQL tests for scanning beyond 64 advisory-locked candidates and rejecting a projectless task after reassignment. Both require the isolated PostgreSQL configuration and migration 0088; they were not run here.

Review-fix verification:

- `cargo fmt --all -- --check` — passed.
- `cargo test -p nexora-task-workflow --locked` — passed: 14 passed, 3 ignored (the existing session-lock integration plus the two new PostgreSQL tests), 0 failed.
- `cargo check -p nexora-task-workflow --locked` — passed.
- `cargo clippy -p nexora-task-workflow --locked --all-targets -- -D warnings` — passed.
- `cargo tree -e normal -p nexora-task-workflow --locked` — inspected; the normal dependency graph remains limited to platform, database connection, Git policy, PostgreSQL, and serialization crates, with no API store, analysis store, indexer, or runtime.
- `git diff --check` — passed.
