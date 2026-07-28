import ReactDOM from "react-dom/client";
import "./index.css";
import { AppBootstrap, RecoveryBoundary } from "./bootstrap";
import { LanguageProvider } from "./components/LanguageProvider";
import { ThemeProvider } from "./components/ThemeProvider";
import { installGlobalDiagnostics } from "./diagnostics";

installGlobalDiagnostics();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <LanguageProvider>
      <RecoveryBoundary>
        <ThemeProvider>
          <AppBootstrap />
        </ThemeProvider>
      </RecoveryBoundary>
    </LanguageProvider>,
);
