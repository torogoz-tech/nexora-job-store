# Security

Do not disclose vulnerabilities in public issues. Contact the maintainers privately with the affected repository, version, impact, reproduction, and suggested mitigation when available.

Public releases must be checked for secrets, internal data, private URLs, generated artifacts, and unsafe examples before publication.

Notification workers must access SMTP credentials and action tokens only through the notification store. Keep master-key files owner-only, verify the database key binding before decryption, and preserve the authenticated encryption context when changing settings or token formats. Recipient eligibility is rechecked from current database permissions when a queued notification is claimed.

Runtime integration tests must use disposable databases and a protected connections file supplied through `NEXORA_TEST_CONNECTIONS`. Never commit test credentials or database connection files. The `nexora-runtime` package validates connection configuration and secret-file permissions before opening PostgreSQL connections.
