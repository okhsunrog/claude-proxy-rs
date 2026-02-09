/** Extract a human-readable error message from an unknown caught value */
export function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message
  if (typeof e === 'object' && e !== null && 'error' in e) {
    return String((e as { error: unknown }).error)
  }
  if (typeof e === 'string') return e
  return 'Unknown error'
}

/** Format microdollars as a dollar string */
export function formatCost(microdollars: number): string {
  const dollars = microdollars / 1_000_000
  if (dollars >= 1000) return `$${(dollars / 1000).toFixed(1)}K`
  if (dollars >= 100) return `$${dollars.toFixed(0)}`
  if (dollars >= 1) return `$${dollars.toFixed(2)}`
  if (dollars >= 0.01) return `$${dollars.toFixed(3)}`
  if (microdollars === 0) return '$0'
  return `$${dollars.toFixed(4)}`
}

/** Convert microdollars to dollars (null-safe) */
export function microToDollars(micro: number | null | undefined): number | null {
  if (micro == null) return null
  return micro / 1_000_000
}

/** Convert dollars to microdollars (null-safe) */
export function dollarsToMicro(dollars: number | null): number | null {
  if (dollars == null) return null
  return Math.round(dollars * 1_000_000)
}
