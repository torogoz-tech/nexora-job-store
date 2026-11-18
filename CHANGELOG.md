# Changelog

## 0.4.0 - Unreleased

- Initial independent repository extraction for Nexora 0.4.0.
- Add a notification persistence package so the notification worker can claim, authorize, render, and finish email deliveries without depending on the API's full platform store.
- Preserve PostgreSQL as the durable source of truth, existing delivery retry limits, SMTP secret format, and encrypted action-token format.
- Add knowledge-worker runtime access to API-managed OpenAI settings and credentials while preserving the existing encrypted-secret envelope and master-key checks.
- Extract deterministic knowledge chunking into `nexora-knowledge-store` so knowledge processing does not need the analysis retrieval package.
- Move the concrete snapshot, graph, query, cache, and knowledge runtime adapters into the independently tested `nexora-runtime` package in this repository.
- Add `nexora-analysis-store` repository transport plans that verify provider identity, connection authorization, bounded origins, and branch commit identities before workers fetch Git data.
- Add `nexora-release-store` so the release worker can authorize, compose, and record release jobs without importing the API's full platform store. Existing release branches are accepted only when their head matches the deterministic composition; a different head is never overwritten.
- Keep the release composition job durable in PostgreSQL, re-check current session or API-key authority and task commit pins, and verify repository authority and provider branch state around publication.
- Replace runtime integration-test references to API checkout paths with explicit protected test-configuration and language-profile environment variables; retain public domain-boundary fixtures in the package.
- Add the `nexora-git-policy` package as the canonical owner of the validated Git policy and preserve the analysis-store public re-export.
- Add the `nexora-task-workflow` package with platform schema validation, shared database connection settings, and the existing Git policy JSON contract.
- Extract durable task transition execution, AI review and private contributor-rating jobs, claim fencing, bounded retry state, and generation-fenced RabbitMQ wake intents into `nexora-task-workflow`.
- Add worker-facing startup recovery and one-shot lease-expiry lookup operations so multiple worker replicas can recover work without periodic task-job polling.
- Pin shared platform contracts to the immutable coordinated `0.4.0-final` contract tag.
- Pin shared analysis-engine crates to the coordinated `0.4.0-final` tag so runtime types resolve from one source revision.
