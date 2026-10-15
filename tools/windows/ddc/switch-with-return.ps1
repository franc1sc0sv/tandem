Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices; using System.Collections.Generic;
public static class Ddc {
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
  public struct PM { public IntPtr h; [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string d; }
  public delegate bool EnumProc(IntPtr hMon, IntPtr hdc, IntPtr rect, IntPtr data);
  [DllImport("user32.dll")] public static extern bool EnumDisplayMonitors(IntPtr hdc, IntPtr clip, EnumProc cb, IntPtr data);
  [DllImport("dxva2.dll")] public static extern bool GetNumberOfPhysicalMonitorsFromHMONITOR(IntPtr h, out uint n);
  [DllImport("dxva2.dll", CharSet=CharSet.Unicode)] public static extern bool GetPhysicalMonitorsFromHMONITOR(IntPtr h, uint n, [Out] PM[] a);
  [DllImport("dxva2.dll", SetLastError=true)] public static extern bool GetVCPFeatureAndVCPFeatureReply(IntPtr h, byte code, IntPtr type, out uint cur, out uint max);
  [DllImport("dxva2.dll", SetLastError=true)] public static extern bool SetVCPFeature(IntPtr h, byte code, uint val);
  [DllImport("dxva2.dll")] public static extern bool DestroyPhysicalMonitors(uint n, PM[] a);
  static List<PM> All() {
    var l = new List<PM>();
    EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero, (hm, a, b, c) => { uint n;
      if (GetNumberOfPhysicalMonitorsFromHMONITOR(hm, out n) && n > 0) { var arr = new PM[n]; if (GetPhysicalMonitorsFromHMONITOR(hm, n, arr)) l.AddRange(arr); }
      return true; }, IntPtr.Zero);
    return l;
  }
  public static string Read() {
    var l = All(); if (l.Count == 0) return "monitores=0";
    uint cur, max; bool ok = GetVCPFeatureAndVCPFeatureReply(l[0].h, 0x60, IntPtr.Zero, out cur, out max);
    int err = Marshal.GetLastWin32Error(); DestroyPhysicalMonitors((uint)l.Count, l.ToArray());
    return "input=" + (ok ? "0x" + cur.ToString("X2") : "ERR " + err);
  }
  public static string Write(uint v) {
    var l = All(); if (l.Count == 0) return "sin monitor";
    bool ok = SetVCPFeature(l[0].h, 0x60, v); int err = Marshal.GetLastWin32Error();
    DestroyPhysicalMonitors((uint)l.Count, l.ToArray());
    return "ok=" + ok + (ok ? "" : " err=" + err);
  }
}
"@

$f = New-Object Windows.Forms.Form
$f.TopMost = $true; $f.Width = 1000; $f.Height = 300; $f.StartPosition = 'CenterScreen'; $f.Text = 'Prueba DDC'; $f.BackColor = 'Black'
$lbl = New-Object Windows.Forms.Label; $lbl.Dock = 'Fill'; $lbl.ForeColor = 'Yellow'; $lbl.TextAlign = 'MiddleCenter'
$lbl.Font = New-Object Drawing.Font('Segoe UI', 40, [Drawing.FontStyle]::Bold); $f.Controls.Add($lbl); $f.Show()
function Show($t) { $lbl.Text = $t; [Windows.Forms.Application]::DoEvents() }
function Wait($s) { $end = (Get-Date).AddSeconds($s); while ((Get-Date) -lt $end) { [Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 100 } }
function T { (Get-Date).ToString("HH:mm:ss") }
foreach ($v in 3, 6) {
  $hex = "0x{0:X2}" -f $v
  Show "En 3 s: probando $hex"; Wait 3
  "$(T)  set $hex " + [Ddc]::Write($v); Show "Enviado $hex"; Wait 10
  "$(T)    vuelta 0x09 " + [Ddc]::Write(9); Show "Volviendo con 0x09 (desde $hex)"; Wait 6
  "$(T)    vuelta 0x0F " + [Ddc]::Write(0x0F); Show "Volviendo con 0x0F (desde $hex)"; Wait 6
}
Show "Fin"; Wait 2; $f.Close()
