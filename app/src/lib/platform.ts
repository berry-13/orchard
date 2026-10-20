import { appInfo, currentBackend } from "@/lib/api"

export type Platform = "macos" | "windows" | "linux"

const root = document.documentElement

/** Set by the inline script in index.html before the first paint. */
export const platform = (root.dataset.platform ?? "linux") as Platform

/** macOS draws its traffic lights over the top of the page instead of a title bar. */
export const overlayTitleBar = root.dataset.titlebar === "overlay"

/**
 * Asks the shell which native material (Liquid Glass, vibrancy or Mica) sits
 * behind the window. The UI turns transparent only once one is confirmed, so a
 * window whose material failed stays opaque.
 */
export async function applyWindowMaterial() {
  if (currentBackend() !== "desktop" || platform === "linux") return
  try {
    const { material } = await appInfo()
    if (material) root.dataset.material = material
  } catch {
    // Keep the opaque look.
  }
}
