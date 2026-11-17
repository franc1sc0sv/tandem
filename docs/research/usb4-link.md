# Enlace USB4 Mac ↔ PC (spike S5)

Pruebas del 2026-11-16 con la MacBook Pro (Apple Silicon, macOS 27.0.1) y la PC (Gigabyte X870 GAMING X WIFI7, BIOS F3, Windows 11 25H2).

## PC

- Controlador: ASMedia **ASM4242** (`PCI\VEN_1B21&DEV_2425`), connection manager de Windows (`USB4(TM) Host Router (Microsoft)`, `usb4hostrouter.inf` 10.0.26100.9818). Firmware del router: **0.1a**. BIOS: Discrete USB4 Support **Enabled**, FW `240521200011`, FWCM Handoff **Released**, Native USB4 **Enabled**.
- Windows trae red USB4 entre hosts (USB4NET, compatible con la red de Thunderbolt 3): al conectarse aparece «USB4 P2P Network Adapter» con IP `169.254.x.x` ([Microsoft Learn](https://learn.microsoft.com/en-us/windows-hardware/design/component-guidelines/usb4-interdomain-connections)). No hay políticas que lo bloqueen.
- Nunca se enumeró un dispositivo bajo `HKLM\SYSTEM\CurrentControlSet\Enum\USB4` aparte del propio router: no hubo conexión entre dominios. Reiniciar el host router con `pnputil /restart-device` y apagar ASPM no cambió nada.
- El PD de los puertos es un ITE (`VID 0x048D`, `PID 0x8851`) por ACPI; no hay UCSI visible.

## Mac (por `ioreg -r -c AppleHPMInterfaceType10`)

- Con el cable de 10 Gbps: `ConnectionActive = Yes`, `TransportsActive = ("CC")`, `TransportsProvisioned = ("CC","USB2","CIO")`. Rol de datos **Host**, recibe energía de la PC.
- El socio (la PC) responde Discover Identity: VDO de UFP `0x4900000C` → declara **USB4** y USB 2.0. El cable: pasivo, VID 5666, `CableSpeed = 2` (USB 3.2 Gen 2, 10 Gbps).
- `system_profiler SPThunderboltDataType`: «No device connected» en los tres buses. `bridge0` (Thunderbolt Bridge) `inactive`.
- Con un cable USB 2.0 (480 Mb/s): imposible, no tiene pares de alta velocidad.

## Conclusión

La negociación PD funciona y la PC dice soportar USB4, pero el enlace USB4 (CIO) nunca entrena. Sospechas: el cable de 10 Gbps con e-marker anterior a USB4, o el firmware del ASM4242/PD de la placa. Sin actualizar BIOS (decisión del usuario) queda probar un cable **Thunderbolt 4 / USB4 de 40 Gbps**. Hasta entonces: Wi-Fi (y Ethernet si se agrega un adaptador a la Mac).

## Cómo volver a comprobar

```bash
ssh mac-runner 'ioreg -lw0 -r -c AppleHPMInterfaceType10 | grep -E "\"(PortDescription|TransportsActive)\""; ifconfig bridge0 | grep status'
powershell.exe -NoProfile -Command "Get-NetAdapter | ft Name,InterfaceDescription,Status,LinkSpeed"
```

Enlace bueno = `CIO` en `TransportsActive`, `bridge0` `active` y un adaptador «USB4 P2P» en Windows.
