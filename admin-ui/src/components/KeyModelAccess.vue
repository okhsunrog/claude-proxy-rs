<script setup lang="ts">
import { ref, onMounted } from 'vue'
import type { Model } from '../client'

const props = defineProps<{
  keyId: string
  availableModels: Model[]
  loadKeyModels: (id: string) => Promise<{ allowAll: boolean; models: string[] }>
  setKeyModels: (id: string, models: string[]) => Promise<void>
}>()

const toast = useToast()
const allowAll = ref(true)
const selectedModels = ref<string[]>([])
const isLoading = ref(true)
const isSaving = ref(false)

onMounted(async () => {
  await loadAccess()
})

async function loadAccess() {
  isLoading.value = true
  try {
    const result = await props.loadKeyModels(props.keyId)
    allowAll.value = result.allowAll
    selectedModels.value = result.models
  } catch (e) {
    console.error('Failed to load model access:', e)
  } finally {
    isLoading.value = false
  }
}

async function handleToggleAllowAll() {
  const newValue = !allowAll.value
  isSaving.value = true
  try {
    if (newValue) {
      // Switch to "allow all" — send empty list
      await props.setKeyModels(props.keyId, [])
      allowAll.value = true
      selectedModels.value = []
    } else {
      // Switch to whitelist — start with all enabled models selected
      const allIds = props.availableModels.filter((m) => m.enabled).map((m) => m.id)
      await props.setKeyModels(props.keyId, allIds)
      allowAll.value = false
      selectedModels.value = allIds
    }
  } catch (e: unknown) {
    toast.add({
      title: 'Failed to update model access',
      description: (e as Error).message,
      color: 'error',
    })
  } finally {
    isSaving.value = false
  }
}

function isModelSelected(modelId: string): boolean {
  return selectedModels.value.includes(modelId)
}

async function toggleModel(modelId: string) {
  const newList = isModelSelected(modelId)
    ? selectedModels.value.filter((m) => m !== modelId)
    : [...selectedModels.value, modelId]

  isSaving.value = true
  try {
    await props.setKeyModels(props.keyId, newList)
    selectedModels.value = newList
    // If list becomes empty, switch to allow all
    if (newList.length === 0) {
      allowAll.value = true
    }
  } catch (e: unknown) {
    toast.add({
      title: 'Failed to update model access',
      description: (e as Error).message,
      color: 'error',
    })
  } finally {
    isSaving.value = false
  }
}
</script>

<template>
  <div class="border-t border-default pt-3 mt-3">
    <div class="flex items-center justify-between mb-2">
      <h3 class="text-sm font-semibold text-muted">Model Access</h3>
      <div class="flex items-center gap-2">
        <span class="text-xs text-muted">Allow all models</span>
        <USwitch
          :model-value="allowAll"
          @update:model-value="handleToggleAllowAll"
          :loading="isSaving"
          size="sm"
        />
      </div>
    </div>

    <div v-if="isLoading" class="text-xs text-muted">Loading...</div>

    <div v-else-if="!allowAll" class="flex flex-wrap gap-1.5 mt-2">
      <UButton
        v-for="model in availableModels.filter(m => m.enabled)"
        :key="model.id"
        size="xs"
        :variant="isModelSelected(model.id) ? 'solid' : 'outline'"
        :color="isModelSelected(model.id) ? 'primary' : 'neutral'"
        @click="toggleModel(model.id)"
      >
        {{ model.id }}
      </UButton>
    </div>
  </div>
</template>
