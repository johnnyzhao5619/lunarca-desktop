import { createRoot } from "react-dom/client";
import "./styles/index.css";
import { language } from "./i18n/i18n";
import { installTrustedTypesPolicy } from "./lib/trusted-types";
import { App } from "./app/App";

document.documentElement.lang = language;
installTrustedTypesPolicy();

const root = document.getElementById("root");
if (root) createRoot(root).render(<App />);
