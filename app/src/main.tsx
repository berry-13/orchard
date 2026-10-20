import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { StrictMode } from "react"
import { createRoot } from "react-dom/client"

import "./index.css"
import { App } from "@/App"
import { RemoteGate } from "@/components/connect-server"
import { initBackend } from "@/lib/api"
import { applyWindowMaterial } from "@/lib/platform"
import { ThemeProvider } from "@/lib/theme"

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { refetchOnWindowFocus: false, retry: 1, staleTime: 30_000 },
  },
})

// Pick the backend and learn the window material first, so the first paint
// already talks to the right place and shows the material.
void initBackend()
  .then(applyWindowMaterial)
  .finally(() =>
    createRoot(document.getElementById("root")!).render(
      <StrictMode>
        <ThemeProvider>
          <QueryClientProvider client={queryClient}>
            <RemoteGate>
              <App />
            </RemoteGate>
          </QueryClientProvider>
        </ThemeProvider>
      </StrictMode>,
    ),
  )
