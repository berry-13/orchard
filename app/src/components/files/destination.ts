import { getSettings, pickFolder } from "@/lib/api"

/** The default download folder from settings, or a folder picked now. */
export async function downloadDestination(title: string): Promise<string | null> {
  try {
    const settings = await getSettings()
    if (settings.downloadDir) return settings.downloadDir
  } catch {
    // Fall back to asking.
  }
  return pickFolder(title)
}
