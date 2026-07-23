import ReactDOM from "react-dom/client";
import { AppBootstrap, RecoveryBoundary } from "./bootstrap";
import { ThemeProvider } from "./components/ThemeProvider";
import { installGlobalDiagnostics } from "./diagnostics";

installGlobalDiagnostics();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <RecoveryBoundary>
      <ThemeProvider>
        <AppBootstrap />
      </ThemeProvider>
    </RecoveryBoundary>,
);
