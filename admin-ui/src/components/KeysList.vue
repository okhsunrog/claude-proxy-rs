<script setup lang="ts">
import { onMounted } from 'vue'
import { useKeys } from '../composables/useKeys'
import CreateKeyForm from './CreateKeyForm.vue'
import KeyCard from './KeyCard.vue'

const { keys, isLoading, newKeyData, usageMap, loadKeys, createKey, deleteKey, updateLimits, resetUsage } = useKeys()

onMounted(() => {
  loadKeys()
})
</script>

<template>
  <UCard>
    <template #header>
      <h2 class="text-xl font-semibold">API Keys</h2>
    </template>

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
          :delete-key="deleteKey"
          :update-limits="updateLimits"
          :reset-usage="resetUsage"
        />
      </div>
    </div>
  </UCard>
</template>
