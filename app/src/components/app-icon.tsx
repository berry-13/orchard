import { useQuery } from "@tanstack/react-query"
import { useEffect, useRef, useState } from "react"

import { appIcon } from "@/lib/api"
import { cn } from "@/lib/utils"

/** Device app icon, fetched only once it scrolls into view. */
export function AppIcon({
  udid,
  bundleId,
  name,
  className,
}: {
  udid: string
  bundleId: string
  name: string
  className?: string
}) {
  const ref = useRef<HTMLSpanElement>(null)
  const [visible, setVisible] = useState(false)

  useEffect(() => {
    const el = ref.current
    if (!el) return
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          setVisible(true)
          observer.disconnect()
        }
      },
      { rootMargin: "200px" },
    )
    observer.observe(el)
    return () => observer.disconnect()
  }, [])

  const { data } = useQuery({
    queryKey: ["appIcon", udid, bundleId],
    queryFn: () => appIcon(udid, bundleId),
    enabled: visible,
    staleTime: Infinity,
  })

  return (
    <span
      ref={ref}
      className={cn(
        "relative flex size-8 shrink-0 items-center justify-center overflow-hidden rounded-[22%] bg-muted text-xs font-medium text-muted-foreground",
        className,
      )}
      aria-hidden="true"
    >
      {data ? (
        <img src={data} alt="" className="size-full animate-in fade-in-0 duration-200" draggable={false} />
      ) : (
        name.charAt(0).toUpperCase()
      )}
    </span>
  )
}
