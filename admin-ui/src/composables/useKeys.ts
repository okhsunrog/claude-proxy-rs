import { ref } from 'vue'
import type { ClientKey, KeyUsageResponse, UpdateLimitsRequest } from '../client'
import {
  listKeys as listKeysApi,
  createKey as createKeyApi,
  deleteKey as deleteKeyApi,
  getKeyUsage,
  updateKeyLimits,
  resetKeyUsage as resetKeyUsageApi,
} from '../client'

export type { KeyUsageResponse }

export function useKeys() {
  const keys = ref<ClientKey[]>([])
  const isLoading = ref(false)
  const newKeyData = ref<{ key: string; id: string } | null>(null)
  const usageMap = ref<Record<string, KeyUsageResponse>>({})

  async function loadKeys() {
    isLoading.value = true
    try {
      const { data } = await listKeysApi()
      keys.value = data?.keys ?? []
      // Load usage for all keys in parallel
      await Promise.all(keys.value.map((key) => loadUsage(key.id)))
    } catch (e) {
      console.error('Failed to load keys:', e)
    } finally {
      isLoading.value = false
    }
  }

  async function loadUsage(id: string) {
    try {
      const { data } = await getKeyUsage({ path: { id } })
      if (data) {
        usageMap.value[id] = data
      }
    } catch (e) {
      console.error('Failed to load usage for key:', id, e)
    }
  }

  async function createKey(name: string): Promise<boolean> {
    const { data, error } = await createKeyApi({ body: { name } })
    if (error) {
      throw error
    }
    newKeyData.value = data ?? null
    await loadKeys()
    return true
  }

  async function deleteKey(id: string): Promise<void> {
    await deleteKeyApi({ path: { id } })
    await loadKeys()
  }

  async function updateLimits(id: string, limits: UpdateLimitsRequest): Promise<void> {
    await updateKeyLimits({ path: { id }, body: limits })
    await loadUsage(id)
  }

  async function resetUsage(
    id: string,
    type: 'hourly' | 'weekly' | 'total' | 'all',
  ): Promise<void> {
    await resetKeyUsageApi({ path: { id }, body: { type } })
    await loadUsage(id)
  }

  return {
    keys,
    isLoading,
    newKeyData,
    usageMap,
    loadKeys,
    loadUsage,
    createKey,
    deleteKey,
    updateLimits,
    resetUsage,
  }
}
