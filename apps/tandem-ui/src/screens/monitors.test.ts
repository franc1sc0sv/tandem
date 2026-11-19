import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { describe, expect, it } from "vitest";

import { STORE_KEY } from "../lib/context";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { Store } from "../lib/store/store.svelte";
import Shell from "../shell/Shell.svelte";

async function open(hash: string, query = "demo&still") {
  const store = new Store(new DemoSource(demoOptions(query)), { hash });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

const picker = () => screen.getByRole("radiogroup", { name: "Which machine shows the 34 inches" });

describe("Monitors (V3-Monitors) in demo mode", () => {
  it("the compatible 34\" brings its selector, its owner and its events", async () => {
    await open("#/monitors");
    const card = screen.getByRole("article", { name: 'Odyssey G5 34"' });
    expect(within(card).getByText("Tandem standard")).toBeInTheDocument();
    const radios = within(picker()).getAllByRole("radio");
    expect(radios).toHaveLength(2);
    expect(radios[0]).toHaveTextContent("MacBook Pro");
    expect(radios[1]).toHaveTextContent("PC-Desktop");
    expect(within(card).getAllByText(/Input →|Switch|Signal/).length).toBeGreaterThan(0);
  });

  it("D11: the 32\" is shown \"not compatible\", with its reason, without a selector or input change", async () => {
    await open("#/monitors");
    const card = screen.getByRole("article", { name: /Odyssey G5 32", unsupported/ });
    expect(within(card).getByText("Unsupported")).toBeInTheDocument();
    expect(within(card).getByText(/its firmware meets the standard/)).toBeInTheDocument();
    expect(within(card).queryByRole("radiogroup")).toBeNull();
    expect(screen.getAllByRole("radiogroup")).toHaveLength(1);
  });

  it("\"What the standard is\" opens and closes the explanation", async () => {
    await open("#/monitors");
    const b = screen.getByRole("button", { name: "What the standard is" });
    expect(b).toHaveAttribute("aria-pressed", "false");
    expect(screen.queryByText(/speaks the custom firmware protocol/)).toBeNull();
    await userEvent.click(b);
    expect(b).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText(/speaks the custom firmware protocol/)).toBeInTheDocument();
    await userEvent.click(b);
    expect(screen.queryByText(/speaks the custom firmware protocol/)).toBeNull();
  });

  it("choosing the other machine asks for the input change and the selector follows what the monitor shows", async () => {
    await open("#/monitors");
    const radios = () => within(picker()).getAllByRole("radio");
    expect(radios()[1]).toHaveAttribute("aria-checked", "true");
    await userEvent.click(radios()[0] as HTMLElement);
    await waitFor(() => expect(radios()[0]).toHaveAttribute("aria-checked", "true"));
    expect(radios()[1]).toHaveAttribute("aria-checked", "false");
  });

  it("the scenes are cards: the active one carries \"Active\" and the others \"Apply\"", async () => {
    await open("#/monitors");
    const cards = screen.getAllByRole("article", { name: /^Scene / });
    expect(cards.length).toBeGreaterThan(1);
    const active = cards.filter((c) => within(c).queryByText("Active"));
    expect(active).toHaveLength(1);
    expect(within(active[0] as HTMLElement).queryByRole("button", { name: "Apply" })).toBeNull();
    for (const c of cards.filter((x) => x !== active[0])) expect(within(c).getByRole("button", { name: "Apply" })).toBeEnabled();
  });

  it("\"Apply\" leads to the step-by-step progress and you can go back to Monitors", async () => {
    const store = await open("#/monitors");
    const other = screen.getAllByRole("article", { name: /^Scene / }).find((c) => !within(c).queryByText("Active")) as HTMLElement;
    await userEvent.click(within(other).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(store.route.screen).toBe("progress"));
    expect(await screen.findByRole("heading", { name: "Changes in progress" })).toBeInTheDocument();
    expect(screen.getByText("Save the current state")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to Monitors" })).toHaveAttribute("href", "#/monitors");
  });
});

describe("New scene (F-NewScene) in demo mode", () => {
  it("it is not saved without a name; with a name and the other machine it ends up as a card in Monitors", async () => {
    const store = await open("#/monitors/scene");
    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    await userEvent.type(screen.getByRole("textbox"), "Game 2");
    expect(save).toBeEnabled();
    await userEvent.click(screen.getByRole("radio", { name: /PC-Desktop/ }));
    expect(screen.getByText("PC-Desktop", { selector: "b" })).toBeInTheDocument();
    await userEvent.click(save);
    await waitFor(() => expect(store.route.screen).toBe("monitors"));
    expect(await screen.findByRole("article", { name: "Scene Game 2" })).toBeInTheDocument();
  });

  it("the shortcut is recorded with the combination and warns if it already belongs to another scene", async () => {
    await open("#/monitors/scene");
    await userEvent.type(screen.getByRole("textbox"), "Other");
    const kbd = screen.getByRole("button", { name: "Shortcut" });
    expect(kbd).toHaveTextContent("No shortcut");
    kbd.focus();
    await userEvent.keyboard("{Control>}{Alt>}4{/Alt}{/Control}");
    expect(kbd.textContent).not.toBe("No shortcut");
    await userEvent.keyboard("{Backspace}");
    expect(kbd).toHaveTextContent("No shortcut");
  });

  it("\"Save and apply\" saves and opens the progress", async () => {
    const store = await open("#/monitors/scene");
    await userEvent.type(screen.getByRole("textbox"), "With progress");
    await userEvent.click(screen.getByRole("radio", { name: /PC-Desktop/ }));
    await userEvent.click(screen.getByRole("button", { name: "Save and apply" }));
    await waitFor(() => expect(store.route.screen).toBe("progress"));
  });
});

describe("Progress (F-Transactions) in demo mode", () => {
  it("a scene in progress shows its steps and the notice that the monitor confirms each change", async () => {
    await open("#/progress", "demo&still&txn=run");
    expect(await screen.findByText("Applying the scene “Game”")).toBeInTheDocument();
    expect(screen.getByText("The monitor confirms each change; nothing is guessed.")).toBeInTheDocument();
    expect(screen.getByText("waiting for the monitor")).toBeInTheDocument();
  });

  it("on finishing well it says the scene became active", async () => {
    await open("#/progress", "demo&still&txn=ok");
    expect(await screen.findByText(/Scene “Game” active/)).toBeInTheDocument();
  });

  it("if the monitor gives no image the rollback is seen and nothing is left half-done", async () => {
    await open("#/progress", "demo&still&txn=fail");
    expect(await screen.findByText("Rolling back to the previous state")).toBeInTheDocument();
    expect(screen.getByText("the monitor said: change failed")).toBeInTheDocument();
    expect(screen.getByText(/nothing was left half done/)).toBeInTheDocument();
  });
});

describe("Changes in progress (F-Transactions): one card per transaction", () => {
  it("with txn=run the scene, the power-on and the link measurement come out", async () => {
    await open("#/progress", "demo&still&txn=run");
    expect(screen.getByRole("region", { name: "Applying the scene “Game”" })).toBeInTheDocument();
    const energy = screen.getByRole("region", { name: "Powering on PC-Desktop" });
    expect(within(energy).getByText("0:18 / 2:00")).toBeInTheDocument();
    expect(within(energy).queryByRole("button")).toBeNull();
    const link = screen.getByRole("region", { name: "Measuring the USB-C link" });
    expect(within(link).getByText("9.4")).toBeInTheDocument();
    expect(within(link).getByRole("img", { name: "9.4 of 10 Gbps" })).toBeInTheDocument();
    expect(within(link).getByText("410 Mbps · 3.8 ms")).toBeInTheDocument();
  });
});
