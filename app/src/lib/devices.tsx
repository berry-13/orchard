import { useQuery, useQueryClient } from "@tanstack/react-query"
import { createContext, useContext, useEffect, useMemo, useState } from "react"

import { deviceInfo, EVENTS, listDevices, subscribe, type Device } from "@/lib/api"
import { readStored, writeStored } from "@/lib/storage"

const STORAGE_KEY = "orchard.device"

interface DeviceContextValue {
  devices: Device[]
  loading: boolean
  /** The selected device, null when none is connected. */
  device: Device | null
  select: (udid: string) => void
}

const DeviceContext = createContext<DeviceContextValue | null>(null)

export const deviceKeys = {
  list: ["devices"] as const,
  info: (udid: string) => ["device", udid] as const,
}

export function DeviceProvider({ children }: { children: React.ReactNode }) {
  const queryClient = useQueryClient()
  const { data: devices = [], isPending } = useQuery({ queryKey: deviceKeys.list, queryFn: listDevices })
  const [selected, setSelected] = useState<string | null>(() => readStored(STORAGE_KEY))

  useEffect(() => {
    const unlisten = subscribe(EVENTS.devices, (e) => {
      void queryClient.invalidateQueries({ queryKey: deviceKeys.list })
      void queryClient.invalidateQueries({ queryKey: deviceKeys.info(e.udid) })
    })
    return () => {
      void unlisten.then((fn) => fn())
    }
  }, [queryClient])

  const device = devices.find((d) => d.udid === selected) ?? devices[0] ?? null

  const value = useMemo<DeviceContextValue>(
    () => ({
      devices,
      loading: isPending,
      device,
      select: (udid) => {
        writeStored(STORAGE_KEY, udid)
        setSelected(udid)
      },
    }),
    [devices, isPending, device],
  )
  return <DeviceContext.Provider value={value}>{children}</DeviceContext.Provider>
}

export function useDevices() {
  const ctx = useContext(DeviceContext)
  if (!ctx) throw new Error("useDevices must be used inside DeviceProvider")
  return ctx
}

export function useDeviceInfo(udid: string | undefined) {
  return useQuery({
    queryKey: deviceKeys.info(udid ?? ""),
    queryFn: () => deviceInfo(udid!),
    enabled: !!udid,
  })
}
