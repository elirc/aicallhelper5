import "../styles/index.css";
import { useApp } from "../app/view";
import { FullLayout } from "./full/FullLayout";
import { PrompterLayout } from "./prompter/PrompterLayout";
import { SettingsScreen } from "./settings/SettingsScreen";

/** Picks Settings vs Full vs Prompter from the view. */
export function Root() {
  const { view } = useApp();
  if (view.screen === "settings") return <SettingsScreen />;
  if (view.layout === "prompter") return <PrompterLayout />;
  return <FullLayout />;
}
