# Releasing Rust Commander

Rust Commander is published as four crates while installing one `rc` executable:

| Publication order | crates.io package | Rust target |
| ---: | --- | --- |
| 1 | `rust-commander-shell` | library `rc_shell` |
| 2 | `rust-commander-core` | library `rc_core` |
| 3 | `rust-commander-ui` | library `rc_ui` |
| 4 | `rust-commander` | binary `rc` |

This order follows the internal dependency graph. A downstream crate cannot pass Cargo's
registry verification until its newly published internal dependency is visible in the
crates.io index.

## Prepare

1. Work from a clean `main` checkout that contains the intended release commit.
2. Set the same version in `[workspace.package]` and every internal dependency requirement.
3. Update `CHANGELOG.md` with the version and actual release date.
4. Confirm that each package has a description, repository, readme, license, and an explicit
   crates.io publication policy.
5. Run the complete project checks:

   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
   cargo test --workspace --all-targets --all-features --locked
   ./scripts/validate_rust_advisory_waivers.sh
   cargo deny check bans licenses sources
   ./scripts/run_cargo_deny.sh \
     --manifest-path Cargo.toml --all-features --locked \
     check --config deny.toml advisories
   ./scripts/verify_release_packages.sh
   git diff --check
   ```

The release-package verifier performs a workspace-wide `cargo publish --dry-run`, so Cargo
builds the normalized archives against a temporary local registry in dependency order. It then
checks their metadata, target names, dependency versions, provenance, licenses, README files,
keymap, and complete embedded-skin set.

6. Optionally inspect the file list of any archive before uploading:

   ```bash
   cargo package -p rust-commander-shell --locked --list
   cargo package -p rust-commander-core --locked --list
   cargo package -p rust-commander-ui --locked --list
   cargo package -p rust-commander --locked --list
   ```

The first three packages are public implementation crates because crates.io must resolve the
binary's dependency graph. Their library target names remain `rc_shell`, `rc_core`, and
`rc_ui`, so existing source imports do not change.

## Publish

Authenticate with a scoped crates.io API token using `cargo login`, then publish one crate at
a time. Run the dry run immediately before each upload:

```bash
cargo publish -p rust-commander-shell --locked --dry-run
cargo publish -p rust-commander-shell --locked

cargo publish -p rust-commander-core --locked --dry-run
cargo publish -p rust-commander-core --locked

cargo publish -p rust-commander-ui --locked --dry-run
cargo publish -p rust-commander-ui --locked

cargo publish -p rust-commander --locked --dry-run
cargo publish -p rust-commander --locked
```

Wait for each upload to become visible in the crates.io index before checking or publishing
the next dependent package. Cargo normally polls for this automatically; if it times out, an
upload may still have succeeded, so check crates.io before retrying. Published crate versions
are immutable.

## Verify and tag

Install exactly the uploaded version into a temporary root and confirm the executable identity:

```bash
release_root="$(mktemp -d)"
cargo install rust-commander --version 0.1.0 --locked --root "$release_root"
"$release_root/bin/rc" --version
```

Only after all four crates and the clean installation are verified, create and push the release
tag and publish the corresponding GitHub release:

```bash
git tag -s v0.1.0 -m "Rust Commander 0.1.0"
git push origin main v0.1.0
```

If a serious issue is discovered after publication, yank the affected version and prepare a
new patch version; never reuse an already published version number.
