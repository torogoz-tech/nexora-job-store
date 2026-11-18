# Changelog

## 0.4.0 - Unreleased

- Initial independent repository extraction for Nexora 0.4.0.
- Add a notification persistence package so the notification worker can claim, authorize, render, and finish email deliveries without depending on the API's full platform store.
- Preserve PostgreSQL as the durable source of truth, existing delivery retry limits, SMTP secret format, and encrypted action-token format.
- Add knowledge-worker runtime access to API-managed OpenAI settings and credentials while preserving the existing encrypted-secret envelope and master-key checks.
- Extract deterministic knowledge chunking into `nexora-knowledge-store` so knowledge processing does not need the analysis retrieval package.
