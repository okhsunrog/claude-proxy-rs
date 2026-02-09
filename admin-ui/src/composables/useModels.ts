import { ref } from 'vue'
import type { Model, AddModelRequest, UpdateModelRequest } from '../client'
import {
  listModelsAdmin,
  addModel as addModelApi,
  deleteModel as deleteModelApi,
  updateModel as updateModelApi,
  reorderModels as reorderModelsApi,
} from '../client'

// Module-scope state: shared across all useModels() callers
const models = ref<Model[]>([])
const isLoading = ref(false)

export function useModels() {
  async function loadModels() {
    isLoading.value = true
    try {
      const { data } = await listModelsAdmin()
      models.value = data?.models ?? []
    } catch (e) {
      console.error('Failed to load models:', e)
    } finally {
      isLoading.value = false
    }
  }

  async function addModel(request: AddModelRequest): Promise<void> {
    await addModelApi({ body: request })
    await loadModels()
  }

  async function deleteModel(id: string): Promise<void> {
    await deleteModelApi({ path: { id } })
    await loadModels()
  }

  async function updateModel(id: string, request: UpdateModelRequest): Promise<void> {
    await updateModelApi({ path: { id }, body: request })
    await loadModels()
  }

  async function reorderModels(ids: string[]): Promise<void> {
    await reorderModelsApi({ body: { ids } })
    await loadModels()
  }

  return {
    models,
    isLoading,
    loadModels,
    addModel,
    deleteModel,
    updateModel,
    reorderModels,
  }
}
