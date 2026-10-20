const UNITS = ["B", "KB", "MB", "GB", "TB"]

/** Formats a byte count with binary units, e.g. "1.4 GB". */
export function formatBytes(bytes: number | null | undefined, digits = 1): string {
  if (bytes === null || bytes === undefined || Number.isNaN(bytes)) return "-"
  if (bytes < 1024) return `${bytes} B`
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024
    unit++
  }
  return `${value.toFixed(value >= 100 ? 0 : digits)} ${UNITS[unit]}`
}

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" })
const dateTimeFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" })
const relativeFormat = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" })
const numberFormat = new Intl.NumberFormat()
const compactFormat = new Intl.NumberFormat(undefined, { notation: "compact" })

export function formatDate(ms: number | null | undefined): string {
  return ms ? dateFormat.format(ms) : "-"
}

export function formatDateTime(ms: number | null | undefined): string {
  return ms ? dateTimeFormat.format(ms) : "-"
}

/** "2 days ago", "in 3 hours", falling back to a date past a month. */
export function formatRelative(ms: number | null | undefined): string {
  if (!ms) return "-"
  const diff = ms - Date.now()
  const abs = Math.abs(diff)
  const minute = 60_000
  const hour = 60 * minute
  const day = 24 * hour
  if (abs < minute) return "just now"
  if (abs < hour) return relativeFormat.format(Math.round(diff / minute), "minute")
  if (abs < day) return relativeFormat.format(Math.round(diff / hour), "hour")
  if (abs < 30 * day) return relativeFormat.format(Math.round(diff / day), "day")
  return dateFormat.format(ms)
}

export function formatNumber(n: number): string {
  return numberFormat.format(n)
}

export function formatCompact(n: number): string {
  return compactFormat.format(n)
}

/** Human label for a task progress stage. */
export function stageLabel(stage: string): string {
  const labels: Record<string, string> = {
    upload: "Uploading",
    install: "Installing",
    download: "Downloading",
    license: "Requesting license",
    sign: "Signing",
    prepare: "Preparing",
    backup: "Backing up",
    restore: "Restoring",
    extract: "Extracting",
    export: "Exporting",
    send: "Sending",
    package: "Packaging",
  }
  return labels[stage] ?? stage.charAt(0).toUpperCase() + stage.slice(1)
}
