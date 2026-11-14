import { createRoot } from "react-dom/client";
import { App } from "./app";
import { ContextMenuProvider } from "./context-menu";
import "./style.css";

createRoot(document.getElementById("root")!).render(<ContextMenuProvider><App /></ContextMenuProvider>);
