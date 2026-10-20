import { invoke, isTauri } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"
import { open, save } from "@tauri-apps/plugin-dialog"
import { revealItemInDir } from "@tauri-apps/plugin-opener"
import { toast } from "sonner"

import { mockHandlers, mockListen, mockDialogs } from "@/lib/mock"
import {
  configuredServer,
  detectSameOrigin,
  downloadFromServer,
  RemoteError,
  remoteCall,
  remoteListen,
  uploadFile,
} from "@/lib/remote"

/* ------------------------------------------------------------------ */
/* Errors                                                              */
/* ------------------------------------------------------------------ */

/** Mirrors `orchard_core::Error::kind()`. */
export type ErrorKind =
  | "notImplemented"
  | "io"
  | "device"
  | "protocol"
  | "invalidInput"
  | "notFound"
  | "network"
  | "authCodeRequired"
  | "authBadCredentials"
  | "authSessionExpired"
  | "authSignedOut"
  | "licenseRequired"
  | "store"
  | "backupPassword"
  | "cancelled"
  | "unknown"

export class OrchardError extends Error {
  readonly kind: ErrorKind

  constructor(kind: ErrorKind, message: string) {
    super(message)
    this.name = "OrchardError"
    this.kind = kind
  }
}

export function isOrchardError(e: unknown, kind?: ErrorKind): e is OrchardError {
  return e instanceof OrchardError && (kind === undefined || e.kind === kind)
}

export function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message
  if (typeof e === "string") return e
  return "Something went wrong"
}

/* ------------------------------------------------------------------ */
/* Types                                                               */
/* ------------------------------------------------------------------ */

export type Connection = "usb" | "wifi"

export interface Device {
  udid: string
  name: string
  /** Marketing name, e.g. "iPhone 15 Pro". */
  model: string
  /** Hardware identifier, e.g. "iPhone16,1". */
  productType: string
  iosVersion: string
  connection: Connection
  paired: boolean
}

export interface DeviceInfo extends Device {
  buildVersion: string
  serialNumber: string
  /** 0 to 100, null when unknown. */
  batteryLevel: number | null
  batteryCharging: boolean
  storageTotal: number
  storageUsed: number
  wifiSyncEnabled: boolean
}

/** What `import_pairing_file` stored: a classic USB-made record, or an iOS 27 Wi-Fi pairing. */
export interface PairingImport {
  kind: "lockdown" | "wifi"
  /** Device UDID (lockdown) or host identifier (Wi-Fi). */
  id: string
}

export interface DeviceEvent {
  kind: "attached" | "detached" | "updated"
  udid: string
}

export type AppKind = "user" | "system"

export interface InstalledApp {
  bundleId: string
  name: string
  version: string
  shortVersion: string
  /** Static plus dynamic size in bytes, null when unknown. */
  size: number | null
  kind: AppKind
  /** True when the app exposes Documents through file sharing. */
  fileSharing: boolean
}

export interface IpaInfo {
  path: string
  name: string
  bundleId: string
  version: string
  shortVersion: string
  minimumOsVersion: string | null
  encrypted: boolean
  size: number
  /** PNG data URL, null when the IPA has no usable icon. */
  icon: string | null
}

/** ipatool handles sign-in reliably; the native client may be rejected by Apple. */
export type StoreBackend = "ipatool" | "native"

/** Apple's price confirmation for a paid purchase, passed back unchanged to buy. */
export interface PurchaseConfirmation {
  message: string
  okLabel: string
  buyParams: string
}

export type PurchaseResult =
  | { status: "purchased" }
  | ({ status: "needsConfirmation" } & PurchaseConfirmation)

export interface StoreAccount {
  email: string
  name: string
  /** Two letter storefront country code, e.g. "US". */
  country: string
  backend: StoreBackend
}

export interface StoreApp {
  appId: number
  bundleId: string
  name: string
  seller: string
  /** 0 for free apps. */
  price: number
  formattedPrice: string
  version: string
  artworkUrl: string
  averageRating: number | null
  ratingCount: number
  genre: string
  size: number | null
  minimumOsVersion: string | null
  description: string
}

export interface StoreVersion {
  /** External version identifier used by the store. */
  id: string
  /** Display version when known. */
  version: string | null
}

export type FileSource =
  | { kind: "media" }
  | { kind: "appDocuments"; bundleId: string }
  | { kind: "appContainer"; bundleId: string }

export type FileKind = "file" | "dir" | "symlink"

export interface FileEntry {
  name: string
  /** Absolute path inside the source, "/" separated. */
  path: string
  kind: FileKind
  size: number
  /** Unix milliseconds, null when unknown. */
  modified: number | null
}

export interface BackupSummary {
  /** Stable identifier, the backup directory name. */
  id: string
  udid: string
  deviceName: string
  productType: string
  iosVersion: string
  /** Unix milliseconds of the last completed backup. */
  date: number
  encrypted: boolean
  size: number
  path: string
}

export type BackupMode = "full" | "incremental"

export interface BackupDomain {
  /** Raw domain, e.g. "HomeDomain" or "AppDomain-com.example.app". */
  domain: string
  kind: "system" | "app"
  /** Human label: app name for app domains, domain otherwise. */
  label: string
  bundleId: string | null
  fileCount: number
  size: number
}

export interface BackupFile {
  fileId: string
  domain: string
  relativePath: string
  size: number
  modified: number | null
}

export interface PhotoItem {
  /** Path on the device, e.g. "/DCIM/100APPLE/IMG_0001.HEIC". */
  path: string
  name: string
  size: number
  modified: number | null
  kind: "image" | "video"
}

export type MediaCategory = "audio" | "video" | "book" | "other"

/** An app that receives files through its shared Documents folder. */
export interface MediaTarget {
  bundleId: string
  name: string
  accepts: MediaCategory[]
  /** True for apps Orchard knows; false means "any file" (fallback). */
  recognized: boolean
}

/** A file in an app's shared folder; `category` is null for folders. */
export interface MediaFile extends FileEntry {
  category: MediaCategory | null
}

export interface SendSummary {
  uploaded: number
  /** Already on the device with the same size. */
  skipped: number
  bytes: number
  /** Device paths that got a numbered name because the name was taken. */
  renamed: string[]
  skippedSymlinks: number
  skippedInvalid: number
}

export type SyslogLevel = "default" | "info" | "debug" | "error" | "fault" | "notice"

export interface SyslogLine {
  udid: string
  /** Device timestamp as printed by the device. */
  timestamp: string
  process: string
  pid: number | null
  level: SyslogLevel
  message: string
}

/** Payload of `orchard://syslog-status`: the device ended the log stream. */
export interface SyslogEnded {
  udid: string
  /** Error text, null when the device closed the stream. */
  message: string | null
}

export type TaskStatus = "running" | "done" | "error"

export interface TaskEvent {
  taskId: string
  stage: string
  done: number
  total: number | null
  status: TaskStatus
  message?: string | null
}

export interface Settings {
  /** Default folder for downloads and exports, null to ask each time. */
  downloadDir: string | null
  /** Where Orchard keeps its own data (pairing records, sessions, cache). */
  dataDir: string
  storeBackend: StoreBackend
  /** Privacy note to show on the sign-in form for this backend, if any. */
  storeSignInNote: string | null
  /** Last `.p12` used for re-signing (the password is never stored). */
  signingP12Path: string | null
  /** Last `.mobileprovision` used for re-signing. */
  signingProfilePath: string | null
}

/** A loaded signing certificate; the backend keeps it under `id`. */
export interface SigningIdentityInfo {
  id: string
  path: string
  commonName: string | null
  teamId: string | null
  /** Unix milliseconds. */
  expiresAt: number
}

/** A loaded provisioning profile; the backend keeps it under `id`. */
export interface ProfileInfo {
  id: string
  path: string
  name: string
  teamId: string
  teamName: string | null
  applicationIdentifier: string
  /** e.g. "com.example.*" */
  bundleIdPattern: string
  wildcard: boolean
  devices: string[]
  provisionsAllDevices: boolean
  /** Unix milliseconds. */
  expiresAt: number
}

export type ResignIssue =
  | { kind: "encrypted" }
  | { kind: "certificateExpired"; expiredAt: number }
  | { kind: "profileExpired"; expiredAt: number }
  | { kind: "teamMismatch"; certificate: string | null; profile: string }
  | { kind: "certificateNotInProfile" }
  | { kind: "bundleIdNotMatched"; bundleId: string; pattern: string }
  | { kind: "deviceNotProvisioned"; udid: string }

export interface ResignReport {
  bundleId: string
  teamId: string
  /** Signed bundle ids, main app first. */
  bundles: string[]
  removed: string[]
  outputSize: number
  path: string
}

export interface AppInfo {
  version: string
  platform: string
  arch: string
  /** Native material behind the window, null when it is opaque. */
  material: "liquidGlass" | "vibrancy" | "mica" | null
}

/* ------------------------------------------------------------------ */
/* Command table                                                       */
/* ------------------------------------------------------------------ */

type Empty = Record<string, never>

/** Every backend command: name, arguments (camelCase), return type. */
export interface Commands {
  list_devices: { args: Empty; ret: Device[] }
  device_info: { args: { udid: string }; ret: DeviceInfo }
  add_wifi_device: { args: { address: string }; ret: Device }
  pair_device: { args: { udid: string }; ret: null }
  unpair_device: { args: { udid: string }; ret: null }
  set_wifi_sync: { args: { udid: string; enabled: boolean }; ret: null }
  import_pairing_file: { args: { path: string }; ret: PairingImport }
  take_screenshot: { args: { udid: string }; ret: string }
  save_screenshot: { args: { path: string; png: string }; ret: null }
  restart_device: { args: { udid: string }; ret: null }
  shutdown_device: { args: { udid: string }; ret: null }

  list_apps: { args: { udid: string }; ret: InstalledApp[] }
  app_icon: { args: { udid: string; bundleId: string }; ret: string | null }
  uninstall_app: { args: { udid: string; bundleId: string }; ret: null }
  inspect_ipa: { args: { path: string }; ret: IpaInfo }
  install_ipa: { args: { taskId: string; udid: string; path: string }; ret: null }

  store_account: { args: Empty; ret: StoreAccount | null }
  store_sign_in: {
    args: { email: string; password: string; code: string | null; country: string; native: boolean }
    ret: StoreAccount
  }
  store_sign_out: { args: Empty; ret: null }
  store_search: { args: { term: string; country: string; limit: number }; ret: StoreApp[] }
  store_versions: { args: { appId: number }; ret: StoreVersion[] }
  store_purchase: {
    args: { appId: number; price: number; confirmation: PurchaseConfirmation | null }
    ret: PurchaseResult
  }
  store_download: {
    args: { taskId: string; appId: number; versionId: string | null; destDir: string }
    ret: string
  }
  store_install: {
    args: { taskId: string; udid: string; appId: number; versionId: string | null }
    ret: null
  }

  list_files: { args: { udid: string; source: FileSource; path: string }; ret: FileEntry[] }
  upload_files: {
    args: { taskId: string; udid: string; source: FileSource; destDir: string; localPaths: string[] }
    ret: null
  }
  download_files: {
    args: { taskId: string; udid: string; source: FileSource; remotePaths: string[]; localDir: string }
    ret: null
  }
  create_folder: { args: { udid: string; source: FileSource; path: string }; ret: null }
  rename_path: { args: { udid: string; source: FileSource; from: string; to: string }; ret: null }
  delete_paths: { args: { udid: string; source: FileSource; paths: string[] }; ret: null }

  list_backups: { args: Empty; ret: BackupSummary[] }
  create_backup: { args: { taskId: string; udid: string; mode: BackupMode }; ret: null }
  open_backup: { args: { backupId: string; password: string | null }; ret: BackupDomain[] }
  list_backup_files: {
    args: { backupId: string; domain: string; query: string | null }
    ret: BackupFile[]
  }
  extract_backup_files: {
    args: { taskId: string; backupId: string; fileIds: string[]; destDir: string }
    ret: null
  }
  restore_backup: {
    args: { taskId: string; udid: string; backupId: string; password: string | null }
    ret: null
  }

  list_photos: { args: { udid: string }; ret: PhotoItem[] }
  export_photos: {
    args: { taskId: string; udid: string; paths: string[]; destDir: string }
    ret: null
  }

  media_targets: { args: { udid: string }; ret: MediaTarget[] }
  list_media: { args: { udid: string; bundleId: string; dir: string }; ret: MediaFile[] }
  send_media: {
    args: {
      taskId: string
      udid: string
      bundleId: string
      localPaths: string[]
      subdir: string | null
    }
    ret: SendSummary
  }

  load_signing_identity: { args: { path: string; password: string }; ret: SigningIdentityInfo }
  load_provisioning_profile: { args: { path: string }; ret: ProfileInfo }
  check_resign: {
    args: {
      ipaPath: string
      identityId: string
      profileId: string
      udid: string | null
      bundleId: string | null
    }
    ret: ResignIssue[]
  }
  resign_ipa: {
    args: {
      taskId: string
      ipaPath: string
      identityId: string
      profileId: string
      bundleId: string | null
      displayName: string | null
      destPath: string
    }
    ret: ResignReport
  }
  resign_and_install: {
    args: {
      taskId: string
      udid: string
      ipaPath: string
      identityId: string
      profileId: string
      bundleId: string | null
      displayName: string | null
    }
    ret: null
  }

  start_syslog: { args: { udid: string }; ret: null }
  stop_syslog: { args: Empty; ret: null }

  cancel_task: { args: { taskId: string }; ret: null }
  get_settings: { args: Empty; ret: Settings }
  set_download_dir: { args: { path: string | null }; ret: Settings }
  open_data_dir: { args: Empty; ret: void }
  /** Server only: a fresh folder for results a remote client will download. */
  new_export_dir: { args: Empty; ret: string }
  app_info: { args: Empty; ret: AppInfo }
}

export type CommandName = keyof Commands
export type CommandArgs<C extends CommandName> = Commands[C]["args"]
export type CommandResult<C extends CommandName> = Commands[C]["ret"]

/* ------------------------------------------------------------------ */
/* Transport                                                           */
/* ------------------------------------------------------------------ */

/** True when running inside the Tauri webview, false in a plain browser. */
export const inTauri = isTauri()

/**
 * Who answers the commands: the desktop app's own Rust backend, an Orchard
 * server (the web app it serves, or the iPad app), or the mock in a plain
 * browser preview.
 */
export type Backend = "desktop" | "remote" | "mock"
let backend: Backend = inTauri ? "desktop" : "mock"

export function currentBackend(): Backend {
  return backend
}

/** Picks the backend; call once before rendering. */
export async function initBackend(): Promise<void> {
  if (inTauri) {
    // The mobile app has no device stack of its own; it is a server client.
    const info = await invoke<AppInfo>("dispatch", { command: "app_info", args: {} }).catch(() => null)
    backend = info?.platform === "ios" || info?.platform === "android" ? "remote" : "desktop"
    return
  }
  if (configuredServer() || (await detectSameOrigin())) backend = "remote"
}

function toOrchardError(e: unknown): OrchardError {
  if (e instanceof OrchardError) return e
  if (e instanceof RemoteError) return new OrchardError(e.kind as ErrorKind, e.message)
  if (e && typeof e === "object" && "kind" in e && "message" in e) {
    const { kind, message } = e as { kind: string; message: string }
    return new OrchardError(kind as ErrorKind, message)
  }
  return new OrchardError("unknown", errorMessage(e))
}

async function call<C extends CommandName>(
  cmd: C,
  args: CommandArgs<C>,
): Promise<CommandResult<C>> {
  try {
    if (backend === "desktop") return await invoke<CommandResult<C>>("dispatch", { command: cmd, args })
    if (backend === "remote") return await remoteCall<CommandResult<C>>(cmd, args)
    return await mockHandlers[cmd](args)
  } catch (e) {
    throw toOrchardError(e)
  }
}

export const EVENTS = {
  task: "orchard://task",
  devices: "orchard://devices",
  syslog: "orchard://syslog",
  syslogStatus: "orchard://syslog-status",
} as const

interface EventPayloads {
  [EVENTS.task]: TaskEvent
  [EVENTS.devices]: DeviceEvent
  [EVENTS.syslog]: SyslogLine
  [EVENTS.syslogStatus]: SyslogEnded
}

/** Subscribes to a backend event. Resolves to an unsubscribe function. */
export async function subscribe<E extends keyof EventPayloads>(
  event: E,
  handler: (payload: EventPayloads[E]) => void,
): Promise<UnlistenFn> {
  if (backend === "desktop") return listen<EventPayloads[E]>(event, (e) => handler(e.payload))
  if (backend === "remote") return remoteListen(event, handler as (payload: unknown) => void)
  return mockListen(event, handler as (payload: unknown) => void)
}

/** Generates a client side task id for long running commands. */
export function newTaskId(): string {
  return crypto.randomUUID()
}

/* ------------------------------------------------------------------ */
/* Devices                                                             */
/* ------------------------------------------------------------------ */

export const listDevices = () => call("list_devices", {})
export const deviceInfo = (udid: string) => call("device_info", { udid })
export const addWifiDevice = (address: string) => call("add_wifi_device", { address })
export const pairDevice = (udid: string) => call("pair_device", { udid })
export const unpairDevice = (udid: string) => call("unpair_device", { udid })
export const setWifiSync = (udid: string, enabled: boolean) =>
  call("set_wifi_sync", { udid, enabled })
export const importPairingFile = (path: string) => call("import_pairing_file", { path })
/** Returns a PNG as a base64 string (no data URL prefix). */
export const takeScreenshot = (udid: string) => call("take_screenshot", { udid })
export const saveScreenshot = (path: string, png: string) => call("save_screenshot", { path, png })
export const restartDevice = (udid: string) => call("restart_device", { udid })
export const shutdownDevice = (udid: string) => call("shutdown_device", { udid })

/* ------------------------------------------------------------------ */
/* Apps                                                                */
/* ------------------------------------------------------------------ */

export const listApps = (udid: string) => call("list_apps", { udid })
/** Returns a PNG data URL, or null when the app has no icon. */
export const appIcon = (udid: string, bundleId: string) => call("app_icon", { udid, bundleId })
export const uninstallApp = (udid: string, bundleId: string) =>
  call("uninstall_app", { udid, bundleId })
export const inspectIpa = (path: string) => call("inspect_ipa", { path })
export const installIpa = (taskId: string, udid: string, path: string) =>
  call("install_ipa", { taskId, udid, path })

/* ------------------------------------------------------------------ */
/* App Store                                                           */
/* ------------------------------------------------------------------ */

export const storeAccount = () => call("store_account", {})
export const storeSignIn = (
  email: string,
  password: string,
  code: string | null,
  country: string,
  native = false,
) => call("store_sign_in", { email, password, code, country, native })
export const storeSignOut = () => call("store_sign_out", {})
export const storeSearch = (term: string, country: string, limit = 30) =>
  call("store_search", { term, country, limit })
export const storeVersions = (appId: number) => call("store_versions", { appId })
/** Gets a free app or buys a paid one; paid apps first return Apple's confirmation. */
export const storePurchase = (
  appId: number,
  price: number,
  confirmation: PurchaseConfirmation | null = null,
) => call("store_purchase", { appId, price, confirmation })
/** Resolves to the path of the downloaded IPA. */
export const storeDownload = (
  taskId: string,
  appId: number,
  versionId: string | null,
  destDir: string,
) => call("store_download", { taskId, appId, versionId, destDir })
export const storeInstall = (taskId: string, udid: string, appId: number, versionId: string | null) =>
  call("store_install", { taskId, udid, appId, versionId })

/* ------------------------------------------------------------------ */
/* Files                                                               */
/* ------------------------------------------------------------------ */

export const listFiles = (udid: string, source: FileSource, path: string) =>
  call("list_files", { udid, source, path })
export const uploadFiles = (
  taskId: string,
  udid: string,
  source: FileSource,
  destDir: string,
  localPaths: string[],
) => call("upload_files", { taskId, udid, source, destDir, localPaths })
export const downloadFiles = (
  taskId: string,
  udid: string,
  source: FileSource,
  remotePaths: string[],
  localDir: string,
) => call("download_files", { taskId, udid, source, remotePaths, localDir })
export const createFolder = (udid: string, source: FileSource, path: string) =>
  call("create_folder", { udid, source, path })
export const renamePath = (udid: string, source: FileSource, from: string, to: string) =>
  call("rename_path", { udid, source, from, to })
export const deletePaths = (udid: string, source: FileSource, paths: string[]) =>
  call("delete_paths", { udid, source, paths })

/* ------------------------------------------------------------------ */
/* Backups                                                             */
/* ------------------------------------------------------------------ */

export const listBackups = () => call("list_backups", {})
export const createBackup = (taskId: string, udid: string, mode: BackupMode) =>
  call("create_backup", { taskId, udid, mode })
/** Unlocks (when encrypted) and indexes a backup, returning its domains. */
export const openBackup = (backupId: string, password: string | null) =>
  call("open_backup", { backupId, password })
export const listBackupFiles = (backupId: string, domain: string, query: string | null) =>
  call("list_backup_files", { backupId, domain, query })
export const extractBackupFiles = (
  taskId: string,
  backupId: string,
  fileIds: string[],
  destDir: string,
) => call("extract_backup_files", { taskId, backupId, fileIds, destDir })
export const restoreBackup = (
  taskId: string,
  udid: string,
  backupId: string,
  password: string | null,
) => call("restore_backup", { taskId, udid, backupId, password })

/* ------------------------------------------------------------------ */
/* Photos                                                              */
/* ------------------------------------------------------------------ */

export const listPhotos = (udid: string) => call("list_photos", { udid })
export const exportPhotos = (taskId: string, udid: string, paths: string[], destDir: string) =>
  call("export_photos", { taskId, udid, paths, destDir })

/* ------------------------------------------------------------------ */
/* Re-signing                                                          */
/* ------------------------------------------------------------------ */

/** The password is sent once to read the `.p12`; it is not stored. */
export const loadSigningIdentity = (path: string, password: string) =>
  call("load_signing_identity", { path, password })
export const loadProvisioningProfile = (path: string) => call("load_provisioning_profile", { path })
export const checkResign = (
  ipaPath: string,
  identityId: string,
  profileId: string,
  udid: string | null,
  bundleId: string | null,
) => call("check_resign", { ipaPath, identityId, profileId, udid, bundleId })

export interface ResignChanges {
  bundleId: string | null
  displayName: string | null
}

/** Re-signs into `destPath`. Task stages: extract, sign, package. */
export const resignIpa = (
  taskId: string,
  ipaPath: string,
  identityId: string,
  profileId: string,
  changes: ResignChanges,
  destPath: string,
) => call("resign_ipa", { taskId, ipaPath, identityId, profileId, ...changes, destPath })

/** Re-signs to a temp file and installs it. Stages: extract, sign, package, upload, install. */
export const resignAndInstall = (
  taskId: string,
  udid: string,
  ipaPath: string,
  identityId: string,
  profileId: string,
  changes: ResignChanges,
) => call("resign_and_install", { taskId, udid, ipaPath, identityId, profileId, ...changes })

/* ------------------------------------------------------------------ */
/* Media                                                               */
/* ------------------------------------------------------------------ */

/** Root of an app's shared folder as seen by `list_media`. */
export const MEDIA_ROOT = "/Documents"

export const mediaTargets = (udid: string) => call("media_targets", { udid })
export const listMedia = (udid: string, bundleId: string, dir = MEDIA_ROOT) =>
  call("list_media", { udid, bundleId, dir })
/** Sends files and folders into the app's Documents (or `subdir` inside it). */
export const sendMedia = (
  taskId: string,
  udid: string,
  bundleId: string,
  localPaths: string[],
  subdir: string | null,
) => call("send_media", { taskId, udid, bundleId, localPaths, subdir })

/* ------------------------------------------------------------------ */
/* Console                                                             */
/* ------------------------------------------------------------------ */

/** Starts streaming `orchard://syslog` for a device, replacing any previous stream. */
export const startSyslog = (udid: string) => call("start_syslog", { udid })
export const stopSyslog = () => call("stop_syslog", {})

/* ------------------------------------------------------------------ */
/* App                                                                 */
/* ------------------------------------------------------------------ */

export const cancelTask = (taskId: string) => call("cancel_task", { taskId })
export const getSettings = () => call("get_settings", {})
export const setDownloadDir = (path: string | null) => call("set_download_dir", { path })
export const appInfo = () => call("app_info", {})

/* ------------------------------------------------------------------ */
/* Native dialogs and shell                                            */
/* ------------------------------------------------------------------ */

function single(result: string | string[] | null): string | null {
  return Array.isArray(result) ? (result[0] ?? null) : result
}

/** Uploads files to the server, with a progress toast; [] when it fails. */
export async function uploadToServer(files: File[]): Promise<string[]> {
  if (files.length === 0) return []
  const id = toast.loading(`Uploading ${files.length === 1 ? files[0].name : `${files.length} files`}`)
  try {
    const paths: string[] = []
    for (const file of files) {
      paths.push(
        await uploadFile(file, (sent, total) =>
          toast.loading(`Uploading ${file.name}`, {
            id,
            description: total ? `${Math.round((sent / total) * 100)}%` : undefined,
          }),
        ),
      )
    }
    toast.dismiss(id)
    return paths
  } catch (e) {
    toast.error("Upload failed", { id, description: errorMessage(e) })
    return []
  }
}

/** A remote client picks files on this device; they are uploaded to the server. */
function pickAndUpload(accept: string, multiple: boolean): Promise<string[]> {
  return new Promise((resolve) => {
    const input = document.createElement("input")
    input.type = "file"
    input.accept = accept
    input.multiple = multiple
    input.addEventListener("cancel", () => resolve([]))
    input.addEventListener("change", () => resolve(uploadToServer(Array.from(input.files ?? []))))
    input.click()
  })
}

/** On a server, results go to a fresh folder the client then downloads. */
const exportDir = () => call("new_export_dir", {})

export async function pickFolder(title: string, defaultPath?: string | null): Promise<string | null> {
  if (backend === "remote") return exportDir()
  if (!inTauri) return mockDialogs.pickFolder()
  return single(await open({ directory: true, title, defaultPath: defaultPath ?? undefined }))
}

export async function pickIpa(): Promise<string | null> {
  if (backend === "remote") return single(await pickAndUpload(".ipa", false))
  if (!inTauri) return mockDialogs.pickIpa()
  return single(
    await open({ title: "Choose an IPA", filters: [{ name: "iOS app", extensions: ["ipa"] }] }),
  )
}

export async function pickP12(defaultPath?: string | null): Promise<string | null> {
  if (backend === "remote") return single(await pickAndUpload(".p12,.pfx", false))
  if (!inTauri) return mockDialogs.pickP12()
  return single(
    await open({
      title: "Choose a signing certificate",
      defaultPath: defaultPath ?? undefined,
      filters: [{ name: "Certificate with private key", extensions: ["p12", "pfx"] }],
    }),
  )
}

export async function pickProfile(defaultPath?: string | null): Promise<string | null> {
  if (backend === "remote") return single(await pickAndUpload(".mobileprovision", false))
  if (!inTauri) return mockDialogs.pickProfile()
  return single(
    await open({
      title: "Choose a provisioning profile",
      defaultPath: defaultPath ?? undefined,
      filters: [{ name: "Provisioning profile", extensions: ["mobileprovision"] }],
    }),
  )
}

export async function pickSaveIpa(defaultPath: string): Promise<string | null> {
  if (backend === "remote") return `${await exportDir()}/${defaultPath}`
  if (!inTauri) return mockDialogs.pickSavePath(defaultPath).then((p) => p.replace("/Pictures/", "/Downloads/"))
  return save({ title: "Save re-signed IPA", defaultPath, filters: [{ name: "iOS app", extensions: ["ipa"] }] })
}

export async function pickFiles(title: string): Promise<string[]> {
  if (backend === "remote") return pickAndUpload("", true)
  if (!inTauri) return mockDialogs.pickFiles()
  const result = await open({ multiple: true, title })
  if (!result) return []
  return Array.isArray(result) ? result : [result]
}

export async function pickSavePath(title: string, defaultPath: string): Promise<string | null> {
  if (backend === "remote") return `${await exportDir()}/${defaultPath}`
  if (!inTauri) return mockDialogs.pickSavePath(defaultPath)
  return save({ title, defaultPath, filters: [{ name: "PNG image", extensions: ["png"] }] })
}

/** Shows a result: in the file manager on the desktop, as a download on a remote client. */
export async function revealPath(path: string): Promise<void> {
  if (backend === "remote") return downloadFromServer(path)
  if (!inTauri) return
  await revealItemInDir(path)
}

/** Opens Orchard's data folder in the system file manager. */
export const openDataDir = () => call("open_data_dir", {})
