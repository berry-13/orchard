import { invoke } from "@tauri-apps/api/core"
import { CheckCircle2, Loader2, Wifi, XCircle } from "lucide-react"
import { useEffect, useRef, useState } from "react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { currentBackend, errorMessage, EVENTS, inTauri, startWifiPairing, subscribe } from "@/lib/api"
import { relayTarget } from "@/lib/remote"

type Phase =
  | { kind: "starting" }
  | { kind: "waiting" }
  | { kind: "connected" }
  | { kind: "pin"; pin: string }
  | { kind: "paired"; name: string }
  | { kind: "failed"; message: string }

/** Only the iPad app can announce on its Wi-Fi; a browser cannot. */
export function canPairOverWifi(): boolean {
  return inTauri && currentBackend() === "remote"
}

const stopAnnouncing = () => invoke("plugin:orchard-pairing|stop").catch(() => undefined)

/**
 * Pairs a device with the server over this iPad's Wi-Fi: the server sets up
 * the pairing host, this app announces it and relays the device's connection,
 * and the device shows "Pair with Orchard" in its Developer Mode settings.
 */
export function WifiPairButton() {
  const [open, setOpen] = useState(false)
  const [phase, setPhase] = useState<Phase>({ kind: "starting" })
  const session = useRef<string | null>(null)

  // Server events for this session. Subscribed while open, before announcing.
  useEffect(() => {
    if (!open) return
    const unlisten = subscribe(EVENTS.wifiPairing, (e) => {
      if (e.sessionId !== session.current) return
      if (e.stage === "connected") setPhase({ kind: "connected" })
      else if (e.stage === "pin" && e.pin) setPhase({ kind: "pin", pin: e.pin })
      else if (e.stage === "paired") {
        setPhase({ kind: "paired", name: e.device?.name ?? "Your device" })
        void stopAnnouncing()
      } else if (e.stage === "failed") {
        setPhase({ kind: "failed", message: e.message ?? "Pairing failed" })
        void stopAnnouncing()
      }
    })
    return () => {
      void unlisten.then((fn) => fn())
    }
  }, [open])

  const start = async () => {
    setPhase({ kind: "starting" })
    try {
      const s = await startWifiPairing()
      session.current = s.sessionId
      const relay = relayTarget(s.relayPath)
      await invoke("plugin:orchard-pairing|announce", {
        serviceType: s.serviceType,
        serviceName: s.serviceName,
        txt: s.txt,
        relayUrl: relay.url,
        token: relay.token,
      })
      setPhase({ kind: "waiting" })
    } catch (e) {
      setPhase({ kind: "failed", message: errorMessage(e) })
    }
  }

  const close = (next: boolean) => {
    if (!next) {
      session.current = null
      void stopAnnouncing()
    }
    setOpen(next)
  }

  return (
    <>
      <Button
        variant="outline"
        size="sm"
        onClick={() => {
          setOpen(true)
          void start()
        }}
      >
        <Wifi aria-hidden="true" />
        Pair a device over Wi-Fi...
      </Button>
      <Dialog open={open} onOpenChange={close}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>Pair a device over Wi-Fi</DialogTitle>
            <DialogDescription>
              For iPhone and iPad on iOS 27 with Developer Mode on, on the same Wi-Fi as this iPad.
            </DialogDescription>
          </DialogHeader>

          <div className="grid min-h-40 place-items-center py-2" aria-live="polite">
            {phase.kind === "starting" ? (
              <p className="flex items-center gap-2 text-sm text-muted-foreground">
                <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
                Announcing Orchard on this Wi-Fi...
              </p>
            ) : phase.kind === "waiting" ? (
              <ol className="grid list-decimal gap-2 pl-5 text-sm">
                <li>On the device, open Settings, then Privacy &amp; Security, then Developer Mode.</li>
                <li>
                  Scroll down and tap <span className="font-medium">Pair with Orchard</span>.
                </li>
                <li>Enter the code that appears here.</li>
              </ol>
            ) : phase.kind === "connected" ? (
              <p className="flex items-center gap-2 text-sm text-muted-foreground">
                <Wifi className="size-4" aria-hidden="true" />
                Device connected, preparing the code...
              </p>
            ) : phase.kind === "pin" ? (
              <div className="grid justify-items-center gap-2 text-center">
                <p className="text-sm text-muted-foreground">Enter this code on the device</p>
                <p
                  className="font-mono text-4xl font-semibold tracking-[0.3em] tabular-nums"
                  aria-label={`Code ${phase.pin.split("").join(" ")}`}
                >
                  {phase.pin}
                </p>
              </div>
            ) : phase.kind === "paired" ? (
              <p className="flex items-center gap-2 text-sm">
                <CheckCircle2 className="size-5 text-primary" aria-hidden="true" />
                {phase.name} is paired with the server.
              </p>
            ) : (
              <p className="flex items-start gap-2 text-sm text-destructive" role="alert">
                <XCircle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
                {phase.message}
              </p>
            )}
          </div>

          <DialogFooter>
            {phase.kind === "failed" ? (
              <Button variant="outline" onClick={() => void start()}>
                Try again
              </Button>
            ) : null}
            <Button variant={phase.kind === "paired" ? "default" : "ghost"} onClick={() => close(false)}>
              {phase.kind === "paired" ? "Done" : "Cancel"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
