/**
 * In-browser stand-in for the Tauri backend. Used automatically when the app
 * runs outside Tauri (`npm run dev` in a browser) so every screen works with
 * realistic data and simulated progress.
 */
import type {
  BackupDomain,
  BackupFile,
  BackupSummary,
  CommandArgs,
  CommandName,
  CommandResult,
  Device,
  DeviceInfo,
  FileEntry,
  FileSource,
  InstalledApp,
  MediaCategory,
  ProfileInfo,
  ResignIssue,
  SigningIdentityInfo,
  MediaFile,
  MediaTarget,
  PhotoItem,
  StoreAccount,
  StoreApp,
  SyslogLevel,
  SyslogLine,
  TaskEvent,
} from "@/lib/api"

/* ------------------------------------------------------------------ */
/* Event bus                                                           */
/* ------------------------------------------------------------------ */

type Listener = (payload: unknown) => void
const listeners = new Map<string, Set<Listener>>()

function emit(event: string, payload: unknown) {
  listeners.get(event)?.forEach((l) => l(payload))
}

export function mockListen(event: string, handler: Listener): Promise<() => void> {
  let set = listeners.get(event)
  if (!set) {
    set = new Set()
    listeners.set(event, set)
  }
  set.add(handler)
  return Promise.resolve(() => {
    listeners.get(event)?.delete(handler)
  })
}

/* ------------------------------------------------------------------ */
/* Helpers                                                             */
/* ------------------------------------------------------------------ */

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))
const latency = () => sleep(180 + Math.random() * 320)

function fail(kind: string, message: string): never {
  throw { kind, message }
}

const GB = 1024 ** 3
const MB = 1024 ** 2
const DAY = 86_400_000
const now = Date.now()

const PALETTE = [
  ["#0ea5e9", "#2563eb"],
  ["#f97316", "#dc2626"],
  ["#22c55e", "#15803d"],
  ["#a855f7", "#6d28d9"],
  ["#f43f5e", "#be123c"],
  ["#eab308", "#ca8a04"],
  ["#14b8a6", "#0f766e"],
  ["#64748b", "#334155"],
  ["#ec4899", "#9d174d"],
  ["#6366f1", "#3730a3"],
]

function hash(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0
  return Math.abs(h)
}

/** A generated rounded-square icon with the app's initial. */
export function iconFor(name: string): string {
  const [a, b] = PALETTE[hash(name) % PALETTE.length]
  const letter = name.trim().charAt(0).toUpperCase()
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="120" height="120" viewBox="0 0 120 120"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${a}"/><stop offset="1" stop-color="${b}"/></linearGradient></defs><rect width="120" height="120" rx="27" fill="url(#g)"/><text x="60" y="78" font-family="Helvetica,Arial,sans-serif" font-size="54" font-weight="600" fill="#fff" text-anchor="middle">${letter}</text></svg>`
  return `data:image/svg+xml;base64,${btoa(svg)}`
}

/** Simulates a long running task, emitting `orchard://task` events. */
async function runTask(
  taskId: string,
  stages: { stage: string; total: number | null; ms: number }[],
  finalMessage?: string,
) {
  cancelled.delete(taskId)
  for (const { stage, total, ms } of stages) {
    const steps = Math.max(4, Math.round(ms / 120))
    for (let i = 0; i <= steps; i++) {
      if (cancelled.has(taskId)) {
        emit("orchard://task", {
          taskId,
          stage,
          done: 0,
          total,
          status: "error",
          message: "Cancelled",
        } satisfies TaskEvent)
        fail("cancelled", "Cancelled")
      }
      const done = total === null ? i : Math.round((total * i) / steps)
      emit("orchard://task", { taskId, stage, done, total, status: "running" } satisfies TaskEvent)
      await sleep(ms / steps)
    }
  }
  const last = stages[stages.length - 1]
  emit("orchard://task", {
    taskId,
    stage: last.stage,
    done: last.total ?? 0,
    total: last.total,
    status: "done",
    message: finalMessage ?? null,
  } satisfies TaskEvent)
}

const cancelled = new Set<string>()

/* ------------------------------------------------------------------ */
/* Devices                                                             */
/* ------------------------------------------------------------------ */

const devices: DeviceInfo[] = [
  {
    udid: "00008130-001A2C3E0E38001C",
    name: "Marco's iPhone",
    model: "iPhone 15 Pro",
    productType: "iPhone16,1",
    iosVersion: "18.6.2",
    buildVersion: "22G100",
    serialNumber: "F4GZK2LQ0D",
    connection: "usb",
    paired: true,
    batteryLevel: 82,
    batteryCharging: true,
    storageTotal: 256 * GB,
    storageUsed: 171.4 * GB,
    wifiSyncEnabled: true,
  },
  {
    udid: "00008103-000E4D9A1E08801E",
    name: "Studio iPad",
    model: "iPad Air (5th generation)",
    productType: "iPad13,16",
    iosVersion: "17.7.1",
    buildVersion: "21H216",
    serialNumber: "DMPHX9K1Q1GC",
    connection: "wifi",
    paired: true,
    batteryLevel: 46,
    batteryCharging: false,
    storageTotal: 64 * GB,
    storageUsed: 39.2 * GB,
    wifiSyncEnabled: true,
  },
]

function summary(d: DeviceInfo): Device {
  return {
    udid: d.udid,
    name: d.name,
    model: d.model,
    productType: d.productType,
    iosVersion: d.iosVersion,
    connection: d.connection,
    paired: d.paired,
  }
}

function device(udid: string): DeviceInfo {
  return devices.find((d) => d.udid === udid) ?? fail("notFound", `device ${udid}`)
}

/* ------------------------------------------------------------------ */
/* Apps                                                                */
/* ------------------------------------------------------------------ */

const userApps: [string, string, string, number, boolean?][] = [
  ["com.burbn.instagram", "Instagram", "352.0", 412 * MB],
  ["com.spotify.client", "Spotify", "9.0.42", 288 * MB],
  ["net.whatsapp.WhatsApp", "WhatsApp", "25.18.76", 1.9 * GB, true],
  ["com.google.chrome.ios", "Chrome", "138.0.7204.156", 241 * MB],
  ["com.atebits.Tweetie2", "X", "11.2", 356 * MB],
  ["com.getdropbox.Dropbox", "Dropbox", "441.2", 198 * MB, true],
  ["com.readdle.PDFExpert5", "PDF Expert", "8.4.1", 286 * MB, true],
  ["com.microsoft.Office.Word", "Microsoft Word", "2.95", 512 * MB, true],
  ["com.duolingo.DuolingoMobile", "Duolingo", "7.82.0", 402 * MB],
  ["com.netflix.Netflix", "Netflix", "18.21.0", 1.2 * GB],
  ["org.videolan.vlc-ios", "VLC", "3.6.4", 98 * MB, true],
  ["com.toyopagroup.picaboo", "Snapchat", "13.48.0", 732 * MB],
  ["com.shazam.Shazam", "Shazam", "18.5", 76 * MB],
  ["io.robbie.HomeAssistant", "Home Assistant", "2025.7", 64 * MB],
  ["com.agilebits.onepassword-ios", "1Password", "8.11.2", 155 * MB],
  ["com.getsignal.Signal", "Signal", "7.62", 214 * MB],
  ["com.google.Maps", "Google Maps", "25.29.0", 389 * MB],
  ["md.obsidian", "Obsidian", "1.9.4", 112 * MB, true],
]

const systemApps: [string, string][] = [
  ["com.apple.mobilesafari", "Safari"],
  ["com.apple.mobilemail", "Mail"],
  ["com.apple.camera", "Camera"],
  ["com.apple.mobileslideshow", "Photos"],
  ["com.apple.Maps", "Maps"],
  ["com.apple.weather", "Weather"],
  ["com.apple.mobilenotes", "Notes"],
  ["com.apple.reminders", "Reminders"],
  ["com.apple.Music", "Music"],
  ["com.apple.Preferences", "Settings"],
]

const appsByDevice = new Map<string, InstalledApp[]>()

function apps(udid: string): InstalledApp[] {
  let list = appsByDevice.get(udid)
  if (!list) {
    const n = udid === devices[0].udid ? userApps.length : 9
    list = [
      ...userApps.slice(0, n).map(([bundleId, name, shortVersion, size, fileSharing]) => ({
        bundleId,
        name,
        shortVersion,
        version: String(hash(bundleId) % 9000 + 1000),
        size: Math.round(size),
        kind: "user" as const,
        fileSharing: fileSharing ?? false,
      })),
      ...systemApps.map(([bundleId, name]) => ({
        bundleId,
        name,
        shortVersion: device(udid).iosVersion,
        version: device(udid).buildVersion,
        size: null,
        kind: "system" as const,
        fileSharing: false,
      })),
    ]
    appsByDevice.set(udid, list)
  }
  return list
}

/* ------------------------------------------------------------------ */
/* App Store                                                           */
/* ------------------------------------------------------------------ */

let account: StoreAccount | null = null
const purchased = new Set<number>([1, 2, 3, 5, 8])

const catalog: Omit<StoreApp, "artworkUrl" | "appId" | "description">[] = [
  { bundleId: "com.openai.chat", name: "ChatGPT", seller: "OpenAI OpCo, LLC", price: 0, formattedPrice: "Free", version: "1.2025.210", averageRating: 4.8, ratingCount: 2_134_002, genre: "Productivity", size: 104 * MB, minimumOsVersion: "17.0" },
  { bundleId: "com.spotify.client", name: "Spotify: Music and Podcasts", seller: "Spotify", price: 0, formattedPrice: "Free", version: "9.0.42", averageRating: 4.8, ratingCount: 31_221_870, genre: "Music", size: 288 * MB, minimumOsVersion: "16.0" },
  { bundleId: "com.toyopagroup.picaboo", name: "Snapchat", seller: "Snap, Inc.", price: 0, formattedPrice: "Free", version: "13.48.0", averageRating: 4.6, ratingCount: 6_200_441, genre: "Photo & Video", size: 732 * MB, minimumOsVersion: "15.0" },
  { bundleId: "com.lumafusion.LumaFusion", name: "LumaFusion", seller: "LumaTouch, LLC", price: 29.99, formattedPrice: "$29.99", version: "5.3.1", averageRating: 4.5, ratingCount: 13_380, genre: "Photo & Video", size: 412 * MB, minimumOsVersion: "16.0" },
  { bundleId: "com.readdle.Spark", name: "Spark Mail + AI Email Inbox", seller: "Readdle Inc.", price: 0, formattedPrice: "Free", version: "3.22.0", averageRating: 4.7, ratingCount: 88_120, genre: "Productivity", size: 231 * MB, minimumOsVersion: "16.0" },
  { bundleId: "com.ideasoncanvas.mindnode", name: "MindNode: Mind Map", seller: "IdeasOnCanvas GmbH", price: 0, formattedPrice: "Free", version: "2025.3", averageRating: 4.6, ratingCount: 4_512, genre: "Productivity", size: 87 * MB, minimumOsVersion: "17.0" },
  { bundleId: "com.pixelmatorteam.pixelmator.photo", name: "Pixelmator Photo", seller: "Pixelmator Team", price: 9.99, formattedPrice: "$9.99", version: "2.4", averageRating: 4.4, ratingCount: 2_840, genre: "Photo & Video", size: 380 * MB, minimumOsVersion: "16.0" },
  { bundleId: "com.halfbrick.FruitNinja", name: "Fruit Ninja Classic", seller: "Halfbrick Studios", price: 0, formattedPrice: "Free", version: "3.10.4", averageRating: 4.5, ratingCount: 410_002, genre: "Games", size: 190 * MB, minimumOsVersion: "13.0" },
  { bundleId: "com.flexibits.fantastical2.iphone", name: "Fantastical Calendar", seller: "Flexibits Inc.", price: 0, formattedPrice: "Free", version: "4.0.8", averageRating: 4.7, ratingCount: 61_009, genre: "Productivity", size: 150 * MB, minimumOsVersion: "17.0" },
  { bundleId: "com.google.Maps", name: "Google Maps", seller: "Google LLC", price: 0, formattedPrice: "Free", version: "25.29.0", averageRating: 4.7, ratingCount: 5_810_224, genre: "Navigation", size: 389 * MB, minimumOsVersion: "16.0" },
  { bundleId: "com.innersloth.amongus", name: "Among Us!", seller: "Innersloth LLC", price: 0, formattedPrice: "Free", version: "2025.6.10", averageRating: 4.1, ratingCount: 1_202_771, genre: "Games", size: 340 * MB, minimumOsVersion: "13.0" },
  { bundleId: "com.things.iphone", name: "Things 3", seller: "Cultured Code GmbH & Co. KG", price: 9.99, formattedPrice: "$9.99", version: "3.21.7", averageRating: 4.8, ratingCount: 21_998, genre: "Productivity", size: 24 * MB, minimumOsVersion: "17.0" },
]

const store: StoreApp[] = catalog.map((app, i) => ({
  ...app,
  appId: i + 1,
  artworkUrl: iconFor(app.name),
  size: app.size === null ? null : Math.round(app.size),
  description: `${app.name} by ${app.seller}. A sample listing served by the Orchard mock backend so the App Store screen can be exercised without an Apple ID.`,
}))

/* ------------------------------------------------------------------ */
/* Files                                                               */
/* ------------------------------------------------------------------ */

type Tree = Map<string, FileEntry[]>
const trees = new Map<string, Tree>()

function sourceKey(udid: string, source: FileSource) {
  return source.kind === "media" ? `${udid}:media` : `${udid}:${source.kind}:${source.bundleId}`
}

function file(dir: string, name: string, size: number, ageDays: number): FileEntry {
  return {
    name,
    path: `${dir === "/" ? "" : dir}/${name}`,
    kind: "file",
    size,
    modified: now - ageDays * DAY,
  }
}

function dir(parent: string, name: string, ageDays: number): FileEntry {
  return {
    name,
    path: `${parent === "/" ? "" : parent}/${name}`,
    kind: "dir",
    size: 0,
    modified: now - ageDays * DAY,
  }
}

function seedTree(source: FileSource): Tree {
  const t: Tree = new Map()
  if (source.kind === "media") {
    t.set("/", [
      dir("/", "DCIM", 1),
      dir("/", "Downloads", 3),
      dir("/", "Books", 40),
      dir("/", "iTunes_Control", 90),
      dir("/", "Recordings", 12),
      dir("/", "PhotoData", 1),
      file("/", "com.apple.itunes.lock_sync", 0, 2),
    ])
    t.set("/DCIM", [dir("/DCIM", "100APPLE", 1), dir("/DCIM", "101APPLE", 8)])
    t.set(
      "/DCIM/100APPLE",
      Array.from({ length: 14 }, (_, i) =>
        file("/DCIM/100APPLE", `IMG_${String(4210 + i).padStart(4, "0")}.HEIC`, 1.8 * MB + i * 97_000, i * 0.6),
      ),
    )
    t.set("/DCIM/101APPLE", [file("/DCIM/101APPLE", "IMG_5001.MOV", 84 * MB, 8)])
    t.set("/Downloads", [
      file("/Downloads", "boarding-pass.pdf", 182_000, 3),
      file("/Downloads", "Quarterly report final v3 (reviewed by finance and legal).xlsx", 1.1 * MB, 6),
      file("/Downloads", "song.m4a", 7.4 * MB, 20),
    ])
    t.set("/Books", [file("/Books", "Purchases.plist", 4_200, 40)])
    t.set("/iTunes_Control", [dir("/iTunes_Control", "iTunes", 90), dir("/iTunes_Control", "Music", 90)])
    t.set("/iTunes_Control/iTunes", [])
    t.set("/iTunes_Control/Music", [])
    t.set("/Recordings", [file("/Recordings", "Voice memo 12.m4a", 2.3 * MB, 12)])
    t.set("/PhotoData", [file("/PhotoData", "Photos.sqlite", 48 * MB, 1)])
  } else {
    const root = source.kind === "appDocuments" ? "/" : "/"
    const docs: FileEntry[] = [
      file("/", "notes.md", 4_812, 2),
      file("/", "export.csv", 88_102, 9),
      dir("/", "Attachments", 4),
    ]
    if (source.kind === "appDocuments") {
      t.set(root, docs)
      t.set("/Attachments", [file("/Attachments", "scan-0001.jpg", 1.2 * MB, 4)])
    } else {
      t.set("/", [dir("/", "Documents", 2), dir("/", "Library", 2), dir("/", "tmp", 0), dir("/", "SystemData", 30)])
      t.set("/Documents", docs.map((d) => ({ ...d, path: `/Documents${d.path}` })))
      t.set("/Documents/Attachments", [file("/Documents/Attachments", "scan-0001.jpg", 1.2 * MB, 4)])
      t.set("/Library", [dir("/Library", "Caches", 1), dir("/Library", "Preferences", 2)])
      t.set("/Library/Caches", [file("/Library/Caches", "http.db", 3.4 * MB, 1)])
      t.set("/Library/Preferences", [file("/Library/Preferences", "group.plist", 2_048, 2)])
      t.set("/tmp", [])
      t.set("/SystemData", [])
    }
  }
  for (const entries of t.values()) for (const e of entries) e.size = Math.round(e.size)
  return t
}

function tree(udid: string, source: FileSource): Tree {
  const key = sourceKey(udid, source)
  let t = trees.get(key)
  if (!t) {
    t = seedTree(source)
    trees.set(key, t)
  }
  return t
}

function parentOf(path: string) {
  const i = path.lastIndexOf("/")
  return i <= 0 ? "/" : path.slice(0, i)
}

function baseName(path: string) {
  return path.slice(path.lastIndexOf("/") + 1) || path
}

/* ------------------------------------------------------------------ */
/* Backups                                                             */
/* ------------------------------------------------------------------ */

const backups: BackupSummary[] = [
  {
    id: devices[0].udid,
    udid: devices[0].udid,
    deviceName: devices[0].name,
    productType: devices[0].productType,
    iosVersion: "18.6.2",
    date: now - 2 * DAY - 3_600_000 * 5,
    encrypted: true,
    size: Math.round(38.6 * GB),
    path: `~/.local/share/orchard/backups/${devices[0].udid}`,
  },
  {
    id: devices[1].udid,
    udid: devices[1].udid,
    deviceName: devices[1].name,
    productType: devices[1].productType,
    iosVersion: "17.7.1",
    date: now - 19 * DAY,
    encrypted: false,
    size: Math.round(12.2 * GB),
    path: `~/.local/share/orchard/backups/${devices[1].udid}`,
  },
  {
    id: "00008030-0019592E3C42802E",
    udid: "00008030-0019592E3C42802E",
    deviceName: "Old iPhone 11",
    productType: "iPhone12,1",
    iosVersion: "16.7.10",
    date: now - 212 * DAY,
    encrypted: false,
    size: Math.round(21.9 * GB),
    path: "~/.local/share/orchard/backups/00008030-0019592E3C42802E",
  },
]

const unlocked = new Set<string>()

const systemDomains: [string, number, number][] = [
  ["HomeDomain", 1_820, 1.4 * GB],
  ["CameraRollDomain", 4_412, 18.2 * GB],
  ["MediaDomain", 640, 2.1 * GB],
  ["KeychainDomain", 3, 0.4 * MB],
  ["WirelessDomain", 12, 0.2 * MB],
  ["SystemPreferencesDomain", 88, 4 * MB],
  ["HealthDomain", 140, 380 * MB],
  ["RootDomain", 30, 22 * MB],
]

function backupDomains(): BackupDomain[] {
  return [
    ...systemDomains.map(([domain, fileCount, size]) => ({
      domain,
      kind: "system" as const,
      label: domain.replace(/Domain$/, ""),
      bundleId: null,
      fileCount,
      size: Math.round(size),
    })),
    ...userApps.slice(0, 12).map(([bundleId, name, , size]) => ({
      domain: `AppDomain-${bundleId}`,
      kind: "app" as const,
      label: name,
      bundleId,
      fileCount: 20 + (hash(bundleId) % 400),
      size: Math.round(size / 3),
    })),
  ]
}

const backupPaths = [
  "Library/SMS/sms.db",
  "Library/AddressBook/AddressBook.sqlitedb",
  "Library/Notes/notes.sqlite",
  "Library/Safari/Bookmarks.db",
  "Library/Preferences/com.apple.springboard.plist",
  "Library/CallHistoryDB/CallHistory.storedata",
  "Media/DCIM/100APPLE/IMG_4210.HEIC",
  "Media/DCIM/100APPLE/IMG_4211.HEIC",
  "Documents/settings.json",
  "Library/Caches/Snapshots/main@3x.ktx",
  "Library/Application Support/database.sqlite",
  "Library/Cookies/Cookies.binarycookies",
]

/* ------------------------------------------------------------------ */
/* Photos                                                              */
/* ------------------------------------------------------------------ */

const photos: PhotoItem[] = Array.from({ length: 48 }, (_, i) => {
  const video = i % 9 === 4
  const num = String(4260 - i).padStart(4, "0")
  const folder = i < 30 ? "100APPLE" : "101APPLE"
  const name = video ? `IMG_${num}.MOV` : `IMG_${num}.HEIC`
  return {
    path: `/DCIM/${folder}/${name}`,
    name,
    size: video ? Math.round((40 + (i % 5) * 18) * MB) : Math.round((1.4 + (i % 7) * 0.31) * MB),
    modified: now - i * 0.7 * DAY,
    kind: video ? "video" : "image",
  }
})

/* ------------------------------------------------------------------ */
/* Media                                                               */
/* ------------------------------------------------------------------ */

const MEDIA_EXT: Record<Exclude<MediaCategory, "other">, string[]> = {
  audio: ["mp3", "m4a", "aac", "flac", "wav", "aiff", "aif", "alac", "opus", "ogg"],
  video: ["mp4", "m4v", "mov", "mkv", "avi", "webm"],
  book: ["epub", "pdf", "cbz", "cbr", "mobi"],
}

function mediaCategory(name: string): MediaCategory {
  const ext = name.slice(name.lastIndexOf(".") + 1).toLowerCase()
  for (const [cat, list] of Object.entries(MEDIA_EXT)) if (list.includes(ext)) return cat as MediaCategory
  return "other"
}

const mediaTargetList: MediaTarget[] = [
  { bundleId: "org.videolan.vlc-ios", name: "VLC", accepts: ["audio", "video"], recognized: true },
  { bundleId: "com.firecore.infuse", name: "Infuse", accepts: ["video"], recognized: true },
  {
    bundleId: "com.readdle.ReaddleDocsIPad",
    name: "Documents",
    accepts: ["audio", "video", "book", "other"],
    recognized: true,
  },
  { bundleId: "md.obsidian", name: "Obsidian", accepts: ["audio", "video", "book", "other"], recognized: false },
]

const mediaTrees = new Map<string, Map<string, MediaFile[]>>()

function mediaEntry(dir: string, name: string, size: number, ageDays: number, folder = false): MediaFile {
  return {
    name,
    path: `${dir}/${name}`,
    kind: folder ? "dir" : "file",
    size: folder ? 0 : Math.round(size),
    modified: now - ageDays * DAY,
    category: folder ? null : mediaCategory(name),
  }
}

function mediaTree(bundleId: string): Map<string, MediaFile[]> {
  let t = mediaTrees.get(bundleId)
  if (t) return t
  t = new Map()
  const root = "/Documents"
  if (bundleId === "org.videolan.vlc-ios") {
    t.set(root, [
      mediaEntry(root, "Concerts", 0, 6, true),
      mediaEntry(root, "Big Buck Bunny (2008).mkv", 690 * MB, 12),
      mediaEntry(root, "Sintel.mp4", 1.2 * GB, 30),
      mediaEntry(root, "Lecture 04 - Signals and systems.m4v", 412 * MB, 2),
      mediaEntry(root, "Night drive.flac", 41 * MB, 9),
      mediaEntry(root, "Podcast episode 218.mp3", 58 * MB, 1),
      mediaEntry(root, "subtitles.srt", 84_000, 12),
    ])
    t.set(`${root}/Concerts`, [mediaEntry(`${root}/Concerts`, "Live at the Roundhouse.mkv", 2.4 * GB, 6)])
  } else if (bundleId === "com.firecore.infuse") {
    t.set(root, [
      mediaEntry(root, "Tears of Steel (2012).mkv", 1.6 * GB, 3),
      mediaEntry(root, "Cosmos Laundromat.mp4", 980 * MB, 20),
    ])
  } else if (bundleId === "com.readdle.ReaddleDocsIPad") {
    t.set(root, [
      mediaEntry(root, "Books", 0, 15, true),
      mediaEntry(root, "Invoice 2026-09.pdf", 220_000, 24),
      mediaEntry(root, "Voice note.m4a", 3.1 * MB, 5),
      mediaEntry(root, "notes.txt", 2_400, 1),
    ])
    t.set(`${root}/Books`, [
      mediaEntry(`${root}/Books`, "The Pragmatic Programmer.epub", 4.2 * MB, 15),
      mediaEntry(`${root}/Books`, "Comic issue 12.cbz", 88 * MB, 40),
    ])
  } else {
    t.set(root, [])
  }
  mediaTrees.set(bundleId, t)
  return t
}

/* ------------------------------------------------------------------ */
/* Syslog                                                              */
/* ------------------------------------------------------------------ */

let syslogTimer: ReturnType<typeof setInterval> | null = null

const syslogSamples: [string, SyslogLevel, string][] = [
  ["SpringBoard", "default", "[com.apple.springboard] Application launch requested for com.spotify.client"],
  ["locationd", "info", "Location icon should now be in state 'Inactive'"],
  ["kernel", "default", "AppleARMPMUCharger: limiting charge to 80%"],
  ["mDNSResponder", "debug", "[R1442] getaddrinfo start -- flags: 0xC000D000, ifindex: 0, protocols: 0, hostname: <mask.hash: 'k3h1'>"],
  ["Spotify", "error", "Network request failed: The Internet connection appears to be offline. (NSURLErrorDomain -1009)"],
  ["backboardd", "notice", "Brightness changed to 0.42 by user"],
  ["wifid", "info", "WiFi:[1753104412.218] Auto-join scan completed, 14 networks found"],
  ["dasd", "default", "Trigger: <private> is now 1"],
  ["CommCenter", "info", "#I Radio access technology changed to 5G NR NSA"],
  ["runningboardd", "fault", "Assertion did invalidate due to timeout: 412-31-8811 (target:[app<com.burbn.instagram>:1844])"],
  ["lockdownd", "default", "_handle_start_session: Session started for host 7F3A-HOST-ID"],
  ["installd", "info", "0x16f3a3000 -[MIClientConnection installURL:withOptions:completion:]: Install of \"/var/mobile/Media/PublicStaging/app.ipa\" requested by afcd"],
]

function syslogLine(udid: string): SyslogLine {
  const [process, level, message] = syslogSamples[Math.floor(Math.random() * syslogSamples.length)]
  const d = new Date()
  const time = d.toTimeString().slice(0, 8) + "." + String(d.getMilliseconds()).padStart(3, "0")
  return {
    udid,
    timestamp: `${d.toLocaleString("en-US", { month: "short" })} ${String(d.getDate()).padStart(2, " ")} ${time}`,
    process,
    pid: 100 + (hash(process) % 3000),
    level,
    message,
  }
}

/* ------------------------------------------------------------------ */
/* Dialogs                                                             */
/* ------------------------------------------------------------------ */

export const mockDialogs = {
  pickFolder: async () => "/home/you/Downloads",
  pickIpa: async () => "/home/you/Downloads/Lumen-2.4.ipa",
  pickP12: async () => "/home/you/Certificates/Apple Development.p12",
  pickProfile: async () => "/home/you/Certificates/Orchard Dev Wildcard.mobileprovision",
  pickFiles: async () => ["/home/you/Documents/report.pdf", "/home/you/Pictures/photo.jpg"],
  pickSavePath: async (defaultPath: string) => `/home/you/Pictures/${defaultPath}`,
}

let downloadDir: string | null = "/home/you/Downloads"

/* Signing material: one identity and one wildcard profile for team 7QK2M9X4LB. */
const SIGNING_TEAM = "7QK2M9X4LB"
const signingIdentities = new Map<string, SigningIdentityInfo>()
const signingProfiles = new Map<string, ProfileInfo>()
let signingIds = 0
let lastP12: string | null = null
let lastProfile: string | null = null

function resignTarget(ipaPath: string, bundleId: string | null) {
  const stem = baseName(ipaPath).replace(/\.ipa$/i, "").replace(/[-_ ]?v?\d[\d.]*$/, "") || "App"
  return {
    encrypted: /signal|_\d+_/i.test(baseName(ipaPath)),
    bundleId: bundleId ?? `com.example.${stem.toLowerCase().replace(/[^a-z0-9]/g, "")}`,
  }
}

const IPATOOL_NOTE =
  "Your Apple ID password is passed to ipatool on its command line. While sign-in runs, other users on this computer can see it in the process list."

/* ------------------------------------------------------------------ */
/* Handlers                                                            */
/* ------------------------------------------------------------------ */

type Handlers = {
  [C in CommandName]: (args: CommandArgs<C>) => Promise<CommandResult<C>>
}

export const mockHandlers: Handlers = {
  async list_devices() {
    await latency()
    return devices.map(summary)
  },
  async device_info({ udid }) {
    await latency()
    return { ...device(udid) }
  },
  async add_wifi_device({ address }) {
    await sleep(900)
    if (!/^[\w.:-]+$/.test(address)) fail("invalidInput", "Enter an IP address or host name")
    fail("device", `No device answered at ${address}. Make sure it is on the same network and has Wi-Fi sync enabled.`)
  },
  async pair_device({ udid }) {
    await sleep(1200)
    device(udid).paired = true
    emit("orchard://devices", { kind: "updated", udid })
    return null
  },
  async unpair_device({ udid }) {
    await latency()
    device(udid).paired = false
    emit("orchard://devices", { kind: "updated", udid })
    return null
  },
  async set_wifi_sync({ udid, enabled }) {
    await latency()
    device(udid).wifiSyncEnabled = enabled
    return null
  },
  async take_screenshot({ udid }) {
    await sleep(700)
    const d = device(udid)
    const tall = d.productType.startsWith("iPhone")
    const [w, h] = tall ? [393, 852] : [820, 1180]
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#1e3a8a"/><stop offset="1" stop-color="#7c3aed"/></linearGradient></defs><rect width="100%" height="100%" fill="url(#g)"/><text x="50%" y="16%" fill="#fff" font-family="Helvetica,Arial" font-size="${tall ? 72 : 96}" font-weight="200" text-anchor="middle">${new Date().toTimeString().slice(0, 5)}</text><text x="50%" y="21%" fill="#fff" opacity=".8" font-family="Helvetica,Arial" font-size="20" text-anchor="middle">${d.name}</text></svg>`
    return btoa(svg)
  },
  async save_screenshot() {
    await latency()
    return null
  },
  async restart_device({ udid }) {
    await latency()
    device(udid)
    return null
  },
  async shutdown_device({ udid }) {
    await latency()
    device(udid)
    return null
  },

  async list_apps({ udid }) {
    await sleep(500)
    return apps(udid).map((a) => ({ ...a }))
  },
  async app_icon({ udid, bundleId }) {
    await sleep(60 + Math.random() * 300)
    const app = apps(udid).find((a) => a.bundleId === bundleId) ?? mediaTargetList.find((t) => t.bundleId === bundleId)
    return app ? iconFor(app.name) : null
  },
  async uninstall_app({ udid, bundleId }) {
    await sleep(900)
    const list = apps(udid)
    const i = list.findIndex((a) => a.bundleId === bundleId)
    if (i < 0) fail("notFound", bundleId)
    list.splice(i, 1)
    return null
  },
  async inspect_ipa({ path }) {
    await sleep(600)
    if (!path.toLowerCase().endsWith(".ipa")) fail("invalidInput", `${baseName(path)} is not an IPA file`)
    const stem = baseName(path).replace(/\.ipa$/i, "").replace(/[-_ ]?v?\d[\d.]*$/, "") || "App"
    const name = stem.charAt(0).toUpperCase() + stem.slice(1)
    return {
      path,
      name,
      bundleId: `com.example.${name.toLowerCase().replace(/[^a-z0-9]/g, "")}`,
      version: "1204",
      shortVersion: "7.62",
      minimumOsVersion: "15.0",
      // App Store downloads (and the sample Signal IPA) are FairPlay encrypted.
      encrypted: /signal|_\d+_/i.test(baseName(path)),
      size: 87 * MB,
      icon: iconFor(name),
    }
  },
  async install_ipa({ taskId, udid, path }) {
    device(udid)
    await runTask(
      taskId,
      [
        { stage: "upload", total: 87 * MB, ms: 2600 },
        { stage: "install", total: 100, ms: 2400 },
      ],
      `Installed ${baseName(path)}`,
    )
    const name = baseName(path).replace(/\.ipa$/i, "")
    apps(udid).unshift({
      bundleId: `com.example.${name.toLowerCase().replace(/[^a-z0-9]/g, "")}`,
      name,
      version: "1204",
      shortVersion: "7.62",
      size: 87 * MB,
      kind: "user",
      fileSharing: false,
    })
    return null
  },

  async store_account() {
    await latency()
    return account
  },
  async store_sign_in({ email, password, code, country }) {
    await sleep(900)
    if (!email.includes("@") || password.length < 4) fail("authBadCredentials", "Your Apple ID or password was incorrect.")
    if (!code) fail("authCodeRequired", "Enter the verification code sent to your devices.")
    if (!/^\d{6}$/.test(code)) fail("authBadCredentials", "The verification code is incorrect.")
    account = { email, name: email.split("@")[0].replace(/\W+/g, " "), country, backend: "ipatool" }
    return account
  },
  async store_sign_out() {
    await latency()
    account = null
    return null
  },
  async store_search({ term, limit }) {
    await sleep(500)
    if (!account) fail("authSignedOut", "Sign in to search the App Store")
    const q = term.trim().toLowerCase()
    const hits = store.filter(
      (a) => !q || a.name.toLowerCase().includes(q) || a.genre.toLowerCase().includes(q) || a.seller.toLowerCase().includes(q),
    )
    return (hits.length ? hits : store).slice(0, limit)
  },
  async store_versions({ appId }) {
    await sleep(600)
    const app = store.find((a) => a.appId === appId) ?? fail("notFound", `app ${appId}`)
    const [major, minor = "0"] = app.version.split(".")
    return Array.from({ length: 6 }, (_, i) => ({
      id: String(870_000_000 - i * 1_337),
      version: i === 0 ? app.version : i < 4 ? `${major}.${Math.max(0, Number(minor) - i)}` : null,
    }))
  },
  async store_purchase({ appId, confirmation }) {
    await sleep(900)
    const app = store.find((a) => a.appId === appId) ?? fail("notFound", `app ${appId}`)
    if (app.price > 0 && !confirmation) {
      return {
        status: "needsConfirmation" as const,
        message: `Do you want to buy ${app.name} for ${app.formattedPrice}? Your payment method on file will be charged.`,
        okLabel: "Buy",
        buyParams: `productType=C&price=${Math.round(app.price * 1000)}&salableAdamId=${appId}`,
      }
    }
    purchased.add(appId)
    return { status: "purchased" as const }
  },
  async store_download({ taskId, appId, destDir }) {
    if (!account) fail("authSignedOut", "Sign in to download apps")
    const app = store.find((a) => a.appId === appId) ?? fail("notFound", `app ${appId}`)
    if (!purchased.has(appId)) fail("licenseRequired", "This Apple ID does not own the app yet")
    await runTask(taskId, [
      { stage: "license", total: null, ms: 600 },
      { stage: "download", total: app.size ?? 100 * MB, ms: 3200 },
      { stage: "sign", total: null, ms: 600 },
    ])
    return `${destDir}/${app.bundleId}_${appId}_${app.version}.ipa`
  },
  async store_install({ taskId, udid, appId }) {
    if (!account) fail("authSignedOut", "Sign in to install apps")
    device(udid)
    const app = store.find((a) => a.appId === appId) ?? fail("notFound", `app ${appId}`)
    if (!purchased.has(appId)) fail("licenseRequired", "This Apple ID does not own the app yet")
    const size = app.size ?? 100 * MB
    await runTask(
      taskId,
      [
        { stage: "license", total: null, ms: 500 },
        { stage: "download", total: size, ms: 2800 },
        { stage: "upload", total: size, ms: 2000 },
        { stage: "install", total: 100, ms: 1800 },
      ],
      `Installed ${app.name}`,
    )
    return null
  },

  async list_files({ udid, source, path }) {
    await latency()
    const entries = tree(udid, source).get(path) ?? fail("notFound", path)
    return entries.map((e) => ({ ...e }))
  },
  async upload_files({ taskId, udid, source, destDir, localPaths }) {
    const t = tree(udid, source)
    const total = localPaths.length * 2.4 * MB
    await runTask(taskId, [{ stage: "upload", total, ms: 1400 + localPaths.length * 400 }])
    const entries = t.get(destDir) ?? []
    for (const p of localPaths) {
      const name = baseName(p.replace(/\\/g, "/"))
      if (!entries.some((e) => e.name === name)) entries.push(file(destDir, name, Math.round(2.4 * MB), 0))
    }
    t.set(destDir, entries)
    return null
  },
  async download_files({ taskId, udid, source, remotePaths }) {
    const t = tree(udid, source)
    let total = 0
    for (const p of remotePaths) total += t.get(parentOf(p))?.find((e) => e.path === p)?.size || MB
    await runTask(taskId, [{ stage: "download", total, ms: 1600 + remotePaths.length * 200 }])
    return null
  },
  async create_folder({ udid, source, path }) {
    await latency()
    const t = tree(udid, source)
    const parent = parentOf(path)
    const entries = t.get(parent) ?? fail("notFound", parent)
    if (entries.some((e) => e.path === path)) fail("invalidInput", `${baseName(path)} already exists`)
    entries.push(dir(parent, baseName(path), 0))
    t.set(path, [])
    return null
  },
  async rename_path({ udid, source, from, to }) {
    await latency()
    const t = tree(udid, source)
    const entries = t.get(parentOf(from)) ?? fail("notFound", from)
    const entry = entries.find((e) => e.path === from) ?? fail("notFound", from)
    if (entries.some((e) => e.path === to)) fail("invalidInput", `${baseName(to)} already exists`)
    entry.path = to
    entry.name = baseName(to)
    if (entry.kind === "dir") {
      for (const key of [...t.keys()]) {
        if (key === from || key.startsWith(`${from}/`)) {
          const moved = t.get(key)!
          t.delete(key)
          t.set(to + key.slice(from.length), moved.map((e) => ({ ...e, path: to + e.path.slice(from.length) })))
        }
      }
    }
    return null
  },
  async delete_paths({ udid, source, paths }) {
    await sleep(400)
    const t = tree(udid, source)
    for (const p of paths) {
      const entries = t.get(parentOf(p))
      if (entries) t.set(parentOf(p), entries.filter((e) => e.path !== p))
      for (const key of [...t.keys()]) if (key === p || key.startsWith(`${p}/`)) t.delete(key)
    }
    return null
  },

  async list_backups() {
    await sleep(400)
    return backups.map((b) => ({ ...b }))
  },
  async create_backup({ taskId, udid, mode }) {
    const d = device(udid)
    const total = Math.round(mode === "full" ? d.storageUsed * 0.2 : d.storageUsed * 0.01)
    await runTask(
      taskId,
      [
        { stage: "prepare", total: null, ms: 800 },
        { stage: "backup", total, ms: mode === "full" ? 5200 : 2600 },
      ],
      `Backed up ${d.name}`,
    )
    const existing = backups.find((b) => b.udid === udid)
    if (existing) existing.date = Date.now()
    else
      backups.unshift({
        id: udid,
        udid,
        deviceName: d.name,
        productType: d.productType,
        iosVersion: d.iosVersion,
        date: Date.now(),
        encrypted: false,
        size: Math.round(d.storageUsed * 0.2),
        path: `~/.local/share/orchard/backups/${udid}`,
      })
    return null
  },
  async open_backup({ backupId, password }) {
    await sleep(700)
    const b = backups.find((x) => x.id === backupId) ?? fail("notFound", backupId)
    if (b.encrypted && !unlocked.has(backupId)) {
      if (!password || password.length < 4) fail("backupPassword", "backup password is missing or wrong")
      unlocked.add(backupId)
    }
    return backupDomains()
  },
  async list_backup_files({ domain, query }) {
    await sleep(300)
    const q = query?.trim().toLowerCase() ?? ""
    const files: BackupFile[] = backupPaths.map((relativePath, i) => ({
      fileId: (hash(domain + relativePath) >>> 0).toString(16).padStart(8, "0") + "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6",
      domain,
      relativePath,
      size: 2_000 + ((hash(relativePath) % 9_000) * 731),
      modified: now - i * 3 * DAY,
    }))
    return q ? files.filter((f) => f.relativePath.toLowerCase().includes(q)) : files
  },
  async extract_backup_files({ taskId, fileIds }) {
    await runTask(taskId, [{ stage: "extract", total: fileIds.length, ms: 900 + fileIds.length * 150 }])
    return null
  },
  async restore_backup({ taskId, udid }) {
    device(udid)
    await runTask(
      taskId,
      [
        { stage: "prepare", total: null, ms: 900 },
        { stage: "restore", total: 4 * GB, ms: 6000 },
      ],
      "Restore finished, the device will restart",
    )
    return null
  },

  async list_photos({ udid }) {
    await sleep(600)
    device(udid)
    return photos.map((p) => ({ ...p }))
  },
  async export_photos({ taskId, paths }) {
    const total = photos.filter((p) => paths.includes(p.path)).reduce((sum, p) => sum + p.size, 0)
    await runTask(
      taskId,
      [{ stage: "export", total, ms: 800 + paths.length * 120 }],
      `Exported ${paths.length} items`,
    )
    return null
  },

  async media_targets({ udid }) {
    await sleep(400)
    device(udid)
    return mediaTargetList.map((t) => ({ ...t, accepts: [...t.accepts] }))
  },
  async list_media({ udid, bundleId, dir }) {
    await latency()
    device(udid)
    const entries = mediaTree(bundleId).get(dir) ?? fail("notFound", dir)
    return entries.map((e) => ({ ...e }))
  },
  async send_media({ taskId, udid, bundleId, localPaths, subdir }) {
    device(udid)
    const dir = subdir ? `/Documents/${subdir}` : "/Documents"
    const t = mediaTree(bundleId)
    const entries = t.get(dir) ?? []
    const size = 64 * MB
    await runTask(taskId, [{ stage: "send", total: size * localPaths.length, ms: 1200 + localPaths.length * 500 }])
    const summary = { uploaded: 0, skipped: 0, bytes: 0, renamed: [] as string[], skippedSymlinks: 0, skippedInvalid: 0 }
    for (const p of localPaths) {
      const name = baseName(p.replace(/\\/g, "/"))
      const existing = entries.find((e) => e.name === name)
      if (existing && existing.size === size) {
        summary.skipped++
        continue
      }
      let finalName = name
      if (existing) {
        const dot = name.lastIndexOf(".")
        const [stem, ext] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""]
        for (let n = 1; entries.some((e) => e.name === finalName); n++) finalName = `${stem} (${n})${ext}`
        summary.renamed.push(`${dir}/${finalName}`)
      }
      entries.push(mediaEntry(dir, finalName, size, 0))
      summary.uploaded++
      summary.bytes += size
    }
    t.set(dir, entries)
    return summary
  },

  async load_signing_identity({ path, password }) {
    await sleep(500)
    if (!password) fail("invalidInput", "The password is wrong or the file is not a .p12 export.")
    const info: SigningIdentityInfo = {
      id: `identity-${signingIds++}`,
      path,
      commonName: "Apple Development: Marco Beretta (4F8T2Q9KXN)",
      teamId: SIGNING_TEAM,
      expiresAt: now + 214 * DAY,
    }
    signingIdentities.set(info.id, info)
    lastP12 = path
    return info
  },
  async load_provisioning_profile({ path }) {
    await sleep(400)
    const info: ProfileInfo = {
      id: `profile-${signingIds++}`,
      path,
      name: "Orchard Dev Wildcard",
      teamId: SIGNING_TEAM,
      teamName: "Marco Beretta",
      applicationIdentifier: `${SIGNING_TEAM}.com.marco.*`,
      bundleIdPattern: "com.marco.*",
      wildcard: true,
      devices: [devices[0].udid, "00008110-000A1B2C3D4E5F60"],
      provisionsAllDevices: false,
      expiresAt: now + 96 * DAY,
    }
    signingProfiles.set(info.id, info)
    lastProfile = path
    return info
  },
  async check_resign({ ipaPath, identityId, profileId, udid, bundleId }) {
    await sleep(300)
    const identity = signingIdentities.get(identityId) ?? fail("invalidInput", "load the signing certificate again")
    const profile = signingProfiles.get(profileId) ?? fail("invalidInput", "load the provisioning profile again")
    const target = resignTarget(ipaPath, bundleId)
    const issues: ResignIssue[] = []
    if (target.encrypted) issues.push({ kind: "encrypted" })
    if (identity.teamId !== profile.teamId)
      issues.push({ kind: "teamMismatch", certificate: identity.teamId, profile: profile.teamId })
    const prefix = profile.bundleIdPattern.replace(/\*$/, "")
    if (!target.bundleId.startsWith(prefix) || target.bundleId === prefix)
      issues.push({ kind: "bundleIdNotMatched", bundleId: target.bundleId, pattern: profile.bundleIdPattern })
    if (udid && !profile.provisionsAllDevices && !profile.devices.some((d) => d.toLowerCase() === udid.toLowerCase()))
      issues.push({ kind: "deviceNotProvisioned", udid })
    return issues
  },
  async resign_ipa({ taskId, ipaPath, identityId, profileId, bundleId, destPath }) {
    const profile = signingProfiles.get(profileId) ?? fail("invalidInput", "load the provisioning profile again")
    if (!signingIdentities.has(identityId)) fail("invalidInput", "load the signing certificate again")
    const target = resignTarget(ipaPath, bundleId)
    if (target.encrypted) fail("invalidInput", "the app is FairPlay encrypted (an App Store download) and cannot be re-signed")
    await runTask(taskId, [
      { stage: "extract", total: null, ms: 700 },
      { stage: "sign", total: 1, ms: 1400 },
      { stage: "package", total: 412, ms: 1100 },
    ])
    return {
      bundleId: target.bundleId,
      teamId: profile.teamId,
      bundles: [target.bundleId, `${target.bundleId}.ShareExtension`],
      removed: ["iTunesMetadata.plist"],
      outputSize: 86 * MB,
      path: destPath,
    }
  },
  async resign_and_install({ taskId, udid, ipaPath, identityId, profileId, bundleId }) {
    device(udid)
    if (!signingProfiles.has(profileId)) fail("invalidInput", "load the provisioning profile again")
    if (!signingIdentities.has(identityId)) fail("invalidInput", "load the signing certificate again")
    const target = resignTarget(ipaPath, bundleId)
    if (target.encrypted) fail("invalidInput", "the app is FairPlay encrypted (an App Store download) and cannot be re-signed")
    await runTask(taskId, [
      { stage: "extract", total: null, ms: 600 },
      { stage: "sign", total: 1, ms: 1200 },
      { stage: "package", total: 412, ms: 900 },
      { stage: "upload", total: 86 * MB, ms: 1800 },
      { stage: "install", total: 100, ms: 1600 },
    ])
    return null
  },

  async start_syslog({ udid }) {
    device(udid)
    if (syslogTimer) clearInterval(syslogTimer)
    syslogTimer = setInterval(() => {
      const n = 1 + Math.floor(Math.random() * 3)
      for (let i = 0; i < n; i++) emit("orchard://syslog", syslogLine(udid))
    }, 280)
    return null
  },
  async stop_syslog() {
    if (syslogTimer) clearInterval(syslogTimer)
    syslogTimer = null
    return null
  },

  async cancel_task({ taskId }) {
    cancelled.add(taskId)
    return null
  },
  async get_settings() {
    return {
      downloadDir,
      dataDir: "~/.local/share/orchard",
      storeBackend: "ipatool",
      storeSignInNote: IPATOOL_NOTE,
      signingP12Path: lastP12,
      signingProfilePath: lastProfile,
    }
  },
  async open_data_dir() {},
  async new_export_dir() {
    return "/home/you/Downloads/Orchard"
  },
  async set_download_dir({ path }) {
    await latency()
    downloadDir = path
    return {
      downloadDir,
      dataDir: "~/.local/share/orchard",
      storeBackend: "ipatool",
      storeSignInNote: IPATOOL_NOTE,
      signingP12Path: lastP12,
      signingProfilePath: lastProfile,
    }
  },
  async app_info() {
    return { version: "0.1.0", platform: "browser (mock)", arch: "wasm", material: null }
  },
}
