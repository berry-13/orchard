import { Star } from "lucide-react"

import { formatCompact } from "@/lib/format"
import { cn } from "@/lib/utils"

/** Five star rating, announced as a single label. */
export function Rating({
  value,
  count,
  className,
}: {
  value: number | null
  count?: number
  className?: string
}) {
  if (value === null) return <span className={cn("text-xs text-muted-foreground", className)}>No ratings</span>
  const label = `Rated ${value.toFixed(1)} out of 5${count !== undefined ? `, ${count} ratings` : ""}`
  return (
    <span className={cn("inline-flex items-center gap-1.5", className)} role="img" aria-label={label}>
      <span className="flex items-center gap-px" aria-hidden="true">
        {Array.from({ length: 5 }, (_, i) => {
          const fill = Math.max(0, Math.min(1, value - i))
          return (
            <span key={i} className="relative size-3">
              <Star className="absolute inset-0 size-3 text-muted-foreground/40" />
              {fill > 0 ? (
                <span className="absolute inset-0 overflow-hidden" style={{ width: `${fill * 100}%` }}>
                  <Star className="size-3 fill-amber-400 text-amber-400" />
                </span>
              ) : null}
            </span>
          )
        })}
      </span>
      <span className="text-xs text-muted-foreground tabular-nums" aria-hidden="true">
        {value.toFixed(1)}
        {count !== undefined ? ` (${formatCompact(count)})` : ""}
      </span>
    </span>
  )
}
