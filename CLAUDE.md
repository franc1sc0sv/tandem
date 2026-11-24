# Tandem

A Mac and a Windows PC working as one desk: monitors (DDC/CI), keyboard and mouse with our own engine, power, and our own encrypted link. All in Rust (+ Tauri for the UI).

- **Design (source of truth):** `docs/DESIGN.md`. Read it before any non-trivial change.
- **Research:** `docs/research/` (DDC of the 34", Deskflow as a reference, input engine).
- **Backlog:** `docs/tasks/T-*.md` (protocol below).
- **Terminology:** `docs/GLOSSARY.md`. Use its terms in code, UI, docs and commits.
- **UI mockups (v0.3):** `mockups/v03/*.dc.html` (map in `mockups/README.md`); compared with `pnpm shots` and `pnpm mockups:compare`.

If the code, the task and the design disagree, **stop and report it**; do not improvise architecture.

## Language

**Everything in the repository is in English**: UI text, code, identifiers, file names, comments, tests, docs, tasks, reports, commit messages, PR titles and descriptions, and CI job names. No Spanish anywhere in the repo. Only the conversation with the user is in Spanish.

## Backlog protocol

- Each task: `docs/tasks/T-<id>.md` with frontmatter `status: todo | in-progress | blocked | done`, `depends_on`, `needs`.
- `needs`:
  - `none`: can be done without hardware.
  - `windows-hw` / `mac-hw`: **still implemented**, with complete code, tests with simulated backends and CI on all three platforms. What can only be checked on the real machine goes in a `## Pending hardware verification` section (checkbox list) of the task file; the task counts as `done` even if that list stays pending.
  - `user` (needs a physical action from the user): not taken.
- **Integration branch: `develop`.** Tasks branch off `origin/develop` and their PRs target `develop`. Only the user touches `main`: never commit, push, PR or merge to `main`.
- One task = one branch `task/T-<id>` = one PR to `develop`.
- **Autonomous merge to `develop`** (authorized by the user): `gh pr merge <n> --squash --delete-branch`, only if all three CI jobs are green and the reviewer agent approved. If `develop` moved, rebase on `origin/develop` and `git push --force-with-lease origin task/T-<id>` (only `task/*` branches).
- In your PR, set `status: done` in the task file and check the criteria you met.
- A PR that changes `PROTOCOL_VERSION`, `CAPABILITIES_SINCE` or `SIDES_SINCE` in `tandem-proto` also updates "Protocol versioning" in `docs/DESIGN.md` §4.5; `scripts/ci/protocol.sh` fails otherwise.
- If you cannot meet a non-hardware criterion, set `status: blocked` with the reason under `## Notes`.
- Only edit your own task file, not others'.

## Quality (required before a commit or PR; a hook enforces it)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For Windows code, the `windows-latest` CI is the check. `cargo check --target x86_64-pc-windows-gnu` in WSL only works if `mingw-w64` is installed (crates with C such as `ring`); if it is missing, that is not a blocker.

## Tests (maximize; see `docs/TESTING.md`)

- All new logic gets unit tests. Invariants get property tests (`proptest`).
- All platform code goes behind a trait with a test double, so the logic is tested on Linux.
- Paths between the two machines are tested with two agents in the same process, over loopback (`tandem-sim`).
- A bug found = first a failing test, then the fix.

## Rust conventions

- Edition 2024. `thiserror` in libraries, `anyhow` only in binaries. No `unwrap()`/`expect()` outside tests.
- Platform code under `#[cfg(windows)]` / `#[cfg(target_os = "macos")]`, behind traits defined in portable code; the logic is tested on Linux with simulated implementations.
- Dependencies: declared in `[workspace.dependencies]` of the root `Cargo.toml`.
- `unsafe` only for FFI, in small functions with a `// SAFETY:` comment.
- Comments and docs in English, low density: explain the why, not the what.
- **Copying code from lan-mouse or Deskflow (GPL) is forbidden.** They may be read as a reference.

## Hardware

- Tests that touch devices: `#[ignore = "hardware"]`. Never in CI or routine tasks.
- Windows PC: reached from WSL with `powershell.exe` (interop). Mac: `ssh mac-runner` (see `docs/dev/hardware.md`).
- Any test that changes a monitor's input **must switch back automatically**: the user could lose sight of the PC.

## Git

- Commits are signed automatically with SSH. Do not use `--no-gpg-sign` or touch `commit.gpgsign`.
- Messages in English, imperative mood, one summary line + a body if needed.
- No AI attribution: no `Co-Authored-By: Claude` trailer and no "Generated with Claude Code" line in commits, PRs or comments.
