# Release

The `Release` workflow (`.github/workflows/release.yml`) builds the two installers, the signed
updater packages, the checksums, the notes and `latest.json`, and creates the release
on GitHub (DECISIONS D19, DESIGN §13). A release is always created as a draft with everything uploaded;
a final version is published at the end and a prerelease (`-rc.N`) stays as a draft.

| Trigger | What it does |
|---|---|
| Pushing a `v<version>` tag | Everything; creates that tag's release. `v0.3.0` is published; `v0.3.0-rc.1` stays as a draft prerelease |
| By hand (*Actions → Release → Run workflow*), with "Test mode" ticked (default) | Everything as workflow artifacts, signed with a disposable key; **no release and no tag** |
| By hand with "Test mode" unticked | Like a tag, for `v<version>` at the chosen commit; GitHub creates the tag on publishing |
| PR that touches `release.yml`, `scripts/release/`, `scripts/macos/`, `tauri.updater.json` or `tauri.macos.conf.json` | Always in test mode |

`workflow_dispatch` only appears in *Actions* when the workflow is on the default branch
(`main`); until then, test mode is seen in the PRs that touch it.

## Jobs

1. **prepare** (Ubuntu): runs `scripts/release/test.sh`, decides the version, the mode and whether it is a
   prerelease, and generates `notes.md`. If it is going to create a release and the secret
   `TAURI_SIGNING_PRIVATE_KEY` or the macOS signing identity (`MACOS_SIGNING_CERTIFICATE`,
   `MACOS_SIGNING_CERTIFICATE_PASSWORD`) is missing, it fails here with the reason (before
   compiling anything).
2. **dmg (macos-latest)**: imports the code-signing identity into a temporary keychain
   (`scripts/macos/keychain.sh`: the stable one in a release, a disposable one in test mode) and
   passes its SHA-1 as `APPLE_SIGNING_IDENTITY`; then `scripts/macos/bundle.sh --config
   src-tauri/tauri.updater.json`, same as CI (see [install-macos.md](install-macos.md)) plus the
   updater package: `Tandem_<version>_<arch>.app.tar.gz` and its `.sig` signature. Then
   `scripts/macos/dr-check.sh` checks that a second version signed with the same identity keeps
   the designated requirement, for the app and for `tandemd`, and that the updater package keeps
   it and carries no quarantine attribute (see [macOS signing identity](#macos-signing-identity)).
   Uploads `installer-macos`.
3. **installer (windows)**: `tools/windows/installer/build-installer.ps1` with
   `TANDEM_UPDATER_ARTIFACTS=1` (leaves `Tandem_<version>_x64-setup.exe.sig`; the updater downloads
   the same installer) and the install, reinstall and uninstall test
   (`test-installer.ps1`): an installer that was not tested does not come out (see
   [install-windows.md](install-windows.md)). Uploads `installer-windows`.
4. **checksums and latest.json** (Ubuntu): requires exactly one `.dmg`, one `-setup.exe` and one
   `.app.tar.gz`, writes `SHA256SUMS.txt` and builds `latest.json` with
   `scripts/release/latest-json.sh`. Uploads the artifact **`tandem-<version>`**: what would be
   published.
5. **release**: only outside test mode; the only job with `contents: write`. Creates the
   draft release with everything, and if it is not a prerelease publishes it (`--latest`). If a
   release with that tag already exists, it fails: delete it and run it again.

## Automatic update

The app carries `tauri-plugin-updater` (code in `apps/tandem-ui/src-tauri/src/update.rs`, which is
portable and has the tests, and `updater.rs`, which connects it to Tauri):

- It reads `https://github.com/franc1sc0sv/tandem/releases/latest/download/latest.json`, that is
  the latest **published** release that is not a prerelease. Drafts and `-rc` ones reach
  nobody on their own.
- Each package is signed with the updater's minisign key; the app only installs what it validates
  with the public one (`plugins.updater.pubkey` in `tauri.conf.json`). It is not code signing: that
  is the [macOS signing identity](#macos-signing-identity) (self-signed, no Developer ID) and,
  on Windows, nothing (no Authenticode).
- **What is updated:** each system's package carries the UI and `tandemd`. On macOS the updater
  replaces `Tandem.app` and the app restarts; on startup it sees the LaunchAgent plist with another
  version and relaunches `tandemd` with the new binary (`launch_agent.rs`). The updater writes the
  new bundle itself (download in the process, `tar` unpacked in place): macOS only adds
  `com.apple.quarantine` to what a quarantine-aware app downloads, and Tandem is not one
  (no `LSFileQuarantineEnabled`), so Gatekeeper does not stop the updated app; the release
  checks that the package itself carries no quarantine attribute. Since every version is signed
  with the same identity, macOS keeps Accessibility, Input Monitoring and Local Network (T-348).
  On Windows it runs the NSIS installer in passive mode: its hooks (`windows/hooks.nsh`) stop
  `tandemw` and `tandemd` before copying and start them again at the end. The per-user install
  (`%LOCALAPPDATA%`) asks for nothing. If Tandem is under Program Files (the T-027 service
  install), the installer needs administrator: the app says so before ("This PC asks for
  administrator once…", `UpdateView.admin`), there is one UAC prompt, and the hook's elevated
  `tandemd --install` refreshes the secure desktop service too. The start with the Windows
  session is the `HKCU` `Run` key (T-071), not a scheduled task.
- **"Update available: vX.Y.Z"** (T-348): when a check finds a new version, the sidebar of the
  main window shows it with **one button** that runs "Update all", its progress and, if it
  failed, why (with "Try again"). After the restart the new app shows "Updated to vX.Y.Z" once,
  with where the other machine is: the app remembers the version it last ran as in
  `update.toml` (`seen`), so the new process knows it was updated.
- **"Update all"** (Settings → Updates, or the notice): first the other machine, then this one.
  This one asks the other, through `tandemd` (`UiCommand::UpdatePeer` → `ControlMsg::Update`), to
  install itself; the other's UI downloads its package and restarts. This one waits up to 10 min for
  the other to come back announcing the new version (or speaking another protocol version, which
  also proves it changed) and only then installs itself. The two are never restarted at the
  same time, so keyboard and mouse stay on the one that works. If the other does not come back, **rollback**:
  this one is not touched and a notice is shown; on retrying, the plan only installs what
  is missing.
- **"Update automatically"** (on by default, stored in `update.toml` next to
  `tandem.toml`): checks every 6 h. With a peer, only machine `A` leads (the other waits for its request);
  with no peer, each one updates itself.

### Updater key

It is generated once and stored as a repo secret; the public one goes in `tauri.conf.json`.

```sh
cd apps/tandem-ui
pnpm tauri signer generate -w ~/.config/tandem-release/updater.key   # asks for a password (optional)
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.config/tandem-release/updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD   # only if you set a password
```

Afterwards, the contents of `~/.config/tandem-release/updater.key.pub` go in
`plugins.updater.pubkey` of `apps/tandem-ui/src-tauri/tauri.conf.json`. **The private one never
goes into the repo.** If it is lost, installed apps no longer accept updates signed with
another one: the new version would have to be installed by hand on both machines.

Without the secret, test mode signs with a disposable key (the packages are useless for
updating anyone, but the whole path is tested) and a release fails in `prepare`.

## Version

The version lives in three places that have to match: `version` of `[workspace.package]`
in `Cargo.toml`, `apps/tandem-ui/src-tauri/tauri.conf.json` and `apps/tandem-ui/package.json`.
`scripts/release/version.sh [v<version>]` checks it and, with a tag, requires it to be
exactly `v` + that version. The workflow always runs it, so a tag that does not match
the version fails before compiling anything. The updater compares versions with SemVer
order (`0.3.0-rc.2` < `0.3.0` < `0.3.1`).

## Notes

`scripts/release/notes.sh [--version x.y.z] [--from <ref>] [--to <ref>]` builds the notes
from the integrated tasks: each task PR enters `develop` as a squash
`T-<id>: <title> (#<pr>)`. By default it takes from the previous `v*` tag (or from the beginning).

- It groups the tasks by the `phase` of their `docs/tasks/T-<id>.md`, in the order of the design's
  phases. A task with several commits appears once, with all its PRs.
- What does not belong to a task (backlog, QA, design) goes in "Other changes".
- At the end, the tasks with unticked boxes in `## Pending hardware verification`:
  what nobody has checked yet on the real machine.

To see them locally: `scripts/release/notes.sh --version "$(scripts/release/version.sh)"`.
The same notes go in `latest.json` (the updater sees them).

## Cutting a release

Checklist (done by the user; agents never tag, publish or touch `main`):

- [ ] **Once:** the repo has the secrets `TAURI_SIGNING_PRIVATE_KEY` ([Updater key](#updater-key))
  and `MACOS_SIGNING_CERTIFICATE` + `MACOS_SIGNING_CERTIFICATE_PASSWORD`
  ([macOS signing identity](#macos-signing-identity)). `gh secret list` shows them.
- [ ] **Bump:** on `develop`, the version in the three files (and `Cargo.lock` with
  `cargo check`), integrated like any change; `scripts/release/version.sh` agrees.
- [ ] **CI green** on that `develop` commit, including `dmg (macos-latest)` (DR check) and
  `installer (windows)`.
- [ ] **Prerelease (optional):** version `x.y.z-rc.N`, tag `vx.y.z-rc.N` on that commit. It stays
  as a draft; install the two packages on the real machine. (Or run the workflow by hand in
  test mode and review the `tandem-<version>` artifact.)
- [ ] **Tag:** bring the change to `main` and push the final version's tag:
  ```sh
  git switch main && git pull
  git tag -s v0.3.0 -m "Tandem 0.3.0"
  git push origin v0.3.0
  ```
- [ ] **Publish:** the workflow publishes the release at the end (its summary says "Signed with
  the stable identity <SHA-1>" and the designated requirements); from that moment the apps
  find it.
- [ ] **Hardware check:** on the real desk, "Update" in either app updates both machines (the
  other first), keyboard and mouse keep working throughout, and the Mac keeps Accessibility,
  Input Monitoring and Local Network with no prompt (`docs/tasks/T-348.md`). The first release
  signed with the stable identity is the exception: grants made to the ad-hoc builds before it
  do not carry over, so that one update asks once.

To check a download: `sha256sum -c SHA256SUMS.txt --ignore-missing` (on macOS,
`shasum -a 256 -c SHA256SUMS.txt`; on Windows, `Get-FileHash` and compare).

## macOS signing identity

macOS ties Accessibility, Input Monitoring and Local Network to the **designated requirement**
(DR) of the code that was granted them: TCC stores it and, on each access, checks that the
running code satisfies it. An ad-hoc signature has the DR `cdhash H"..."`, the hash of that exact
build, so every version was a new app and asked for the permissions again. Since T-348 the release
signs `Tandem.app` and `tandemd` with a **stable self-signed code-signing identity** (DECISIONS
T-348; no Apple Developer account). Its DR is the identifier plus the certificate, which every
version signed with it satisfies:

```
designated => identifier "com.franc1sc0sv.tandem" and certificate leaf = H"<SHA-1 of the certificate>"
designated => identifier tandemd and certificate leaf = H"<the same SHA-1>"             # Contents/MacOS/tandemd
```

`tandemd` is what the LaunchAgent runs and what holds the permissions; with no Info.plist of its
own, codesign names it after the file. The certificate has no Organization on purpose: for a
non-Apple certificate, codesign pins the last certificate of the chain that shares the leaf's
Organization (`DRMaker::nonAppleAnchor` in Apple's open-source Security framework), and with a
single self-signed certificate and no Organization that is `certificate leaf`. Either way it is
the SHA-1 of the one certificate, the same in every build. These are the lines the
`dmg (macos-latest)` job printed for its disposable identity (`scripts/macos/bundle.sh` prints them
on every build, and the release summary shows them).

### Creating it (once)

On any machine with `openssl` (Linux, WSL or macOS), from the repository folder:

```sh
scripts/release/make-macos-identity.sh
# writes ~/.config/tandem-release/macos-identity/{identity.p12,identity.p12.b64,password,certificate.pem}
# and prints these two commands:
gh secret set MACOS_SIGNING_CERTIFICATE < ~/.config/tandem-release/macos-identity/identity.p12.b64
gh secret set MACOS_SIGNING_CERTIFICATE_PASSWORD < ~/.config/tandem-release/macos-identity/password
```

The certificate lasts 20 years (`--days`), has the code-signing usage, and the `.p12` uses the
classic SHA1-3DES encryption that macOS `security import` reads. **The `.p12` never goes into the
repo**; keep the folder backed up somewhere private. Nothing else changes: the
`signingIdentity: "-"` of `tauri.macos.conf.json` stays for local builds, and the workflow sets
`APPLE_SIGNING_IDENTITY` (an environment variable wins over the config).

### What the workflows do

- **Release** (`release.yml`, tag or by hand without test mode): `prepare` fails with the
  reason if either secret is missing. The `dmg` job decodes the `.p12`, imports it into a
  keychain that only lives for the job (`scripts/macos/keychain.sh import`: random keychain
  password, `set-key-partition-list` so codesign needs no dialog) and passes the identity's
  SHA-1 as `APPLE_SIGNING_IDENTITY`. Tauri passes it as is to `codesign -s` (with hardened
  runtime) for `tandemd` and the app. The keychain is deleted at the end, even on failure.
- **Test mode and PR CI** (`release.yml` in test mode, `ci.yml`'s `dmg (macos-latest)`): the same
  with a disposable identity made on the spot (`keychain.sh disposable`), never the real
  secret. The path is the same; the result is useless for updating anyone.
- **The DR check** (`scripts/macos/dr-check.sh all`), in both: it copies the built app into a
  second version (other `CFBundleShortVersionString`/`CFBundleVersion`, and a `tandemd` with
  another SDK version in `LC_BUILD_VERSION`, so both cdhashes change) and signs it again with the
  same identity. For the app and for `tandemd` it fails if the DR is missing or a `cdhash`
  (ad hoc), if the two `codesign -d -r-` differ, if the second version does not satisfy the
  first one's DR (`codesign --verify -R "=<DR>"`, what TCC evaluates), or if both have the same
  cdhash (then it proved nothing). In the release it also unpacks the updater package and checks
  that it keeps the DR and carries no `com.apple.quarantine`.

### Rotating it

A new certificate is a new DR: every Mac asks for the permissions once more on the next
update. Only rotate it if the `.p12` leaks or is lost:

```sh
scripts/release/make-macos-identity.sh --force   # replaces the files in the folder
gh secret set MACOS_SIGNING_CERTIFICATE < ~/.config/tandem-release/macos-identity/identity.p12.b64
gh secret set MACOS_SIGNING_CERTIFICATE_PASSWORD < ~/.config/tandem-release/macos-identity/password
```

Say it in that release's notes. A self-signed certificate cannot be revoked (TN2206): if the key
leaks, rotating is the only answer, and the old certificate stays trusted by every grant made to it
until the user removes those entries.

### Caveats

- **Gatekeeper:** a self-signed app is still "unidentified developer" on its first install from
  the `.dmg` (right click → Open, see [install-macos.md](install-macos.md#install)). Updates
  through the app do not go through that again (no quarantine, see above).
- **The first signed release:** grants made to the ad-hoc builds stored their `cdhash` DR, which
  the new signature does not satisfy. Updating from an ad-hoc version (every release before T-348) to
  the first self-signed one asks once; from then on the grants stay.
- **Local Network:** Apple says it "tracks the identity of your program using its code
  signature" and also uses the main executable's Mach-O UUID (TN3179). Since T-344 the LaunchAgent
  names the app in `AssociatedBundleIdentifiers`, so the grant belongs to `Tandem.app`, whose DR
  is now stable. Whether a new UUID alone makes it ask again is not documented, hence the
  hardware check.
- **Expiry:** codesign refuses to sign with an expired certificate (the signatures already made
  stay valid, TN2206), hence the 20 years; when it expires, it is a rotation.
- **Hardened runtime with no Team ID:** library validation only lets the app load system
  libraries or its own; Tandem loads nothing else.

References: Apple TN2206 *macOS Code Signing In Depth* ("Code Designated Requirement": a DR
"should also be satisfied by updates… This is how the macOS code signing policy engine recognizes
updates"; "Self-signed identities and homemade certificate authorities (CA) work by default for
this case"), TN3127 *Inside Code Signing: Requirements* ("Ad hoc signed code… has a DR but it's tied
to that specific version of the code"; macOS records the app's DR and "checks that this version of
the app satisfies the original DR"), TN3179 *Understanding local network privacy*, and
`man codesign` (`-r-`, `-R`).

## Signing

- **macOS:** the stable self-signed identity above in the release (and a disposable one in CI);
  ad hoc for local builds. Developer ID and notarization stay possible with the same Tauri
  variables (`APPLE_SIGNING_IDENTITY`, `APPLE_ID`...; [install-macos.md](install-macos.md#signing)).
- **Windows:** no Authenticode (D19); it is enabled with `TANDEM_WINDOWS_SIGN_COMMAND` or
  `TANDEM_WINDOWS_CERT_THUMBPRINT` ([install-windows.md](install-windows.md)), stored as secrets
  and passed as `env:` to the build step. They are deliberately not passed empty.

The updater's minisign signature is something else and is in place: see [Updater key](#updater-key).

## Pending

- Universal macOS binary (today `aarch64`, the architecture of the `macos-latest` runner; the
  `latest.json` only carries `darwin-aarch64` and `windows-x86_64`).
- Developer ID / Authenticode signing in the release (user's decision, see above); the self-signed
  macOS identity covers the permissions, not Gatekeeper's first opening.
