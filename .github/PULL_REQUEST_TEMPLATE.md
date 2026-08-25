## Summary

<!-- What changes and why? -->

## Validation

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --all-targets --locked`
- [ ] `pnpm --dir web typecheck`
- [ ] `pnpm --dir web build`

## Definition and provenance check

- [ ] This change does not copy third-party application source or indexer definitions.
- [ ] Any externally sourced behavior is documented in `THIRD_PARTY.md`.
