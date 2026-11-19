import { describe, expect, it } from "vitest";

import {
  HISTORY,
  checkName,
  missingPermission,
  cpuSub,
  diskText,
  envLabel,
  gb,
  gpuSub,
  headerSub,
  nextLive,
  orderEnvs,
  pct,
  ramSub,
  shortCpu,
  shortGpu,
  summaryOf,
  tint,
  trendArea,
  trendPoints,
  vramWarn,
} from "./machine";
import type { ResourceSample } from "./model";
import type { EnvView, HardwareView, LiveView } from "./store/types";

const GIB = 2 ** 30;

const PC: HardwareView = { cpu: "AMD Ryzen 7 7800X3D 8-Core Processor", cores: 16, memory: 24 * GIB, gpu: { name: "NVIDIA GeForce RTX 5080", vram: 16 * GIB } };
const MAC: HardwareView = { cpu: "Apple M5", cores: 12, memory: 24 * GIB, gpu: { name: "Apple M5", vram: null } };

function sample(over: Partial<ResourceSample> = {}): ResourceSample {
  return {
    cpu: { load: 640, cores: [], temperature: 62 },
    memory: { used: 17.1 * GIB, total: 24 * GIB },
    gpus: [{ name: "RTX 5080", load: 970, vram_used: 14.1 * GIB, vram_total: 16 * GIB, temperature: 71 }],
    disk: { free: 412 * GIB, total: 1.8 * 1024 * GIB },
    ...over,
  };
}

describe("unidades (V3-Machine)", () => {
  it("GB con un decimal sin «.0» y enteros desde 100", () => {
    expect(gb(17.1 * GIB)).toBe("17.1");
    expect(gb(24 * GIB)).toBe("24");
    expect(gb(1.2 * GIB)).toBe("1.2");
    expect(gb(412.4 * GIB)).toBe("412");
    expect(gb(0)).toBe("0");
  });

  it("el disco dice libres y el total en TB cuando pasa de 1000 GB", () => {
    expect(diskText(412 * GIB, 1.8 * 1024 * GIB)).toBe("412 GB free of 1.8 TB");
    expect(diskText(231 * GIB, 1024 * GIB)).toBe("231 GB free of 1 TB");
    expect(diskText(40 * GIB, 256 * GIB)).toBe("40 GB free of 256 GB");
  });

  it("los porcentajes quedan entre 0 y 100, también con totales raros", () => {
    for (let i = 0; i < 200; i++) {
      const total = Math.random() * 1e12 - 1e11;
      const part = Math.random() * 2e12 - 5e11;
      const p = pct(part, total);
      expect(p).toBeGreaterThanOrEqual(0);
      expect(p).toBeLessThanOrEqual(100);
      expect(Number.isNaN(p)).toBe(false);
    }
    expect(pct(1, 0)).toBe(0);
  });
});

describe("nombres del equipo (salen del escaneo, D10)", () => {
  it("procesador y GPU sin marcas", () => {
    expect(shortCpu("AMD Ryzen 7 7800X3D 8-Core Processor")).toBe("Ryzen 7 7800X3D");
    expect(shortCpu("Intel(R) Core(TM) i7-13700K CPU @ 3.40GHz")).toBe("Core i7-13700K");
    expect(shortCpu("Apple M5")).toBe("M5");
    expect(shortGpu("NVIDIA GeForce RTX 5080")).toBe("RTX 5080");
    expect(shortGpu("Apple M5")).toBe("M5");
    expect(shortGpu("AMD Radeon RX 7900 XT")).toBe("Radeon RX 7900 XT");
  });

  it("subtítulos de cada grupo", () => {
    expect(cpuSub(PC, "windows")).toBe("Ryzen 7 7800X3D · 16 threads");
    expect(cpuSub(MAC, "macos")).toBe("M5 · 12 cores");
    expect(cpuSub(null, "macos")).toBe("");
    expect(ramSub(PC, null)).toBe("24 GB");
    expect(ramSub(MAC, null)).toBe("unified");
    expect(gpuSub(PC)).toBe("RTX 5080");
    expect(gpuSub(MAC)).toBe("M5 integrated");
    expect(gpuSub({ ...PC, gpu: null })).toBe("");
  });

  it("bajo el nombre: la otra dice si está online; esta, su procesador", () => {
    expect(headerSub({ here: false, system: "Windows 11", power: "on", hardware: PC })).toBe("Windows 11 · online");
    expect(headerSub({ here: false, system: "Windows 11", power: "off", hardware: PC })).toBe("Windows 11 · off");
    expect(headerSub({ here: false, system: "Windows", power: "lost", hardware: null })).toBe("Windows · offline");
    expect(headerSub({ here: true, system: "macOS 26", power: "on", hardware: MAC })).toBe("macOS 26 · Apple M5");
    expect(headerSub({ here: true, system: "macOS", power: "on", hardware: null })).toBe("macOS");
  });

  it("el fondo tenue sale del acento (los de la palette, los del mockup)", () => {
    expect(tint("#a99be8")).toBe("#1d1a2b");
    expect(tint("#7AA7E6")).toBe("#16202e");
    expect(tint("#123456")).toContain("color-mix");
  });
});

describe("entornos", () => {
  const env = (id: string, kind: EnvView["kind"], def: boolean): EnvView => ({ id, kind, name: id, default: def, ready: true, detail: "" });

  it("la distro de WSL por omisión va primero; si no hay, el shell; el resto en su orden", () => {
    const pwsh = env("pwsh", "shell", true);
    const wsl = env("Ubuntu", "wsl", true);
    const docker = env("docker", "docker", false);
    expect(orderEnvs([pwsh, wsl, docker])).toEqual({ main: wsl, rest: [pwsh, docker] });
    expect(orderEnvs([docker, pwsh])).toEqual({ main: pwsh, rest: [docker] });
    expect(orderEnvs([docker])).toEqual({ main: null, rest: [docker] });
    expect(orderEnvs([])).toEqual({ main: null, rest: [] });
  });

  it("ningún entorno se pierde ni se repite", () => {
    for (let n = 0; n < 50; n++) {
      const kinds: EnvView["kind"][] = ["shell", "wsl", "docker"];
      const list = Array.from({ length: n % 6 }, (_, i) => env(`e${i}`, kinds[(n + i) % 3] ?? "shell", (n * (i + 1)) % 3 === 0));
      const { main, rest } = orderEnvs(list);
      const all = [...(main ? [main] : []), ...rest];
      expect(new Set(all.map((e) => e.id))).toEqual(new Set(list.map((e) => e.id)));
      expect(all.length).toBe(list.length);
    }
  });

  it("las distros de WSL dicen que lo son", () => {
    expect(envLabel({ kind: "wsl", name: "Ubuntu 24.04" })).toBe("WSL · Ubuntu 24.04");
    expect(envLabel({ kind: "shell", name: "zsh" })).toBe("zsh");
  });
});

describe("líneas de tendencia", () => {
  it("van de 0 a 120 y abajo es 0 %", () => {
    expect(trendPoints([0, 50, 100], 120, 34)).toBe("0,34 60,17 120,0");
    expect(trendPoints([150, -5], 120, 20)).toBe("0,0 120,20");
    expect(trendPoints([])).toBe("");
    expect(trendArea([0, 100], 120, 34)).toBe("M0,34 L120,0 L120 34 L0 34Z");
  });
});

describe("muestras del agente → vista en vivo", () => {
  it("convierte décimas y bytes y avanza la historia de cada línea", () => {
    const a = nextLive(null, sample());
    expect(a.cpu).toEqual({ percent: 64, temp: 62, history: [64] });
    expect(a.ram.used).toBe(17.1 * GIB);
    expect(a.ram.history[0]).toBeCloseTo(71.25);
    expect(a.gpu).toMatchObject({ percent: 97, vramUsed: 14.1 * GIB, vramTotal: 16 * GIB, temp: 71, history: [97] });
    const b = nextLive(a, sample({ cpu: { load: 125, cores: [], temperature: null } }));
    expect(b.cpu).toEqual({ percent: 12.5, temp: null, history: [64, 12.5] });
  });

  it("la historia nunca pasa de HISTORY muestras", () => {
    let live: LiveView | null = null;
    for (let i = 0; i < HISTORY * 3; i++) live = nextLive(live, sample({ cpu: { load: i, cores: [], temperature: null } }));
    expect(live?.cpu.history.length).toBe(HISTORY);
    expect(live?.cpu.history.at(-1)).toBe((HISTORY * 3 - 1) / 10);
  });

  it("sin GPU no hay grupo; sin lectura de uso la línea no avanza", () => {
    expect(nextLive(null, sample({ gpus: [] })).gpu).toBeNull();
    const g = nextLive(null, sample({ gpus: [{ name: "M5", load: null, vram_used: null, vram_total: null, temperature: null }] })).gpu;
    expect(g).toMatchObject({ percent: null, history: [] });
  });

  it("el notice de VRAM aparece desde el 85 %", () => {
    expect(vramWarn(nextLive(null, sample()))).toBe(true);
    expect(vramWarn(nextLive(null, sample({ gpus: [{ name: "x", load: 1, vram_used: 8 * GIB, vram_total: 16 * GIB, temperature: null }] })))).toBe(false);
    expect(vramWarn(null)).toBe(false);
  });

  it("el resumen del Desk y la tray sale de la misma vista", () => {
    const r = summaryOf(nextLive(null, sample()));
    expect(r.cpu).toEqual({ percent: 64, label: "64 %" });
    expect(r.ram.label).toBe("17.1/24 GB");
    expect(r.gpuLabel).toBe("VRAM");
    expect(r.gpu?.label).toBe("14.1/16 GB");
    const mac = summaryOf(nextLive(null, sample({ gpus: [{ name: "M5", load: 60, vram_used: null, vram_total: null, temperature: null }] })));
    expect([mac.gpuLabel, mac.gpu?.label]).toEqual(["GPU", "6 %"]);
  });
});

describe("fila «Permisos»", () => {
  it("muestra primero el que deja sin teclado", () => {
    expect(missingPermission(["accessibility", "input_monitoring"])?.id).toBe("input_monitoring");
    expect(missingPermission(["local_network", "accessibility"])?.text).toContain("“Accessibility”");
    expect(missingPermission([])).toBeNull();
  });
});

describe("nombre de la machine", () => {
  it("se recorta, no queda vacío y no pasa de 40 caracteres", () => {
    expect(checkName("  Studio ")).toEqual({ ok: true, name: "Studio" });
    expect(checkName("   ").ok).toBe(false);
    expect(checkName("x".repeat(41)).ok).toBe(false);
    expect(checkName("ñ".repeat(40)).ok).toBe(true);
  });
});
