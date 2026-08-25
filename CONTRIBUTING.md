# Contributing

IndexLoom is in early development. Please discuss substantial protocol,
database or definition-schema changes in an issue before implementation.

## Development requirements

- Docker with the Compose plugin.
- Git configured to sign off commits.

## Workflow

1. Create a focused branch from `main`.
2. Use Conventional Commits, for example `feat(engine): add selector filter`.
3. Add or update tests for behavioral changes.
4. Run the same checks as CI.
5. Submit a pull request describing compatibility and security implications.

All commits must include a Developer Certificate of Origin sign-off:

```bash
git commit --signoff
```

By contributing, you agree that your contribution is licensed under
AGPL-3.0-or-later.
