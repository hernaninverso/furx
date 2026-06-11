// 047 FR-006 — ExtensionsView: una sola vista con tabs (Plugins / Skills) que fusiona los dos
// marketplaces. Test L2 (RTL + mockIPC): el tablist es accesible, arranca en la tab pedida, y
// al cambiar de tab cambia el panel + los atributos aria. PluginsView/ToolsView leen del backend
// al montar (skill_list / plugins_*) → mockeamos invoke con respuestas vacías (estado válido).
import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks";
import { ExtensionsView } from "../../views/ExtensionsView";

// Backend mock fail-soft: respondemos con la FORMA correcta por comando (no un []/0 ciego) para que
// ambos sub-paneles (PluginsView/ToolsView) monten su estado vacío sin reventar al destructurar.
function mockEmptyBackend() {
  mockIPC((cmd) => {
    switch (cmd) {
      case "skill_list":
        return [];
      case "skills_trust_list":
        return [[], false]; // [SkillTrustRow[], boolean]
      case "skills_discover_local":
        return [];
      case "plugin_list_bundled":
        return [];
      case "plugins_list":
        return [];
      default:
        // Cualquier otro comando de mount no previsto → lista vacía (fail-soft).
        return [];
    }
  });
}

describe("ExtensionsView (047 FR-006)", () => {
  beforeEach(() => {
    clearMocks();
    mockEmptyBackend();
  });

  it("expone un tablist accesible con las 2 tabs", async () => {
    render(<ExtensionsView />);
    const tablist = await screen.findByRole("tablist", { name: "Extensiones" });
    const tabs = within(tablist).getAllByRole("tab");
    expect(tabs).toHaveLength(2);
    expect(tabs.map((t) => t.textContent)).toEqual(
      expect.arrayContaining([expect.stringContaining("Plugins"), expect.stringContaining("Skills")]),
    );
  });

  it("arranca en la tab por defecto (Plugins) y la marca aria-selected", async () => {
    render(<ExtensionsView />);
    const plugins = await screen.findByRole("tab", { name: /Plugins/ });
    const skills = screen.getByRole("tab", { name: /Skills/ });
    expect(plugins).toHaveAttribute("aria-selected", "true");
    expect(skills).toHaveAttribute("aria-selected", "false");
    // El tabpanel referencia a la tab activa (a11y: aria-labelledby → id de la tab).
    const panel = screen.getByRole("tabpanel");
    expect(panel).toHaveAttribute("aria-labelledby", "ext-tab-plugins");
  });

  it("respeta initialTab='skills' (deep-link furx://tools)", async () => {
    render(<ExtensionsView initialTab="skills" />);
    const skills = await screen.findByRole("tab", { name: /Skills/ });
    expect(skills).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tabpanel")).toHaveAttribute("aria-labelledby", "ext-tab-skills");
  });

  it("cambiar de tab por clic actualiza la selección y el panel", async () => {
    const user = userEvent.setup();
    render(<ExtensionsView />);
    const skills = await screen.findByRole("tab", { name: /Skills/ });
    await user.click(skills);
    await waitFor(() => expect(skills).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("tab", { name: /Plugins/ })).toHaveAttribute("aria-selected", "false");
    expect(screen.getByRole("tabpanel")).toHaveAttribute("aria-labelledby", "ext-tab-skills");
  });
});
