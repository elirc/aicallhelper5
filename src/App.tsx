import { AppProvider } from "./app/AppProvider";
import { Root } from "./components/Root";
import type { CoreApi } from "./ipc/types";

export function App({ api }: { api: CoreApi }) {
  return (
    <AppProvider api={api}>
      <Root />
    </AppProvider>
  );
}
