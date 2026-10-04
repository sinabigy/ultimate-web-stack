import { render } from "solid-js/web";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/components.css";
import "./styles/shell.css";
import { App } from "./app";
import { installTelemetry } from "./lib/telemetry";
import { applyTheme } from "./stores/theme";

applyTheme();
installTelemetry();
const root = document.getElementById("root");
if (root) render(() => <App />, root);
