# Password policy for local accounts

IndexLoom supports a persistent, owner-controlled password policy in
**Settings → Security & users → Password policy**. Choose a preset
(Passphrase / Balanced / Strict), or enable individual rules and save.
Rules live in the main SQLite database and are shared by all browser clients.

## Supported checks

| Control | Configuration |
| --- | --- |
| Minimum length | Enable/disable; 8–128 Unicode characters |
| Maximum length | Enable/disable; 8–128 Unicode characters |
| Uppercase letters | Enable/disable; minimum 1–32, Unicode-aware |
| Lowercase letters | Enable/disable; minimum 1–32, Unicode-aware |
| Digits | Enable/disable; minimum 1–32, Unicode-aware |
| Special characters | Enable/disable; minimum 1–32, non-alphanumeric non-space |
| Whitespace | Block spaces and other whitespace |
| Username | Block including the username, case-insensitively |
| Email | Block the whole email and the local part before @ |
| Common passwords | Block a small built-in list; **not** an online breach database |
| Identical consecutive characters | Enable/disable; maximum run length 2–12 |
| Ascending/descending sequences | Enable/disable; detect contiguous ASCII sequences of length 3–12 |
| Password history | Enable/disable; compare the new password with current and previous 1–10 Argon2id hashes |

## Non-optional guardrails

Even with all optional controls disabled, new passwords must contain
**at least 8 Unicode characters**, at most **128 UTF-8 bytes**, and no
control characters. The default policy retains the former 12-character
minimum while allowing passphrases without compulsory character mixes.

The configured maximum length counts **Unicode characters**, whereas the
absolute 128-byte cap protects the password hashing endpoint. For non-ASCII
passphrases, the byte cap may be reached first.

Security advice: long unique passphrases are often preferable to
mandatory special-character mixtures. Preset names describe configurations,
not guarantees of strength. This version does not force periodic expiration
or perform any third-party breach-password lookups.

## Enforcement and compatibility

All *new* local passwords are validated on the server:
- first administrator bootstrap;
- creation of local accounts by the owner;
- authenticated password changes;
- email-based password resets.

The user interface shows requirements and preliminary live hints, but only
the backend decides whether a password is accepted. Incorrect passwords
are never stored in plaintext.

**Existing passwords remain valid when the policy changes.** OIDC passwords
belong to the identity provider; API bearer tokens and one-time reset links
have independent security controls. Existing accounts are not logged out
just because the password policy changes.

When password-history checks are enabled, a replacement must differ from
the current password and the configured number of previous passwords.
The database retains no more than ten prior salted Argon2id hashes per
account; these hashes are deleted when the account is deleted. Successful
password changes revoke that account's existing local sessions.

## HTTP API

- `GET /api/v1/auth/password-policy` is public and read-only, so the
  bootstrap and reset-password screens can describe active requirements.
- `PUT /api/v1/auth/password-policy/settings` requires owner authorization.
  Submit the complete JSON policy in camelCase. Unknown fields, invalid
  ranges, impossible combinations and malformed payloads are rejected.
- Updating a policy appends a `password-policy-updated` security event.
  Passwords, hashes and session tokens are never returned by these endpoints.

Settings are stored in SQLite migration `0009_password_policy.sql` and
are included in standard database backups. Back up the database and the
encryption master key separately.

## Current limitations

The built-in common-password blocklist is deliberately small and is **not**
equivalent to a breach corpus. Sequence detection targets predictable
ASCII character runs rather than keyboard layouts or dictionary wordlists.
The history check is performed before the password update; simultaneous
admin resets remain subject to the one-time reset-token transaction.
