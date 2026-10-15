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
  [StructLayout(LayoutKind.Sequential)] public struct TR { public uint hf; public uint vf; public byte st; }
  [DllImport("dxva2.dll", SetLastError=true)] public static extern bool GetTimingReport(IntPtr h, ref TR t);
  public static string Timing() { var l = All(); if (l.Count == 0) return "sin monitor"; var t = new TR();
    bool ok = GetTimingReport(l[0].h, ref t); int e = Marshal.GetLastWin32Error(); DestroyPhysicalMonitors((uint)l.Count, l.ToArray());
    return ok ? ("H=" + t.hf + "Hz V=" + (t.vf/100.0) + "Hz status=0x" + t.st.ToString("X2")) : ("timing ERR " + e); }
  [DllImport("dxva2.dll", SetLastError=true)] public static extern bool GetCapabilitiesStringLength(IntPtr h, out uint len);
  [DllImport("dxva2.dll", SetLastError=true, CharSet=CharSet.Ansi)] public static extern bool CapabilitiesRequestAndCapabilitiesReply(IntPtr h, System.Text.StringBuilder s, uint len);
  public static string Caps() { var l = All(); if (l.Count == 0) return "sin monitor"; uint n; string r;
    if (!GetCapabilitiesStringLength(l[0].h, out n)) r = "len ERR " + Marshal.GetLastWin32Error();
    else { var sb = new System.Text.StringBuilder((int)n); r = CapabilitiesRequestAndCapabilitiesReply(l[0].h, sb, n) ? sb.ToString() : "caps ERR " + Marshal.GetLastWin32Error(); }
    DestroyPhysicalMonitors((uint)l.Count, l.ToArray()); return r; }
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



function T { (Get-Date).ToString("HH:mm:ss.f") }
"CAPS: " + [Ddc]::Caps()
$base = [Ddc]::Timing(); "$(T)  base: $base"
foreach ($v in 1..14) {
  if ($v -eq 9) { continue }
  $hex = "0x{0:X2}" -f $v
  $w = [Ddc]::Write($v)
  $seen = @()
  for ($i=0; $i -lt 14; $i++) { Start-Sleep -Milliseconds 500; $t = [Ddc]::Timing(); if ($t -ne $base) { $seen += $t } }
  "$(T)  $hex $w  distintos=$($seen.Count)/14  " + (($seen | Select-Object -Unique) -join ' | ')
  if ($seen.Count -ge 3) {
    "$(T)    vuelta 0x09 " + [Ddc]::Write(9)
    for ($i=0; $i -lt 16; $i++) { Start-Sleep -Milliseconds 500; if ([Ddc]::Timing() -eq $base) { break } }
    "$(T)    tras vuelta: " + [Ddc]::Timing()
  }
}
