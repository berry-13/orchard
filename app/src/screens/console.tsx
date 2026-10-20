import { useVirtualizer } from "@tanstack/react-virtual"
import { CircleStop, Eraser, Pause, Play, RotateCcw, Search, SquareTerminal } from "lucide-react"
import { useEffect, useMemo, useRef, useState } from "react"
import { toast } from "sonner"

import { EmptyState } from "@/components/empty-state"
import { NoDevice } from "@/components/no-device"
import { PageHeader } from "@/components/page-header"
import { Button } from "@/components/ui/button"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { errorMessage, EVENTS, startSyslog, stopSyslog, subscribe, type SyslogLine } from "@/lib/api"
import { useDevices } from "@/lib/devices"
import { formatNumber } from "@/lib/format"
import { cn } from "@/lib/utils"

const MAX_LINES = 5000
const ROW_HEIGHT = 22
const ALL = "__all__"

interface Entry extends SyslogLine {
  id: number
}

const LEVEL_CLASS: Record<SyslogLine["level"], string> = {
  default: "text-muted-foreground",
  info: "text-muted-foreground",
  notice: "text-muted-foreground",
  debug: "text-muted-foreground",
  error: "text-destructive",
  fault: "text-destructive font-medium",
}

/** Streams syslog for a device into a capped buffer, flushing once per frame. */
function useSyslog(udid: string, paused: boolean) {
  const nextId = useRef(0)
  const pausedRef = useRef(paused)
  const [lines, setLines] = useState<Entry[]>([])
  const [processes, setProcesses] = useState<string[]>([])
  /** Set when the stream stopped on its own or failed to start. */
  const [ended, setEnded] = useState<{ message: string | null } | null>(null)
  const [session, setSession] = useState(0)

  useEffect(() => {
    pausedRef.current = paused
  }, [paused])

  useEffect(() => {
    let frame = 0
    let pending: Entry[] = []
    const seen = new Set<string>()
    const flush = () => {
      frame = 0
      const batch = pending
      pending = []
      setLines((prev) => {
        const next = prev.concat(batch)
        return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next
      })
      setProcesses((list) => (list.length === seen.size ? list : [...seen].sort((a, b) => a.localeCompare(b))))
    }
    const unlisten = subscribe(EVENTS.syslog, (line) => {
      if (line.udid !== udid || pausedRef.current) return
      pending.push({ ...line, id: nextId.current++ })
      seen.add(line.process)
      if (!frame) frame = requestAnimationFrame(flush)
    })
    const unlistenStatus = subscribe(EVENTS.syslogStatus, (e) => {
      if (e.udid === udid) setEnded({ message: e.message })
    })
    startSyslog(udid).catch((e) => {
      toast.error("Could not start the console", { description: errorMessage(e) })
      setEnded({ message: errorMessage(e) })
    })
    return () => {
      if (frame) cancelAnimationFrame(frame)
      void unlisten.then((fn) => fn())
      void unlistenStatus.then((fn) => fn())
      void stopSyslog().catch(() => undefined)
    }
  }, [udid, session])

  return {
    lines,
    processes,
    ended,
    clear: () => setLines([]),
    reconnect: () => {
      setEnded(null)
      setSession((s) => s + 1)
    },
  }
}

function LogView({ udid }: { udid: string }) {
  const [paused, setPaused] = useState(false)
  const [filter, setFilter] = useState("")
  const [process, setProcess] = useState(ALL)
  const [autoscroll, setAutoscroll] = useState(true)
  const { lines, processes, ended, clear, reconnect } = useSyslog(udid, paused)
  const scrollRef = useRef<HTMLDivElement>(null)

  const visible = useMemo(() => {
    const q = filter.trim().toLowerCase()
    if (!q && process === ALL) return lines
    return lines.filter(
      (l) =>
        (process === ALL || l.process === process) &&
        (!q || l.message.toLowerCase().includes(q) || l.process.toLowerCase().includes(q)),
    )
  }, [lines, filter, process])

  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack Virtual is not memo-safe; this component does not memoize its results.
  const virtualizer = useVirtualizer({
    count: visible.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 20,
  })

  useEffect(() => {
    if (autoscroll && visible.length > 0) virtualizer.scrollToIndex(visible.length - 1, { align: "end" })
  }, [autoscroll, visible.length, virtualizer])

  const status = ended
    ? `Stream stopped${ended.message ? `: ${ended.message}` : ""}. ${formatNumber(lines.length)} lines kept.`
    : paused
    ? `Paused, new lines are dropped. ${formatNumber(visible.length)} lines shown.`
    : `Streaming. ${formatNumber(visible.length)} of ${formatNumber(lines.length)} lines shown.`

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b px-4 py-2">
        <Button variant="outline" size="sm" onClick={() => setPaused((p) => !p)} aria-pressed={paused}>
          {paused ? <Play aria-hidden="true" /> : <Pause aria-hidden="true" />}
          {paused ? "Resume" : "Pause"}
        </Button>
        <Button variant="ghost" size="sm" onClick={clear}>
          <Eraser aria-hidden="true" />
          Clear
        </Button>
        <InputGroup className="h-7 w-56">
          <InputGroupAddon>
            <Search aria-hidden="true" />
          </InputGroupAddon>
          <InputGroupInput
            placeholder="Filter messages"
            aria-label="Filter messages"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        </InputGroup>
        <Select value={process} onValueChange={setProcess}>
          <SelectTrigger size="sm" className="w-48" aria-label="Process">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL}>All processes</SelectItem>
            {processes.map((p) => (
              <SelectItem key={p} value={p}>
                {p}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <div className="ml-auto flex items-center gap-2">
          <Switch id="autoscroll" checked={autoscroll} onCheckedChange={setAutoscroll} />
          <Label htmlFor="autoscroll" className="font-normal">
            Autoscroll
          </Label>
        </div>
      </div>
      <p className="sr-only" role="status" aria-live="polite">
        {status}
      </p>
      {ended ? (
        <div className="flex items-center gap-3 border-b bg-muted/40 px-4 py-2 text-sm">
          <CircleStop className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
          <p className="min-w-0 flex-1 truncate" title={ended.message ?? undefined}>
            The log stream stopped
            {ended.message ? <span className="text-muted-foreground">: {ended.message}</span> : "."}
          </p>
          <Button variant="outline" size="sm" onClick={reconnect}>
            <RotateCcw aria-hidden="true" />
            Reconnect
          </Button>
        </div>
      ) : null}
      {visible.length === 0 ? (
        <div className="flex flex-1 p-4">
          <EmptyState
            icon={SquareTerminal}
            title={lines.length ? "No matching lines" : paused ? "Console paused" : "Waiting for log lines"}
            description={
              lines.length
                ? "Change the filter or pick another process."
                : paused
                  ? "Resume to keep reading the device log."
                  : "Messages from the device appear here as they are logged."
            }
          />
        </div>
      ) : (
        <div
          ref={scrollRef}
          role="log"
          aria-live="off"
          aria-label="Device log"
          tabIndex={0}
          onWheel={(e) => {
            if (e.deltaY < 0 && autoscroll) setAutoscroll(false)
          }}
          className="min-h-0 flex-1 overflow-auto bg-muted/20 font-mono text-xs outline-none focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:ring-inset"
        >
          <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
            {virtualizer.getVirtualItems().map((row) => {
              const l = visible[row.index]
              return (
                <div
                  key={l.id}
                  className="absolute top-0 left-0 grid w-full grid-cols-[9.5rem_11rem_3.5rem_1fr] items-center gap-3 px-4 hover:bg-muted/60"
                  style={{ height: ROW_HEIGHT, transform: `translateY(${row.start}px)` }}
                >
                  <span className="truncate text-muted-foreground tabular-nums">{l.timestamp}</span>
                  <span className="truncate" title={`${l.process}[${l.pid ?? "?"}]`}>
                    {l.process}
                    {l.pid !== null ? <span className="text-muted-foreground">[{l.pid}]</span> : null}
                  </span>
                  <span className={cn("truncate", LEVEL_CLASS[l.level])}>{l.level}</span>
                  <span className="truncate" title={l.message}>
                    {l.message}
                  </span>
                </div>
              )
            })}
          </div>
        </div>
      )}
      <div className="flex h-7 items-center gap-2 border-t px-4 text-xs text-muted-foreground tabular-nums" aria-hidden="true">
        <span
          className={cn(
            "size-1.5 rounded-full",
            ended ? "bg-destructive" : paused ? "bg-muted-foreground" : "bg-emerald-500",
          )}
        />
        {ended ? "Stopped" : paused ? "Paused" : "Live"} · {formatNumber(visible.length)} lines
        {lines.length >= MAX_LINES ? ` · keeping the last ${formatNumber(MAX_LINES)}` : ""}
      </div>
    </div>
  )
}

export default function ConsoleScreen() {
  const { device, loading } = useDevices()
  return (
    <div className="flex h-full min-h-0 flex-col">
      <PageHeader title="Console" description={device ? device.name : undefined} />
      {loading ? null : device ? (
        <LogView key={device.udid} udid={device.udid} />
      ) : (
        <div className="flex flex-1 flex-col p-4 md:p-6">
          <NoDevice />
        </div>
      )}
    </div>
  )
}
