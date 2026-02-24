import { ref, computed } from 'vue'

export type Period = '24h' | '7d' | '30d'

export interface TimeseriesPoint {
  timestamp: number
  requestCount: number
  costMicrodollars: number
  inputTokens: number
  outputTokens: number
  cacheReadTokens: number
  cacheWriteTokens: number
}

export interface ModelBreakdown {
  model: string
  requestCount: number
  costMicrodollars: number
  inputTokens: number
  outputTokens: number
  cacheReadTokens: number
  cacheWriteTokens: number
}

export interface KeyBreakdown {
  keyId: string
  keyName: string | null
  requestCount: number
  costMicrodollars: number
  inputTokens: number
  outputTokens: number
  cacheReadTokens: number
  cacheWriteTokens: number
}

const period = ref<Period>('24h')
const timeseries = ref<TimeseriesPoint[]>([])
const byModel = ref<ModelBreakdown[]>([])
const byKey = ref<KeyBreakdown[]>([])
const isLoading = ref(false)
const granularity = ref('hour')

export function useUsageHistory() {
  const totalCost = computed(() =>
    timeseries.value.reduce((sum, p) => sum + p.costMicrodollars, 0),
  )

  const totalRequests = computed(() =>
    timeseries.value.reduce((sum, p) => sum + p.requestCount, 0),
  )

  const avgCostPerRequest = computed(() =>
    totalRequests.value > 0 ? totalCost.value / totalRequests.value : 0,
  )

  const totalInputTokens = computed(() =>
    timeseries.value.reduce((sum, p) => sum + p.inputTokens, 0),
  )

  const totalOutputTokens = computed(() =>
    timeseries.value.reduce((sum, p) => sum + p.outputTokens, 0),
  )

  const totalCacheReadTokens = computed(() =>
    timeseries.value.reduce((sum, p) => sum + p.cacheReadTokens, 0),
  )

  const totalCacheWriteTokens = computed(() =>
    timeseries.value.reduce((sum, p) => sum + p.cacheWriteTokens, 0),
  )

  const totalTokens = computed(
    () =>
      totalInputTokens.value +
      totalOutputTokens.value +
      totalCacheReadTokens.value +
      totalCacheWriteTokens.value,
  )

  async function fetchTimeseries() {
    try {
      const res = await fetch(`/admin/usage-history/timeseries?period=${period.value}`, {
        credentials: 'same-origin',
      })
      if (res.ok) {
        const data = await res.json()
        timeseries.value = data.points ?? []
        granularity.value = data.granularity ?? 'hour'
      }
    } catch (e) {
      console.error('Failed to fetch timeseries:', e)
    }
  }

  async function fetchByModel() {
    try {
      const res = await fetch(`/admin/usage-history/by-model?period=${period.value}`, {
        credentials: 'same-origin',
      })
      if (res.ok) {
        const data = await res.json()
        byModel.value = data.models ?? []
      }
    } catch (e) {
      console.error('Failed to fetch by-model:', e)
    }
  }

  async function fetchByKey() {
    try {
      const res = await fetch(`/admin/usage-history/by-key?period=${period.value}`, {
        credentials: 'same-origin',
      })
      if (res.ok) {
        const data = await res.json()
        byKey.value = data.keys ?? []
      }
    } catch (e) {
      console.error('Failed to fetch by-key:', e)
    }
  }

  async function fetchAll() {
    isLoading.value = true
    try {
      await Promise.all([fetchTimeseries(), fetchByModel(), fetchByKey()])
    } finally {
      isLoading.value = false
    }
  }

  async function clearHistory() {
    try {
      const res = await fetch('/admin/usage-history', {
        method: 'DELETE',
        credentials: 'same-origin',
      })
      if (res.ok) {
        await fetchAll()
      }
    } catch (e) {
      console.error('Failed to clear history:', e)
    }
  }

  async function setPeriod(p: Period) {
    period.value = p
    await fetchAll()
  }

  return {
    period,
    timeseries,
    byModel,
    byKey,
    isLoading,
    granularity,
    totalCost,
    totalRequests,
    avgCostPerRequest,
    totalInputTokens,
    totalOutputTokens,
    totalCacheReadTokens,
    totalCacheWriteTokens,
    totalTokens,
    fetchAll,
    clearHistory,
    setPeriod,
  }
}
