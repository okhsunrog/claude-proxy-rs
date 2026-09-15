import { ref, computed } from 'vue'
import type {
  TimeseriesPoint,
  ModelBreakdown,
  KeyBreakdown,
  KeyModelBreakdown,
  ClientKey,
} from '../client'
import {
  getUsageHistoryTimeseries,
  getUsageHistoryByModel,
  getUsageHistoryByKey,
  getUsageHistoryByKeyModel,
  deleteUsageHistory,
  listKeys,
} from '../client'

export type Period = '24h' | '7d' | '30d'
export type { TimeseriesPoint, ModelBreakdown, KeyBreakdown, KeyModelBreakdown }

const period = ref<Period>('24h')
/** `null` = all keys; otherwise every breakdown is restricted to this key. */
const selectedKeyId = ref<string | null>(null)
const timeseries = ref<TimeseriesPoint[]>([])
const byModel = ref<ModelBreakdown[]>([])
const byKey = ref<KeyBreakdown[]>([])
const byKeyModel = ref<KeyModelBreakdown[]>([])
/** All known keys, unfiltered — powers the key selector. */
const allKeys = ref<ClientKey[]>([])
const isLoading = ref(false)
const granularity = ref('hour')

export function useUsageHistory() {
  const totalCost = computed(() => timeseries.value.reduce((sum, p) => sum + p.costMicrodollars, 0))

  const totalRequests = computed(() => timeseries.value.reduce((sum, p) => sum + p.requestCount, 0))

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

  /** Query params shared by every history endpoint. */
  function historyQuery() {
    return selectedKeyId.value
      ? { period: period.value, keyId: selectedKeyId.value }
      : { period: period.value }
  }

  async function fetchTimeseries() {
    try {
      const { data } = await getUsageHistoryTimeseries({ query: historyQuery() })
      timeseries.value = data?.points ?? []
      granularity.value = data?.granularity ?? 'hour'
    } catch (e) {
      console.error('Failed to fetch timeseries:', e)
    }
  }

  async function fetchByModel() {
    try {
      const { data } = await getUsageHistoryByModel({ query: historyQuery() })
      byModel.value = data?.models ?? []
    } catch (e) {
      console.error('Failed to fetch by-model:', e)
    }
  }

  async function fetchByKey() {
    try {
      const { data } = await getUsageHistoryByKey({ query: historyQuery() })
      byKey.value = data?.keys ?? []
    } catch (e) {
      console.error('Failed to fetch by-key:', e)
    }
  }

  async function fetchByKeyModel() {
    try {
      const { data } = await getUsageHistoryByKeyModel({ query: historyQuery() })
      byKeyModel.value = data?.entries ?? []
    } catch (e) {
      console.error('Failed to fetch by-key-model:', e)
    }
  }

  /** Key list for the selector — independent of period/key filters. */
  async function fetchKeys() {
    try {
      const { data } = await listKeys()
      allKeys.value = data?.keys ?? []
    } catch (e) {
      console.error('Failed to fetch keys:', e)
    }
  }

  async function fetchAll() {
    isLoading.value = true
    try {
      await Promise.all([
        fetchTimeseries(),
        fetchByModel(),
        fetchByKey(),
        fetchByKeyModel(),
        fetchKeys(),
      ])
    } finally {
      isLoading.value = false
    }
  }

  async function clearHistory() {
    try {
      await deleteUsageHistory()
      await fetchAll()
    } catch (e) {
      console.error('Failed to clear history:', e)
    }
  }

  async function setPeriod(p: Period) {
    period.value = p
    await fetchAll()
  }

  async function setSelectedKey(keyId: string | null) {
    selectedKeyId.value = keyId
    await fetchAll()
  }

  const selectedKeyName = computed(
    () => allKeys.value.find((k) => k.id === selectedKeyId.value)?.name ?? null,
  )

  return {
    period,
    selectedKeyId,
    selectedKeyName,
    allKeys,
    timeseries,
    byModel,
    byKey,
    byKeyModel,
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
    setSelectedKey,
  }
}
