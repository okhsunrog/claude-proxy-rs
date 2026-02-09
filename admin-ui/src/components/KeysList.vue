<script setup lang="ts">
import { onMounted } from 'vue'
import { useKeys } from '../composables/useKeys'
import { useModels } from '../composables/useModels'
import CreateKeyForm from './CreateKeyForm.vue'
import KeyCard from './KeyCard.vue'

const {
  keys,
  isLoading,
  newKeyData,
  usageMap,
  loadKeys,
  createKey,
  deleteKey,
  updateLimits,
  resetUsage,
  loadKeyModels,
  setKeyModels,
  loadKeyModelUsage,
  setModelLimits,
  removeModelLimits,
  resetModelUsage,
} = useKeys()

const { models, loadModels } = useModels()

onMounted(() => {
  loadKeys()
  loadModels()
})
</script>

<template>
  <div class="space-y-4">
    <CreateKeyForm
      :create-key="createKey"
      :new-key-data="newKeyData"
    />

    <div v-if="isLoading && keys.length === 0" class="text-center py-8 text-muted">
      Loading...
    </div>

    <div v-else-if="keys.length === 0" class="text-center py-8 text-muted">
      No API keys yet
    </div>

    <div v-else class="space-y-3">
      <KeyCard
        v-for="key in keys"
        :key="key.id"
        :key-data="key"
        :usage="usageMap[key.id]"
        :available-models="models"
        :delete-key="deleteKey"
        :update-limits="updateLimits"
        :reset-usage="resetUsage"
        :load-key-models="loadKeyModels"
        :set-key-models="setKeyModels"
        :load-key-model-usage="loadKeyModelUsage"
        :set-model-limits="setModelLimits"
        :remove-model-limits="removeModelLimits"
        :reset-model-usage="resetModelUsage"
      />
    </div>
  </div>
</template>
