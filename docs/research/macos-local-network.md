# Local Network permission for a LaunchAgent on macOS 15+

> Status (2026-11-20, T-344): research from Apple's documentation and the Apple Developer
> Forums, implemented in `tandemd::local_network` and the LaunchAgent plist. **Not yet tested
> on the real Mac**: what has to be checked is in `docs/tasks/T-344.md`.

## The symptom (2026-11-19, a Mac managed by MDM)

`tandemd`, started by the LaunchAgent, never got the Local Network alert and did not show up in
System Settings → Privacy & Security → Local Network. The two machines did not find each other
until the user ran `tandemd` from Terminal, which uses Terminal's permission.

## What Apple documents

Source: Apple TN3179 "Understanding local network privacy"
(<https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy>,
revision of 2026-10-06; read through its JSON at
`https://developer.apple.com/tutorials/data/documentation/technotes/tn3179-understanding-local-network-privacy.json`
because the page itself is rendered with JavaScript).

- **Who is exempt.** "macOS automatically allows local network access by: Any daemon started
  by `launchd`; Any program running as root; Command-line tools run from Terminal or over
  SSH, including any child processes they spawn." And: "The exception for `launchd` daemons
  doesn't apply to `launchd` agents." That explains why `tandemd` from Terminal worked and the
  LaunchAgent did not.
- **Responsible code.** "When a process performs a local network operation, macOS tries to
  track down the responsible code", and uses it for the alert's name and usage text, to record
  the choice "for the whole app", and in System Settings.
- **The fix TN3179 gives for exactly our case.** "If you ship a `launchd` agent that's not
  installed using `SMAppService`, make macOS aware of the responsible code by setting the
  `AssociatedBundleIdentifiers` property in your `launchd` property list." Tandem's agent is
  installed by writing `~/Library/LaunchAgents/…plist` and `launchctl bootstrap`, not with
  `SMAppService`, and its plist had no such key.
- **Usage description.** `NSLocalNetworkUsageDescription` and `NSBonjourServices` go in the
  app's `Info.plist` (Tandem already has both, `apps/tandem-ui/src-tauri/Info.plist`). A
  standalone executable needs them in an embedded `Info.plist` only if it has no app acting as
  its responsible code.
- **Short-lived processes.** "macOS fails to display the local network alert when a process
  with a very short lifespan performs a local network operation […] update your code to not
  exit immediately after a local network operation fails." `tandemd` is long-lived, but this
  rules out probing from a helper that exits.
- **Triggering the alert.** There is no API for it; "connect a UDP socket to a local network
  address" triggers it. All multicast addresses (224.0.0.0/4, ff00::/8) are local network
  addresses. While the alert is up, the system "may deny the operation immediately, before the
  user has responded", so code has to retry.
- **Reading the state.** "There's no general API that returns whether the current process has
  local network access (FB8711182)." Only Bonjour (`kDNSServiceErr_PolicyDenied`) and
  `NWConnection` (`unsatisfiedReason == .localNetworkDenied`) say it, neither of which `tandemd`
  uses (it is BSD sockets through `mdns-sd` and `quinn`).
- **Identity.** It "tracks the identity of your program using its code signature"; ad-hoc
  signing makes that unreliable (T-348 moves to a stable identity). Several installed copies of
  the same app confuse System Settings (FB15568200).
- **MDM.** "Device managers aren't able to configure local network privacy using MDM." A
  profile cannot grant it, so the managed Mac in the report is not the cause by itself. Only the
  `com.apple.network.local-network` defaults (`AllowedEthernetLocalNetworkAddresses`,
  `AllowedWiFiLocalNetworkAddresses`, macOS 15.5+, set with `sudo`, applied on restart) exempt
  whole networks.
- **Reset.** Since macOS 27.2 the Local Network list has + and − buttons; before that there is
  no way to reset a program's state short of a new user account or a VM.

## What the forums add

- **The errno.** A BSD send or connect to a local address fails with `EHOSTUNREACH` while the
  permission is undetermined (the alert is pending) and when it is denied. Quinn (Apple DTS):
  "Our hands are tied here because of the limitations of the available `errno` error codes"
  (<https://developer.apple.com/forums/thread/765285>), and asked whether any errno tells a
  local network denial apart, "No"; "I'd really like it to fail with `EPERM` here, but BSD
  Sockets code just isn't set up for that" (<https://developer.apple.com/forums/thread/778457>).
- **`AssociatedBundleIdentifiers` is not bulletproof.** In
  <https://developer.apple.com/forums/thread/774160> a developer set it, with the same Team ID as
  the app, and the alert still named the agent. Quinn reproduced it on macOS 15.2: attribution
  worked for Input Monitoring but not for the local network trigger (FB16563823). So the alert
  may still say "tandemd" instead of "Tandem". Either way it appears, and that is what was missing.

## What Tandem does (T-344)

1. **Plist**: the LaunchAgent carries `AssociatedBundleIdentifiers = [com.franc1sc0sv.tandem]`
   (`tandem_ui::launch_agent::plist`). Agents installed before are rewritten and reloaded the
   next time the app opens (the plist changes).
2. **Probe** (`tandemd::local_network`, macOS only): at start-up and then every 5 s while not
   granted (60 s once granted), a UDP socket connected to `224.0.0.251:9` (the mDNS group, the
   discard port) sends a zero-length datagram. No ARP and no real traffic beyond one empty
   multicast packet. The first send is also what brings up the alert; the agent stays alive
   after it.
3. **Denied vs. not asked**: errno cannot tell them apart, so time and history do:
   - the send goes through → `Granted`;
   - `EHOSTUNREACH` right after a success → `Denied` (macOS never asks twice; the user turned it
     off);
   - `EHOSTUNREACH` for 20 s or more → `Denied` (denied, or the alert left unanswered);
   - `EHOSTUNREACH` for less than that, `ENETUNREACH`/no address, or no attempt yet → `Unknown`;
   - any other error keeps the last state.
4. **UI**: `Capabilities::local_network` (protocol v44) reaches step 3 of the setup, which shows
   the state and an "Ask again" button (`UiCommand::CheckLocalNetwork`: the agent probes at
   once). The pairing step and the Network screen say "Local Network permission missing on this
   Mac" with the System Settings path when the state is `Denied`.

## If the alert still does not appear on the real Mac

To try, in this order, and record in `docs/tasks/T-344.md`:

- `launchctl print gui/$(id -u)/com.franc1sc0sv.tandem.tandemd` lists
  `AssociatedBundleIdentifiers`? `log stream --predicate 'subsystem == "com.apple.network"' --info`
  while the agent starts shows the local network decision and the responsible process.
- A stable signature (T-348): with ad-hoc signing every build is a new identity.
- Probe from the app process too (`Tandem.app` is a real bundle with the usage text): if
  attribution works, the app's grant covers the agent.
- Embed an `Info.plist` in `tandemd` (`-sectcreate __TEXT __info_plist`) with
  `NSLocalNetworkUsageDescription`, as TN3179 allows for standalone executables. Note that
  `codesign` then takes its `CFBundleIdentifier` as the binary's identifier, which changes how
  TCC records Accessibility and Input Monitoring.
- Install the agent with `SMAppService.agent(plistName:)` from inside the bundle
  (`Contents/Library/LaunchAgents`), which TN3179 names as the case that needs no key.
