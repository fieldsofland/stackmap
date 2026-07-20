# Releasing Stackmap

The alpha is a binary-first GitHub prerelease. Crates.io publication is disabled,
and released binaries are unsigned and not notarized. Provenance attestations do
not replace Apple Developer ID signing or notarization.

## Repository prerequisites

- `main` and `v*-alpha.*` tags are protected; release tags must point to `main`.
- CI and Dependency policy are required checks.
- The `prerelease` environment requires maintainer approval.
- GitHub Actions, artifact attestations, Dependabot/security alerts, and private
  vulnerability reporting are enabled.

## Publish checklist

1. Start from a clean `main` checkout. Run the complete commands in
   [CONTRIBUTING.md](../CONTRIBUTING.md) plus `cargo deny check`.
2. Update `Cargo.toml` and `Cargo.lock`; verify help and `--version` agree.
3. Confirm release notes and support/Graphite compatibility claims. Never reuse
   a released version or move its tag.
4. Create and push the matching tag, for example
   `git tag -s v0.1.0-alpha.1 && git push origin v0.1.0-alpha.1`.
5. Review the exact-SHA ARM64/Intel verification, dependency policy, native
   smoke evidence, archive membership, executable modes, CPU types, macOS 15
   deployment targets, versions, and checksums. Approve the protected release
   environment only when all evidence is complete.

The aggregation job refuses missing, duplicate, stale, unexpected, or existing
release assets. It publishes exactly two native archives plus `SHA256SUMS` and
attests all three only after both architectures pass. Matrix workers cannot
mutate Releases. Failed prepublication workflow artifacts expire after seven
days; delete any abandoned private draft before retrying.

## Consumer verification

Download the archive for `arm64` or `x86_64` and `SHA256SUMS` from the same
release. Verify:

```sh
shasum -a 256 -c SHA256SUMS
gh attestation verify stackmap-0.1.0-alpha.1-arm64.tar.gz \
  --repo fieldsofland/stackmap
./stackmap-0.1.0-alpha.1-arm64/stackmap --version
```

On a clean host per architecture, also extract the archive, confirm the binary
is executable, start/quit it in a disposable Git repository, and record observed
quarantine/Gatekeeper behavior. Do not broadly disable macOS protections. Build
from the tagged source if the provisional unsigned binary is unsuitable.

If a released asset is defective, publish the next prerelease. For compromise,
mark the release withdrawn, publish a security advisory, and issue a new version;
never replace assets or reuse the tag.
