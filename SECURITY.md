# Security policy

## Supported versions

Security fixes are provided for the newest `0.1.0-alpha.*` prerelease. Older
alphas and untagged development snapshots are unsupported.

## Report a vulnerability

Use [GitHub private vulnerability reporting](https://github.com/fieldsofland/stackmap/security/advisories/new).
Do not open a public issue or attach credentials, private branch names, Git
configuration, repository contents, or Graphite databases. Include the
Stackmap version, macOS version and architecture, terminal, Git version, and a
minimal sanitized reproduction when possible.

The maintainer will acknowledge a report within seven days, coordinate a fix
and disclosure when confirmed, and publish an advisory for affected releases.
Private vulnerability reporting must be enabled before the first public alpha.

Dependency advisories, licenses, duplicate versions, and sources are checked by
`cargo-deny` on the default branch, relevant pull requests, a weekly schedule,
and before release. Any temporary exception must identify its scope, owner,
reason, and removal condition in `deny.toml`.
