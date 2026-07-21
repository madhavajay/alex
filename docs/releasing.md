# Stable release operations

The stable workflow is `.github/workflows/release.yml`. It runs for an exact
`vX.Y.Z` tag or by `workflow_dispatch` with an existing stable tag. Both entry
points execute the same fail-closed dependency graph; dispatch is a resumable
operator entry point, not a reduced or unsigned release mode.

## Required repository configuration

Configure the GitHub environment `stable-windows-release` with required
reviewers before cutting a stable tag. The protected `windows-approval` job
downloads the source-built Windows archive, verifies its checksum and both
binaries, and uploads machine-readable evidence only after an operator approves
that environment. Environment protection is repository state and cannot be
enforced by workflow YAML alone; a repository administrator must verify it in
Settings → Environments.

The workflow also requires the crates.io token, Apple signing/notarization
credentials, Sparkle private key, and Homebrew tap deployment key used by the
existing release workflows. Missing credentials fail the stable run rather than
creating a partial public release.

The Linux runner must provide Docker and permit the repository's pinned
Ubuntu/systemd smoke wrapper to use a privileged container, host cgroups, and
the `/sys/fs/cgroup` mount. That wrapper is the release gate; the workflow does
not fall back to a host-only daemon smoke when those capabilities are absent.

## Publication order

1. Validate the tag and run Rust and Swift tests.
2. Build Linux GNU, Linux musl, Windows, signed macOS CLI, and signed/notarized
   DMG workflow artifacts directly from the tagged source. Nothing is published.
3. Run the installed Linux package under the pinned privileged systemd
   container, the installed app/CLI macOS smoke, and the protected Windows
   approval smoke. Preserve their JSON evidence as workflow artifacts.
4. Generate one `manifest.json` from all packaged assets. Verification requires
   both macOS CLI architectures, GNU Linux, both musl Linux architectures,
   Windows, and the DMG, and checks every size and SHA-256.
5. Create or resume a draft GitHub release and upload the complete asset bundle.
   Re-download and verify the draft before proceeding.
6. Publish crates in dependency order. Existing crate versions are skipped, so
   a partial crates.io run can be resumed safely.
7. Re-download the release assets, verify them against the manifest again, and
   only then promote the draft to the public stable release.
8. Generate the stable appcast from the already-built DMG artifact and update
   the Homebrew formula/cask after promotion.

## Resuming a failed release

Use **Run workflow** with the same tag, or rerun failed jobs from the original
run. Artifact uploads use stable names, draft uploads use `--clobber`, crate
publishing skips versions already visible on crates.io, Homebrew commits are
no-ops when unchanged, and promotion accepts an already-public release only
after the complete asset verification passes.

Never delete and recreate a tag to resume. If the Windows environment is not
protected by required reviewers, or an existing public release is missing an
asset, stop and repair that repository state before continuing.
