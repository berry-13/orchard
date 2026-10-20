/**
 * Talking to an Orchard server (`orchard-server`): from the web app it serves,
 * from the iPad app, or from a dev build pointed at one with `?server=URL`.
 * Commands go over HTTP, events over one WebSocket, files over uploads and
 * download links.
 */
import { isTauri } from "@tauri-apps/api/core"
import { openUrl } from "@tauri-apps/plugin-opener"

import { readStored, writeStored } from "@/lib/storage"

const SERVER_KEY = "orchard.server"
/** Tokens are stored per server origin, so one is only ever sent to the server that issued it. */
const tokenKey = (origin: string) => `orchard.token:${origin || window.location.origin}`

/** Server origin, "" when it is the page's own origin. */
let base = readStored(SERVER_KEY) ?? ""
let token = readStored(tokenKey(base))

/** Points the client at another server, with that server's own token. */
function setBase(next: string) {
  base = next
  writeStored(SERVER_KEY, next || null)
  token = readStored(tokenKey(next))
}

export type RemoteStatus = "connected" | "signedOut"
const statusListeners = new Set<() => void>()

function notifyStatus() {
  statusListeners.forEach((l) => l())
}

export function remoteStatus(): RemoteStatus {
  return token ? "connected" : "signedOut"
}

export function onRemoteStatus(listener: () => void): () => void {
  statusListeners.add(listener)
  return () => statusListeners.delete(listener)
}

/** The server this client talks to, for display; the page origin when same origin. */
export function serverUrl(): string {
  return base || window.location.origin
}

/** Whether the server address has to be entered (no same origin server). */
export function needsServerAddress(): boolean {
  return !base && !sameOrigin
}

let sameOrigin = false

function normalizeUrl(url: string): string {
  const trimmed = url.trim().replace(/\/+$/, "")
  return /^https?:\/\//i.test(trimmed) ? trimmed : `http://${trimmed}`
}

/** True when `url` (or the page origin for "") answers like an Orchard server. */
export async function probe(url: string): Promise<boolean> {
  try {
    const res = await fetch(`${url}/api/hello`, { signal: AbortSignal.timeout(4000) })
    if (!res.ok) return false
    const body = (await res.json()) as { name?: string }
    return body.name === "orchard"
  } catch {
    return false
  }
}

/** Uses the page's own origin as the server when it serves Orchard. */
export async function detectSameOrigin(): Promise<boolean> {
  sameOrigin = !base && (await probe(""))
  return sameOrigin
}

/** A server stored earlier, or passed as `?server=` (dev builds). */
export function configuredServer(): string | null {
  // Dev builds only: a link must not be able to point a real client elsewhere.
  const param = import.meta.env.DEV ? new URLSearchParams(window.location.search).get("server") : null
  if (param) setBase(normalizeUrl(param))
  return base || null
}

export class RemoteError extends Error {
  readonly kind: string

  constructor(kind: string, message: string) {
    super(message)
    this.kind = kind
  }
}

function signedOut() {
  token = null
  writeStored(tokenKey(base), null)
  closeSocket()
  notifyStatus()
}

async function failure(res: Response): Promise<RemoteError> {
  if (res.status === 401) signedOut()
  try {
    const body = (await res.json()) as { kind?: string; message?: string }
    return new RemoteError(body.kind ?? "unknown", body.message ?? res.statusText)
  } catch {
    return new RemoteError("network", `The server answered ${res.status} ${res.statusText}`)
  }
}

async function request(path: string, init: RequestInit): Promise<Response> {
  try {
    return await fetch(`${base}${path}`, {
      ...init,
      headers: { ...init.headers, ...(token ? { Authorization: `Bearer ${token}` } : {}) },
    })
  } catch {
    throw new RemoteError("network", `Could not reach the Orchard server at ${serverUrl()}`)
  }
}

/** Exchanges the connect code shown by the server for a token. */
export async function connect(url: string | null, code: string, client: string): Promise<void> {
  if (url !== null) {
    const normalized = normalizeUrl(url)
    if (!(await probe(normalized))) {
      throw new RemoteError("network", `No Orchard server answers at ${normalized}`)
    }
    setBase(normalized)
  }
  const res = await request("/api/auth", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code, client }),
  })
  if (!res.ok) throw await failure(res)
  token = ((await res.json()) as { token: string }).token
  writeStored(tokenKey(base), token)
  notifyStatus()
}

/** Forgets the token (and the server, when it was entered by hand). */
export function disconnect(): void {
  // Drop this server's token before forgetting which server it was.
  signedOut()
  if (!sameOrigin) setBase("")
}

export async function remoteCall<T>(command: string, args: unknown): Promise<T> {
  const res = await request(`/api/call/${command}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(args ?? {}),
  })
  if (!res.ok) throw await failure(res)
  return (await res.json()) as T
}

/** Sends a file to the server and returns its path there. */
export function uploadFile(file: File, onProgress?: (sent: number, total: number) => void): Promise<string> {
  return new Promise((resolve, reject) => {
    // XHR rather than fetch: it reports upload progress.
    const xhr = new XMLHttpRequest()
    xhr.open("POST", `${base}/api/upload?name=${encodeURIComponent(file.name)}`)
    if (token) xhr.setRequestHeader("Authorization", `Bearer ${token}`)
    xhr.upload.onprogress = (e) => onProgress?.(e.loaded, e.total)
    xhr.onload = () => {
      try {
        const body = JSON.parse(xhr.responseText) as { path?: string; kind?: string; message?: string }
        if (xhr.status === 200 && body.path) return resolve(body.path)
        if (xhr.status === 401) signedOut()
        reject(new RemoteError(body.kind ?? "unknown", body.message ?? `Upload failed (${xhr.status})`))
      } catch {
        reject(new RemoteError("network", `Upload failed (${xhr.status})`))
      }
    }
    xhr.onerror = () => reject(new RemoteError("network", "The upload was interrupted"))
    xhr.send(file)
  })
}

/** What the iPad app needs to open a relay WebSocket to the server itself. */
export function relayTarget(path: string): { url: string; token: string } {
  const origin = base || window.location.origin
  return { url: `${origin.replace(/^http/i, "ws")}${path}`, token: token ?? "" }
}

/** Downloads a server file, or a folder as a zip, to this device. */
export async function downloadFromServer(path: string): Promise<void> {
  const url = `${base || window.location.origin}${await remoteCall<string>("download_ticket", { path })}`
  // The iPad app's web view cannot save downloads; Safari saves them to Files.
  if (isTauri()) return openUrl(url)
  const a = document.createElement("a")
  a.href = url
  a.download = ""
  a.rel = "noopener"
  document.body.append(a)
  a.click()
  a.remove()
}

/* ------------------------------------------------------------------ */
/* Events                                                              */
/* ------------------------------------------------------------------ */

const listeners = new Map<string, Set<(payload: unknown) => void>>()
let socket: WebSocket | null = null
let retry: ReturnType<typeof setTimeout> | null = null

function closeSocket() {
  if (retry) clearTimeout(retry)
  retry = null
  const s = socket
  socket = null
  s?.close()
}

function openSocket() {
  if (socket || !token) return
  const origin = base || window.location.origin
  const ws = new WebSocket(`${origin.replace(/^http/i, "ws")}/api/events`)
  socket = ws
  ws.onopen = () => ws.send(JSON.stringify({ token }))
  ws.onmessage = (e) => {
    try {
      const { event, payload } = JSON.parse(String(e.data)) as { event: string; payload: unknown }
      listeners.get(event)?.forEach((l) => l(payload))
    } catch {
      // Not an event frame.
    }
  }
  ws.onclose = (e) => {
    if (socket !== ws) return
    socket = null
    if (e.code === 4401) return signedOut()
    // The server restarted or the network dropped: try again shortly.
    retry = setTimeout(openSocket, 2000)
  }
}

onRemoteStatus(() => {
  if (token && listeners.size) openSocket()
})

export function remoteListen(event: string, handler: (payload: unknown) => void): () => void {
  let set = listeners.get(event)
  if (!set) listeners.set(event, (set = new Set()))
  set.add(handler)
  openSocket()
  return () => {
    set.delete(handler)
  }
}
