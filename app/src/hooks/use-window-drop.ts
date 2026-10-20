import { getCurrentWebview } from "@tauri-apps/api/webview"
import { useEffect, useSyncExternalStore } from "react"

import { currentBackend, uploadToServer } from "@/lib/api"

/** Returns true when it handled the dropped paths. */
export type DropHandler = (paths: string[]) => boolean

export interface DropTarget {
  handler: DropHandler
  /** Overlay text shown while dragging, e.g. "Drop to upload". */
  label: string
}

const targets: DropTarget[] = []
let dragging = false
const subscribers = new Set<() => void>()

function setDragging(value: boolean) {
  if (dragging === value) return
  dragging = value
  subscribers.forEach((s) => s())
}

function dispatch(paths: string[]) {
  setDragging(false)
  if (paths.length === 0) return
  for (let i = targets.length - 1; i >= 0; i--) if (targets[i].handler(paths)) return
}

/** The most recently registered target, used for the overlay label. */
export function activeDropLabel(): string {
  return targets[targets.length - 1]?.label ?? "Drop files"
}

let installed = false

function install() {
  if (installed) return
  installed = true
  if (currentBackend() === "desktop") {
    void getCurrentWebview().onDragDropEvent((event) => {
      const p = event.payload
      if (p.type === "enter" || p.type === "over") setDragging(true)
      else if (p.type === "leave") setDragging(false)
      else if (p.type === "drop") dispatch(p.paths)
    })
    return
  }
  // In a browser only File objects are available: a remote client uploads them
  // to the server first, the preview hands their names to the mock as paths.
  let depth = 0
  const hasFiles = (e: DragEvent) => e.dataTransfer?.types.includes("Files") ?? false
  window.addEventListener("dragenter", (e) => {
    if (!hasFiles(e)) return
    depth++
    setDragging(true)
  })
  window.addEventListener("dragover", (e) => {
    if (hasFiles(e)) e.preventDefault()
  })
  window.addEventListener("dragleave", (e) => {
    if (!hasFiles(e)) return
    depth = Math.max(0, depth - 1)
    if (depth === 0) setDragging(false)
  })
  window.addEventListener("drop", (e) => {
    if (!hasFiles(e)) return
    e.preventDefault()
    depth = 0
    const files = Array.from(e.dataTransfer?.files ?? [])
    if (currentBackend() === "remote") {
      setDragging(false)
      void uploadToServer(files).then(dispatch)
    } else {
      dispatch(files.map((f) => `/home/you/Downloads/${f.name}`))
    }
  })
}

/** Registers a window level drop target while the calling component is mounted. */
export function useDropTarget(target: DropTarget | null) {
  useEffect(() => {
    install()
    if (!target) return
    targets.push(target)
    return () => {
      const i = targets.lastIndexOf(target)
      if (i >= 0) targets.splice(i, 1)
    }
  }, [target])
}

/** True while files are dragged over the window. */
export function useDragging(): boolean {
  return useSyncExternalStore(
    (cb) => {
      subscribers.add(cb)
      return () => subscribers.delete(cb)
    },
    () => dragging,
  )
}
