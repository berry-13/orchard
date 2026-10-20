import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { ArrowLeft, ShieldCheck, ShoppingBag } from "lucide-react"
import { useState } from "react"
import { toast } from "sonner"

import { COUNTRIES } from "@/components/store/countries"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { InputOTP, InputOTPGroup, InputOTPSeparator, InputOTPSlot } from "@/components/ui/input-otp"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { errorMessage, getSettings, isOrchardError, storeSignIn } from "@/lib/api"

/** Apple ID sign in, with a second step for the two-factor code. */
export function SignInCard() {
  const queryClient = useQueryClient()
  const settings = useQuery({ queryKey: ["settings"], queryFn: getSettings })
  const native = settings.data?.storeBackend === "native"
  const [email, setEmail] = useState("")
  const [password, setPassword] = useState("")
  const [country, setCountry] = useState("US")
  const [code, setCode] = useState("")
  const [step, setStep] = useState<"credentials" | "code">("credentials")
  const [error, setError] = useState<string | null>(null)
  const [useBuiltIn, setUseBuiltIn] = useState(false)

  const signIn = useMutation({
    mutationFn: (withCode: string | null) => storeSignIn(email.trim(), password, withCode, country, useBuiltIn),
    onMutate: () => setError(null),
    onSuccess: (account) => {
      queryClient.setQueryData(["storeAccount"], account)
      toast.success(`Signed in as ${account.email}`)
    },
    onError: (e) => {
      if (isOrchardError(e, "authCodeRequired")) {
        setStep("code")
        setCode("")
        return
      }
      setError(errorMessage(e))
      if (step === "code") setCode("")
    },
  })

  const busy = signIn.isPending

  return (
    <div className="flex flex-1 items-center justify-center py-8">
      <Card className="w-full max-w-sm animate-in fade-in-0 duration-200">
        {step === "credentials" ? (
          <form
            className="grid gap-6"
            onSubmit={(e) => {
              e.preventDefault()
              signIn.mutate(null)
            }}
          >
            <CardHeader className="items-center text-center">
              <div className="mx-auto mb-2 flex size-10 items-center justify-center rounded-lg border bg-muted/50">
                <ShoppingBag className="size-5 text-muted-foreground" aria-hidden="true" />
              </div>
              <CardTitle>Sign in to the App Store</CardTitle>
              <CardDescription>Use your Apple ID to search, download and install apps.</CardDescription>
            </CardHeader>
            <CardContent className="grid gap-4">
              <div className="grid gap-2">
                <Label htmlFor="store-email">Apple ID</Label>
                <Input
                  id="store-email"
                  type="email"
                  autoComplete="username"
                  placeholder="name@example.com"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  required
                  autoFocus
                />
              </div>
              <div className="grid gap-2">
                <Label htmlFor="store-password">Password</Label>
                <Input
                  id="store-password"
                  type="password"
                  autoComplete="current-password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  aria-invalid={error ? true : undefined}
                  aria-describedby={error ? "store-error" : undefined}
                  required
                />
              </div>
              <div className="grid gap-2">
                <Label htmlFor="store-country">Country or region</Label>
                <Select value={country} onValueChange={setCountry}>
                  <SelectTrigger id="store-country" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {COUNTRIES.map((c) => (
                      <SelectItem key={c.code} value={c.code}>
                        {c.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              {native ? null : (
                <div className="flex items-start gap-2">
                  <Checkbox
                    id="store-builtin"
                    checked={useBuiltIn}
                    onCheckedChange={(v) => setUseBuiltIn(v === true)}
                    aria-describedby="store-builtin-hint"
                    className="mt-0.5"
                  />
                  <div className="grid gap-1">
                    <Label htmlFor="store-builtin" className="font-normal">
                      Use Orchard's built-in client
                    </Label>
                    <p id="store-builtin-hint" className="text-xs text-muted-foreground">
                      Needed to buy paid apps, which ipatool cannot do. Apple may reject this sign-in.
                    </p>
                  </div>
                </div>
              )}
              {error ? (
                <p id="store-error" className="text-sm text-destructive" role="alert">
                  {error}
                </p>
              ) : null}
            </CardContent>
            <CardFooter className="flex-col gap-3">
              <Button type="submit" className="w-full" disabled={busy || !email.trim() || !password}>
                {busy ? "Signing in..." : "Sign in"}
              </Button>
              <p className="text-center text-xs text-muted-foreground">
                {(!useBuiltIn && settings.data?.storeSignInNote) ||
                  "Your password is sent only to Apple. Orchard keeps a session token on this computer."}
              </p>
              {native ? (
                <p className="rounded-md border bg-muted/50 px-3 py-2 text-xs text-muted-foreground">
                  Apple may reject sign-in from the built-in client. Install{" "}
                  <span className="font-mono text-foreground">ipatool</span> and put it on your PATH (or set{" "}
                  <span className="font-mono text-foreground">ORCHARD_IPATOOL</span>) for reliable sign-in.
                </p>
              ) : null}
            </CardFooter>
          </form>
        ) : (
          <form
            className="grid gap-6"
            onSubmit={(e) => {
              e.preventDefault()
              if (code.length === 6) signIn.mutate(code)
            }}
          >
            <CardHeader className="items-center text-center">
              <div className="mx-auto mb-2 flex size-10 items-center justify-center rounded-lg border bg-muted/50">
                <ShieldCheck className="size-5 text-muted-foreground" aria-hidden="true" />
              </div>
              <CardTitle>Two-factor authentication</CardTitle>
              <CardDescription>Enter the 6 digit code shown on your trusted devices.</CardDescription>
            </CardHeader>
            <CardContent className="grid justify-items-center gap-3">
              <Label htmlFor="store-code" className="sr-only">
                Verification code
              </Label>
              <InputOTP
                id="store-code"
                maxLength={6}
                value={code}
                onChange={(v) => {
                  setCode(v)
                  if (v.length === 6 && !busy) signIn.mutate(v)
                }}
                inputMode="numeric"
                pattern="^[0-9]*$"
                autoComplete="one-time-code"
                autoFocus
                aria-invalid={error ? true : undefined}
                aria-describedby={error ? "store-code-error" : undefined}
                disabled={busy}
              >
                <InputOTPGroup>
                  <InputOTPSlot index={0} />
                  <InputOTPSlot index={1} />
                  <InputOTPSlot index={2} />
                </InputOTPGroup>
                <InputOTPSeparator />
                <InputOTPGroup>
                  <InputOTPSlot index={3} />
                  <InputOTPSlot index={4} />
                  <InputOTPSlot index={5} />
                </InputOTPGroup>
              </InputOTP>
              {error ? (
                <p id="store-code-error" className="text-sm text-destructive" role="alert">
                  {error}
                </p>
              ) : null}
            </CardContent>
            <CardFooter className="flex-col gap-2">
              <Button type="submit" className="w-full" disabled={busy || code.length !== 6}>
                {busy ? "Verifying..." : "Verify"}
              </Button>
              <Button
                type="button"
                variant="ghost"
                className="w-full"
                onClick={() => {
                  setStep("credentials")
                  setError(null)
                }}
              >
                <ArrowLeft aria-hidden="true" />
                Back
              </Button>
            </CardFooter>
          </form>
        )}
      </Card>
    </div>
  )
}
