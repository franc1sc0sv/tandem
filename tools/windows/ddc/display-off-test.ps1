Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices; using System.Collections.Generic;
public static class Pw {
  [DllImport("user32.dll")] public static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, IntPtr l, uint f, uint t, out IntPtr r);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, IntPtr e);
  public static void Off() { IntPtr r; SendMessageTimeout((IntPtr)0xFFFF, 0x0112, (IntPtr)0xF170, (IntPtr)2, 2, 1000, out r); }
  public static void Wake() { mouse_event(1, 1, 0, 0, IntPtr.Zero); System.Threading.Thread.Sleep(50); mouse_event(1, -1, 0, 0, IntPtr.Zero); }
}
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
"$(T)  base: " + [Ddc]::Timing()
Start-Sleep 3
"$(T)  >> apagar pantalla (30 s sin consultas)"; [Pw]::Off()
Start-Sleep 30
"$(T)  lectura al final del reposo: " + [Ddc]::Timing()
"$(T)  >> despertar"; [Pw]::Wake()
for ($i=0; $i -lt 10; $i++) { Start-Sleep 1; "$(T)    " + [Ddc]::Timing() }
