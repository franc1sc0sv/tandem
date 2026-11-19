import { fireEvent, render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import Bar from "./Bar.svelte";
import Button from "./Button.svelte";
import Card from "./Card.svelte";
import Chip from "./Chip.svelte";
import Icon from "./Icon.svelte";
import { ICONS } from "./icons";
import Ring from "./Ring.svelte";
import Segmented from "./Segmented.svelte";
import Sparkline, { sparkPoints } from "./Sparkline.svelte";
import Switch from "./Switch.svelte";
import { text } from "../test/snippet";

describe("Button", () => {
  it("siempre lleva ícono y el texto; el alto sale del tamaño", () => {
    render(Button, { icon: "check", size: "sm", children: text("Save") });
    const b = screen.getByRole("button", { name: "Save" });
    expect(b.querySelector("svg")).not.toBeNull();
    expect(b).toHaveClass("btn", "sm");
  });

  it("variantes pri, ghost y danger", () => {
    for (const variant of ["pri", "ghost", "danger"] as const) {
      const { unmount } = render(Button, { icon: "check", variant, children: text(variant) });
      expect(screen.getByRole("button", { name: variant })).toHaveClass(variant);
      unmount();
    }
  });

  it("solo ícono: nombre accesible y título, sin texto", () => {
    render(Button, { icon: "edit", iconOnly: true, label: "Edit arrangement", children: text("no se ve") });
    const b = screen.getByRole("button", { name: "Edit arrangement" });
    expect(b).toHaveClass("ic");
    expect(b).toHaveAttribute("title", "Edit arrangement");
    expect(b.textContent).toBe("");
  });

  it("se activa con clic, Enter y Espacio, y recibe el foco con Tab", async () => {
    const user = userEvent.setup();
    const onclick = vi.fn();
    render(Button, { icon: "play", onclick, children: text("Apply") });
    await user.tab();
    const b = screen.getByRole("button", { name: "Apply" });
    expect(b).toHaveFocus();
    await user.keyboard("{Enter}");
    await user.keyboard(" ");
    await user.click(b);
    expect(onclick).toHaveBeenCalledTimes(3);
  });

  it("deshabilitado no hace nada", async () => {
    const onclick = vi.fn();
    render(Button, { icon: "play", disabled: true, onclick, children: text("Apply") });
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onclick).not.toHaveBeenCalled();
  });

  it("con href es un link con el mismo aspecto", () => {
    render(Button, { icon: "arrow-right", href: "#/network", children: text("Ver network") });
    const a = screen.getByRole("link", { name: "Ver network" });
    expect(a).toHaveAttribute("href", "#/network");
    expect(a).toHaveClass("btn");
  });

  it("el ícono puede ir después y moverse distinto al pasar", () => {
    render(Button, { icon: "arrow-right", iconAfter: true, motion: "go", children: text("Next") });
    const b = screen.getByRole("button", { name: "Next" });
    expect(b.lastElementChild?.tagName.toLowerCase()).toBe("svg");
    expect(b).toHaveClass("m-go");
  });
});

describe("Segmented", () => {
  const options = [
    { value: "izq", label: "Left" },
    { value: "cen", label: "Center" },
    { value: "der", label: "Right" },
  ];

  it("es un grupo de radio con la opción elegida marcada", () => {
    render(Segmented, { options, value: "cen", label: "Alignment" });
    expect(screen.getByRole("radiogroup", { name: "Alignment" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Center" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "Left" })).toHaveAttribute("aria-checked", "false");
  });

  it("el indicador se desliza a la opción elegida", async () => {
    const onchange = vi.fn();
    const { container } = render(Segmented, { options, value: "izq", label: "Alignment", onchange });
    const ind = container.querySelector(".ind") as HTMLElement;
    expect(ind.style.transform).toBe("translateX(0%)");
    await userEvent.click(screen.getByRole("radio", { name: "Right" }));
    expect(onchange).toHaveBeenCalledWith("der");
    expect(ind.style.transform).toBe("translateX(200%)");
  });

  it("flechas, Inicio y Fin mueven la elección y el foco; solo la elegida está en el orden de Tab", async () => {
    const onchange = vi.fn();
    render(Segmented, { options, value: "izq", label: "Alignment", onchange });
    const izq = screen.getByRole("radio", { name: "Left" });
    expect(izq).toHaveAttribute("tabindex", "0");
    expect(screen.getByRole("radio", { name: "Center" })).toHaveAttribute("tabindex", "-1");
    izq.focus();
    await fireEvent.keyDown(izq, { key: "ArrowRight" });
    expect(screen.getByRole("radio", { name: "Center" })).toHaveFocus();
    await fireEvent.keyDown(document.activeElement as Element, { key: "End" });
    expect(screen.getByRole("radio", { name: "Right" })).toHaveAttribute("aria-checked", "true");
    await fireEvent.keyDown(document.activeElement as Element, { key: "ArrowRight" });
    expect(screen.getByRole("radio", { name: "Left" })).toHaveAttribute("aria-checked", "true");
    await fireEvent.keyDown(document.activeElement as Element, { key: "ArrowLeft" });
    await fireEvent.keyDown(document.activeElement as Element, { key: "Home" });
    expect(onchange.mock.calls.map((c) => c[0])).toEqual(["cen", "der", "izq", "der", "izq"]);
  });

  it("el indicador toma el color de la opción si lo tiene", () => {
    const { container } = render(Segmented, {
      options: [
        { value: "a", label: "A", accent: "#4f7cbc" },
        { value: "b", label: "B", accent: "#6a5fa6" },
      ],
      value: "b",
      label: "Machine",
      size: "lg",
    });
    const ind = container.querySelector(".ind") as HTMLElement;
    expect(ind.style.borderColor).toBe("rgb(106, 95, 166)");
    expect(container.querySelector(".seg")).toHaveClass("lg");
  });
});

describe("Switch", () => {
  it("cambia con clic y con el teclado, y avisa", async () => {
    const user = userEvent.setup();
    const onchange = vi.fn();
    render(Switch, { checked: true, label: "Shared clipboard", onchange });
    const sw = screen.getByRole("switch", { name: "Shared clipboard" });
    expect(sw).toHaveAttribute("aria-checked", "true");
    await user.click(sw);
    expect(sw).toHaveAttribute("aria-checked", "false");
    expect(sw).toHaveClass("off");
    sw.focus();
    await user.keyboard(" ");
    expect(sw).toHaveAttribute("aria-checked", "true");
    expect(onchange.mock.calls.map((c) => c[0])).toEqual([false, true]);
  });

  it("deshabilitado no cambia", async () => {
    const onchange = vi.fn();
    render(Switch, { checked: false, label: "Module", disabled: true, onchange });
    await userEvent.click(screen.getByRole("switch", { name: "Module" }));
    expect(onchange).not.toHaveBeenCalled();
  });
});

describe("Chip", () => {
  it("formato uniforme: punto y texto, con el tono como clase", () => {
    const { container } = render(Chip, { tone: "ok", children: text("activo") });
    const chip = container.querySelector(".chip") as HTMLElement;
    expect(chip).toHaveClass("ok");
    expect(chip.querySelector(".d")).not.toBeNull();
    expect(chip.textContent).toBe("activo");
  });

  it("el contador no lleva punto; el pulso marca lo vivo", () => {
    const { container } = render(Chip, { count: true, children: text("3") });
    expect(container.querySelector(".chip.count .d")).toBeNull();
    const live = render(Chip, { tone: "info", pulse: true, children: text("activo") });
    expect(live.container.querySelector(".chip")).toHaveClass("pulse");
  });

  it("con ícono en lugar de punto", () => {
    const { container } = render(Chip, { icon: "warn", tone: "warn", children: text("Unsupported") });
    expect(container.querySelector(".chip svg")).not.toBeNull();
    expect(container.querySelector(".chip .d")).toBeNull();
  });
});

describe("Card", () => {
  it("entra con retraso y no lleva línea de color arriba", () => {
    const { container } = render(Card, { delay: 0.12, children: text("hello") });
    const card = container.querySelector(".card") as HTMLElement;
    expect(card.style.animationDelay).toBe("0.12s");
    expect(card.style.borderTop).toBe("");
  });

  it("con href es un link que se eleva", () => {
    render(Card, { href: "#/machine/a", label: "Studio", children: text("x") });
    const a = screen.getByRole("link", { name: "Studio" });
    expect(a).toHaveClass("card", "lift");
  });
});

describe("íconos, anillos, barras y sparklines", () => {
  it("cada ícono del set se dibuja con el trazo pedido", () => {
    for (const name of Object.keys(ICONS) as (keyof typeof ICONS)[]) {
      const { container, unmount } = render(Icon, { name, stroke: 2 });
      const svg = container.querySelector("svg") as SVGElement;
      expect(svg.getAttribute("viewBox")).toBe("0 0 24 24");
      expect(svg.innerHTML.length).toBeGreaterThan(10);
      if (!("fill" in ICONS[name])) expect(svg.getAttribute("stroke-width")).toBe("2");
      unmount();
    }
  });

  it("el anillo marca la fracción", () => {
    const { container } = render(Ring, { value: 0.5, color: "#6cc59a", label: "CPU 50 %" });
    expect(screen.getByRole("img", { name: "CPU 50 %" })).toBeInTheDocument();
    expect(container.querySelector("circle.v")?.getAttribute("stroke-dashoffset")).toBe("57");
  });

  it("la barra se limita a 0–100", () => {
    const { container } = render(Bar, { percent: 140, color: "#7aa7e6", label: "RAM" });
    expect((container.querySelector(".bar span") as HTMLElement).style.width).toBe("100%");
    expect(screen.getByRole("meter", { name: "RAM" })).toHaveAttribute("aria-valuenow", "100");
  });

  it("la sparkline pone 0 abajo y 100 arriba, repartida a lo ancho", () => {
    expect(sparkPoints([0, 50, 100], 300, 54)).toEqual([
      [0, 54],
      [150, 27],
      [300, 0],
    ]);
    expect(sparkPoints([])).toEqual([]);
    const { container } = render(Sparkline, { values: [26, 30], color: "#7aa7e6" });
    expect(container.querySelector("path.ln")?.getAttribute("d")).toBe("M0 40 L300 38");
  });
});
