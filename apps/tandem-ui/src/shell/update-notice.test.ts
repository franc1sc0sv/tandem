import { render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { describe, expect, it } from "vitest";

import { STORE_KEY } from "../lib/context";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { Store } from "../lib/store/store.svelte";
import { ADMIN_TEXT } from "../lib/updates";
import Shell from "./Shell.svelte";

async function open(query: string, hash = "#/desk") {
  const store = new Store(new DemoSource(demoOptions(query)), { hash });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

const notice = () => screen.findByRole("region", { name: "Update" });

describe("\"Update available\" notice (T-348)", () => {
  it("does not show while there is nothing new", async () => {
    await open("demo&still");
    await tick();
    expect(screen.queryByRole("region", { name: "Update" })).toBeNull();
    expect(screen.queryByText(/Update available/)).toBeNull();
  });

  it("shows the version and one button that runs \"Update all\", then its progress", async () => {
    await open("demo&still&update=available");
    const box = await notice();
    expect(within(box).getByText("Update available: v0.3.1")).toBeInTheDocument();
    expect(within(box).getByRole("status")).toHaveTextContent("It installs on every machine, one at a time, without cutting the keyboard.");
    const buttons = within(box).getAllByRole("button");
    expect(buttons).toHaveLength(1);
    await userEvent.click(within(box).getByRole("button", { name: /Update all/ }));
    expect(await within(box).findByText("Asking the other machine to update…")).toBeInTheDocument();
    expect(within(box).getByRole("button", { name: /Updating…/ })).toBeDisabled();
  });

  it("a failed update says why and offers to try again", async () => {
    await open("demo&still&update=fail");
    const box = await notice();
    await userEvent.click(within(box).getByRole("button", { name: /Update all/ }));
    expect(await within(box).findByText(/^The update did not finish: the other machine did not come back with 0.3.1/)).toBeInTheDocument();
    expect(within(box).getByRole("button", { name: /Try again/ })).toBeEnabled();
  });

  it("on a PC under Program Files it says the update asks for administrator once", async () => {
    await open("demo&still&update=admin");
    const box = await notice();
    expect(within(box).getByRole("status")).toHaveTextContent(ADMIN_TEXT);
  });

  it("after the restart, the result: updated, and both machines on the new version", async () => {
    await open("demo&still&update=updated");
    expect(await screen.findByText("Updated to v0.3.1")).toBeInTheDocument();
    expect(screen.getByText("From v0.3.0. Both machines are on v0.3.1.")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Update" })).toBeNull();
  });

  it("Settings keeps the full row (no second button) and sees the run started from the notice", async () => {
    const store = await open("demo&still&update=available");
    const box = await notice();
    await userEvent.click(within(box).getByRole("button", { name: /Update all/ }));
    await within(box).findByText("Asking the other machine to update…");
    store.go({ screen: "settings" });
    await tick();
    expect(screen.queryByRole("region", { name: "Update" })).toBeNull();
    await userEvent.click(screen.getByRole("tab", { name: "Updates" }));
    expect(await screen.findByText("Asking the other machine to update…")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /Update all/ })).toHaveLength(1);
    expect(screen.getByRole("button", { name: /Update all/ })).toBeDisabled();
  });

  it("the tray window does not repeat the result", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still&update=updated")), null, ["machines"]);
    await store.start();
    await tick();
    expect(store.toasts.find((t) => t.text === "Updated to v0.3.1")).toBeUndefined();
  });
});
