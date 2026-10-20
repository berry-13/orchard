import { useQueryClient } from "@tanstack/react-query"
import { Server } from "lucide-react"
import { useState, useSyncExternalStore } from "react"

import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { InputOTP, InputOTPGroup, InputOTPSeparator, InputOTPSlot } from "@/components/ui/input-otp"
import { Label } from "@/components/ui/label"
import { currentBackend, errorMessage, inTauri } from "@/lib/api"
import { connect, needsServerAddress, onRemoteStatus, remoteStatus, serverUrl } from "@/lib/remote"

const CODE_PATTERN = "^[A-Za-z0-9]*$"

/** A name the server lists the client under. */
function clientName(): string {
  if (inTauri) return "Orchard for iPad"
  const ua = navigator.userAgent
  const browser = /Edg\//.test(ua) ? "Edge" : /Chrome\//.test(ua) ? "Chrome" : /Firefox\//.test(ua) ? "Firefox" : "Safari"
  return `${browser} on ${navigator.platform || "this device"}`
}

/** Connects a remote client to an Orchard server with the code it prints. */
function ConnectServer() {
  const queryClient = useQueryClient()
  const askAddress = needsServerAddress()
  const [address, setAddress] = useState("")
  const [code, setCode] = useState("")
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (value = code) => {
    if (busy || value.length !== 8 || (askAddress && !address.trim())) return
    setBusy(true)
    setError(null)
    try {
      await connect(askAddress ? address : null, value, clientName())
      // Data cached for another server must not show up for this one.
      queryClient.clear()
    } catch (e) {
      setError(errorMessage(e))
      setCode("")
    } finally {
      setBusy(false)
    }
  }

  return (
    <main className="flex min-h-full items-center justify-center p-4">
      <Card className="w-full max-w-sm animate-in fade-in-0 duration-200">
        <form
          className="grid gap-6"
          onSubmit={(e) => {
            e.preventDefault()
            void submit()
          }}
        >
          <CardHeader className="items-center text-center">
            <div className="mx-auto mb-2 flex size-10 items-center justify-center rounded-lg border bg-muted/50">
              <Server className="size-5 text-muted-foreground" aria-hidden="true" />
            </div>
            <CardTitle role="heading" aria-level={1}>
              Connect to Orchard
            </CardTitle>
            <CardDescription>
              Enter the connect code printed by <span className="font-mono text-foreground">orchard-server</span>.
              Run <span className="font-mono text-foreground">orchard-server code</span> there for a new one.
            </CardDescription>
          </CardHeader>
          <CardContent className="grid gap-4">
            {askAddress ? (
              <div className="grid gap-2">
                <Label htmlFor="server-address">Server address</Label>
                <Input
                  id="server-address"
                  inputMode="url"
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck={false}
                  placeholder="192.168.1.10:7420"
                  value={address}
                  onChange={(e) => setAddress(e.target.value)}
                  required
                  autoFocus
                />
              </div>
            ) : (
              <p className="text-center text-sm text-muted-foreground">
                Server <span className="font-mono text-foreground">{serverUrl()}</span>
              </p>
            )}
            <div className="grid justify-items-center gap-2">
              <Label htmlFor="server-code" className={askAddress ? "justify-self-start" : "sr-only"}>
                Connect code
              </Label>
              <InputOTP
                id="server-code"
                maxLength={8}
                value={code}
                onChange={(v) => {
                  const upper = v.toUpperCase()
                  setCode(upper)
                  if (upper.length === 8) void submit(upper)
                }}
                pattern={CODE_PATTERN}
                autoCapitalize="characters"
                autoComplete="one-time-code"
                autoFocus={!askAddress}
                aria-invalid={error ? true : undefined}
                aria-describedby={error ? "server-error" : undefined}
                disabled={busy}
              >
                <InputOTPGroup>
                  <InputOTPSlot index={0} />
                  <InputOTPSlot index={1} />
                  <InputOTPSlot index={2} />
                  <InputOTPSlot index={3} />
                </InputOTPGroup>
                <InputOTPSeparator />
                <InputOTPGroup>
                  <InputOTPSlot index={4} />
                  <InputOTPSlot index={5} />
                  <InputOTPSlot index={6} />
                  <InputOTPSlot index={7} />
                </InputOTPGroup>
              </InputOTP>
            </div>
            {error ? (
              <p id="server-error" className="text-sm text-destructive" role="alert">
                {error}
              </p>
            ) : null}
          </CardContent>
          <CardFooter>
            <Button
              type="submit"
              className="w-full"
              disabled={busy || code.length !== 8 || (askAddress && !address.trim())}
            >
              {busy ? "Connecting..." : "Connect"}
            </Button>
          </CardFooter>
        </form>
      </Card>
    </main>
  )
}

/** Shows the connect screen instead of the app while a remote client is signed out. */
export function RemoteGate({ children }: { children: React.ReactNode }) {
  const status = useSyncExternalStore(onRemoteStatus, remoteStatus)
  if (currentBackend() !== "remote" || status === "connected") return children
  return <ConnectServer />
}
