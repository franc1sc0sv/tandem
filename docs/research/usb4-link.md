# USB4 link Mac ↔ PC (spike S5)

Tests of 2026-11-16 with the MacBook Pro (Apple Silicon, macOS 27.0.1) and the PC (Gigabyte X870 GAMING X WIFI7, BIOS F3, Windows 11 25H2).

## PC

- Controller: ASMedia **ASM4242** (`PCI\VEN_1B21&DEV_2425`), Windows connection manager (`USB4(TM) Host Router (Microsoft)`, `usb4hostrouter.inf` 10.0.26100.9818). Router firmware: **0.1a**. BIOS: Discrete USB4 Support **Enabled**, FW `240521200011`, FWCM Handoff **Released**, Native USB4 **Enabled**.
- Windows brings host-to-host USB4 networking (USB4NET, compatible with Thunderbolt 3 networking): on connecting, "USB4 P2P Network Adapter" appears with IP `169.254.x.x` ([Microsoft Learn](https://learn.microsoft.com/en-us/windows-hardware/design/component-guidelines/usb4-interdomain-connections)). There are no policies that block it.
- No device was ever enumerated under `HKLM\SYSTEM\CurrentControlSet\Enum\USB4` apart from the router itself: there was no connection between domains. Restarting the host router with `pnputil /restart-device` and turning off ASPM changed nothing.
- The ports' PD is an ITE (`VID 0x048D`, `PID 0x8851`) through ACPI; no UCSI is visible.

## Mac (through `ioreg -r -c AppleHPMInterfaceType10`)

- With the 10 Gbps cable: `ConnectionActive = Yes`, `TransportsActive = ("CC")`, `TransportsProvisioned = ("CC","USB2","CIO")`. Data role **Host**, it receives power from the PC.
- The partner (the PC) answers Discover Identity: UFP VDO `0x4900000C` → it declares **USB4** and USB 2.0. The cable: passive, VID 5666, `CableSpeed = 2` (USB 3.2 Gen 2, 10 Gbps).
- `system_profiler SPThunderboltDataType`: "No device connected" on the three buses. `bridge0` (Thunderbolt Bridge) `inactive`.
- With a USB 2.0 cable (480 Mb/s): impossible, it has no high-speed pairs.

## Conclusion

PD negotiation works and the PC says it supports USB4, but the USB4 link (CIO) never trains. Suspects: the 10 Gbps cable with a pre-USB4 e-marker, or the ASM4242/PD firmware on the board. Without updating the BIOS (user's decision) what remains is trying a **40 Gbps Thunderbolt 4 / USB4** cable. Until then: Wi-Fi (and Ethernet if an adapter is added to the Mac).

## How to check again

```bash
ssh mac-runner 'ioreg -lw0 -r -c AppleHPMInterfaceType10 | grep -E "\"(PortDescription|TransportsActive)\""; ifconfig bridge0 | grep status'
powershell.exe -NoProfile -Command "Get-NetAdapter | ft Name,InterfaceDescription,Status,LinkSpeed"
```

A good link = `CIO` in `TransportsActive`, `bridge0` `active` and a "USB4 P2P" adapter on Windows.
