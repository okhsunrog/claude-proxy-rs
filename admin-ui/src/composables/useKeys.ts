import { ref } from 'vue'
import type {
  ClientKey,
  KeyUsageResponse,
  KeyModelsResponse,
  KeyModelUsageResponse,
  UpdateLimitsRequest,
} from '../client'
import {
  listKeys as listKeysApi,
  createKey as createKeyApi,
  deleteKey as deleteKeyApi,
  setKeyEnabled as setKeyEnabledApi,
  getKeyUsage,
  updateKeyLimits,
  resetKeyUsage as resetKeyUsageApi,
  getKeyModels,
  setKeyModels as setKeyModelsApi,
  getKeyModelUsage,
  setKeyModelLimits as setKeyModelLimitsApi,
  removeKeyModelLimits as removeKeyModelLimitsApi,
  resetKeyModelUsage as resetKeyModelUsageApi,
} from '../client'

export type { KeyUsageResponse, KeyModelsResponse, KeyModelUsageResponse }

export function useKeys() {
  const keys = ref<ClientKey[]>([])
  const isLoading = ref(false)
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
    const { error } = await createKeyApi({ body: { name } })
    if (error) {
      throw error
    }
    await loadKeys()
    return true
  }

  async function deleteKey(id: string): Promise<void> {
    await deleteKeyApi({ path: { id } })
    await loadKeys()
  }

  async function toggleKey(id: string, enabled: boolean): Promise<void> {
    await setKeyEnabledApi({ path: { id }, body: { enabled } })
    await loadKeys()
  }

  async function updateLimits(id: string, limits: UpdateLimitsRequest): Promise<void> {
    await updateKeyLimits({ path: { id }, body: limits })
    await loadUsage(id)
  }

  async function resetUsage(
    id: string,
    type: 'fiveHour' | 'weekly' | 'total' | 'all',
  ): Promise<void> {
    await resetKeyUsageApi({ path: { id }, body: { type } })
    await loadUsage(id)
  }

  // Per-key model access
  async function loadKeyModels(id: string): Promise<KeyModelsResponse> {
    const { data } = await getKeyModels({ path: { id } })
    return data ?? { allowAll: true, models: [] }
  }

  async function setKeyModels(id: string, models: string[]): Promise<void> {
    await setKeyModelsApi({ path: { id }, body: { models } })
  }

  // Per-key per-model usage
  async function loadKeyModelUsage(id: string): Promise<KeyModelUsageResponse> {
    const { data } = await getKeyModelUsage({ path: { id } })
    return data ?? { entries: [] }
  }

  async function setModelLimits(
    keyId: string,
    model: string,
    limits: UpdateLimitsRequest,
  ): Promise<void> {
    await setKeyModelLimitsApi({ path: { id: keyId, model }, body: limits })
  }

  async function removeModelLimits(keyId: string, model: string): Promise<void> {
    await removeKeyModelLimitsApi({ path: { id: keyId, model } })
  }

  async function resetModelUsage(
    keyId: string,
    model: string,
    type: 'fiveHour' | 'weekly' | 'total' | 'all',
  ): Promise<void> {
    await resetKeyModelUsageApi({ path: { id: keyId, model }, body: { type } })
  }

  return {
    keys,
    isLoading,
    usageMap,
    loadKeys,
    loadUsage,
    createKey,
    deleteKey,
    toggleKey,
    updateLimits,
    resetUsage,
    loadKeyModels,
    setKeyModels,
    loadKeyModelUsage,
    setModelLimits,
    removeModelLimits,
    resetModelUsage,
  }
}
