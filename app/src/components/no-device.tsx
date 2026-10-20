import { Cable } from "lucide-react"

import { EmptyState } from "@/components/empty-state"

export function NoDevice() {
  return (
    <EmptyState
      icon={Cable}
      title="No device connected"
      description="Connect an iPhone or iPad with a cable and tap Trust on the device, or add one over Wi-Fi."
    />
  )
}
