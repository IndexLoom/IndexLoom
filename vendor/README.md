# Temporary security dependency patches

These directories contain the actual crates.io sources for `sqlx-sqlite
0.9.0` and `openidconnect 4.1.0`, copied from Cargo's registry. Original
licenses and source tests are retained. Registry metadata and nested
lockfiles are omitted; the workspace `Cargo.lock` controls resolution.
`Cargo.toml.orig` records the original upstream manifest; Cargo builds from
the patched, normalized `Cargo.toml`.

## sqlx-sqlite

The only upstream file change is the `libsqlite3-sys` range in `Cargo.toml`,
from `>=0.30.1, <0.38.0` to `>=0.39.1, <0.40.0`. The workspace locks the
actual patched `libsqlite3-sys 0.39.1` (RUSTSEC-2026-0459 /
CVE-2026-119375). No SQLite code is renamed or copied into this patch.

Remove this patch when a released SQLx version accepts a fixed SQLite
library. Track [sqlx#4379](https://github.com/launchbadge/sqlx/issues/4379)
and [sqlx#4402](https://github.com/launchbadge/sqlx/issues/4402). Re-run the
storage migration, quota, and durable tracker-session tests on updates.

## openidconnect

The manifest requires real `rsa 0.10.0` with its `pkcs1v15` feature instead
of vulnerable RSA 0.9 (RUSTSEC-2023-0071). Changes are confined to that
dependency, `src/core/crypto.rs`, `src/core/jwk/mod.rs`, and the upstream
test's rand error type import in `src/core/jwk/tests.rs`.

RSA uses the new `PublicKey`/`PrivateKey`, `BoxedUint`, generic PSS, and
RSA-exported SHA-2 APIs. JWK integers accept leading zeros on input and
are canonical unsigned big-endian integers on output. An adapter forwards
the existing OS-backed rand 0.8 RNG's fallible operations to rand_core 0.10;
entropy errors propagate rather than producing a signature. HMAC and
non-RSA hashing retain their original SHA-2 API. All six RSA JWS algorithms
remain implemented; provider key constraints and OIDC claim checks are
unchanged.

Server regression tests cover independent upstream RSA signature vectors,
signing, JWK round trips and leading zeros, tampered messages/signatures,
incompatible JWK metadata, and signed RS256 tokens with invalid issuer,
audience or nonce. The PEM fixture is a public upstream test key.

Remove this patch once a released openidconnect version uses a patched
RSA dependency, after re-running these tests. Do not restore RSA 0.9 or
introduce advisory exceptions to make an audit pass.
