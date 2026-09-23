import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { createTauriApi } from "./ipc/tauri";
import type { CoreApi } from "./ipc/types";

/** Inside Tauri use the real core; in a plain browser (`npm run dev`) use the
 * scripted fake core so the UI can be exercised without the Rust side. */
async function pickApi(): Promise<CoreApi> {
  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) return createTauriApi();
  const { createFakeApi } = await import("./ipc/fake");
  return createFakeApi({ demo: true });
}

const el = document.getElementById("root");
if (el) {
  void pickApi().then((api) => {
    createRoot(el).render(
      <StrictMode>
        <App api={api} />
      </StrictMode>,
    );
  });
}
