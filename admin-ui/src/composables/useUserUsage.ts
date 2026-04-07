import { ref, computed } from 'vue'
import { getMyUsage, getMyTimeseries, getMyByModel } from '../client'
import type { UserUsageResponse, TimeseriesResponse, ModelBreakdownResponse } from '../client'
import { createClient, createConfig } from '../client/client'

export type { UserUsageResponse, TimeseriesResponse, ModelBreakdownResponse }
export type Period = '24h' | '7d' | '30d'

// A separate client instance that injects the user's Bearer token
function makeUserClient(token: string) {
  return createClient(
    createConfig({
      baseUrl: '/admin',
      headers: { Authorization: `Bearer ${token}` },
    }),
  )
}

const TOKEN_KEY = 'user_api_token'

export function useUserUsage() {
  const token = ref<string>(localStorage.getItem(TOKEN_KEY) ?? '')
  const isAuthenticated = computed(() => token.value.length > 0)

  const usage = ref<UserUsageResponse | null>(null)
  const timeseries = ref<TimeseriesResponse | null>(null)
  const byModel = ref<ModelBreakdownResponse | null>(null)
  const period = ref<Period>('24h')
  const isLoading = ref(false)
  const error = ref<string | null>(null)

  function saveToken(t: string) {
    token.value = t.trim()
    if (token.value) {
      localStorage.setItem(TOKEN_KEY, token.value)
    } else {
      localStorage.removeItem(TOKEN_KEY)
    }
  }

  function clearToken() {
    saveToken('')
    usage.value = null
    timeseries.value = null
    byModel.value = null
    error.value = null
  }

  async function fetchAll() {
    if (!token.value) return
    isLoading.value = true
    error.value = null
    const userClient = makeUserClient(token.value)
    try {
      const [usageRes, tsRes, modelRes] = await Promise.all([
        getMyUsage({ client: userClient }),
        getMyTimeseries({ client: userClient, query: { period: period.value } }),
        getMyByModel({ client: userClient, query: { period: period.value } }),
      ])

      if (usageRes.error)
        throw new Error((usageRes.error as { error?: string }).error ?? 'Unauthorized')
      if (tsRes.error)
        throw new Error((tsRes.error as { error?: string }).error ?? 'Failed to fetch history')
      if (modelRes.error)
        throw new Error(
          (modelRes.error as { error?: string }).error ?? 'Failed to fetch model breakdown',
        )

      usage.value = usageRes.data ?? null
      timeseries.value = tsRes.data ?? null
      byModel.value = modelRes.data ?? null
    } catch (e: unknown) {
      error.value = e instanceof Error ? e.message : 'Unknown error'
      if (error.value === 'Unauthorized') clearToken()
    } finally {
      isLoading.value = false
    }
  }

  async function setPeriod(p: Period) {
    period.value = p
    await fetchAll()
  }

  return {
    token,
    isAuthenticated,
    usage,
    timeseries,
    byModel,
    period,
    isLoading,
    error,
    saveToken,
    clearToken,
    fetchAll,
    setPeriod,
  }
}
