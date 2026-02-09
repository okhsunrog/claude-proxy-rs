<script setup lang="ts">
import { ref } from 'vue'
import type { ClientKey, UpdateLimitsRequest } from '../client'
import type { KeyUsageResponse } from '../composables/useKeys'
import UsageStats from './UsageStats.vue'
import TokenLimitsForm from './TokenLimitsForm.vue'

const props = defineProps<{
  keyData: ClientKey
  usage: KeyUsageResponse | undefined
  deleteKey: (id: string) => Promise<void>
  updateLimits: (id: string, limits: UpdateLimitsRequest) => Promise<void>
  resetUsage: (id: string, type: 'hourly' | 'weekly' | 'total' | 'all') => Promise<void>
}>()

const emit = defineEmits<{
  deleted: []
}>()

const toast = useToast()
const showDeleteModal = ref(false)
const isDeleting = ref(false)

function formatDate(timestamp: number): string {
  return new Date(timestamp).toLocaleDateString()
}

function copyKey() {
  navigator.clipboard.writeText(props.keyData.key).then(
    () => toast.add({ title: 'Key copied to clipboard', color: 'success' }),
    () => toast.add({ title: 'Failed to copy key. Please copy manually.', color: 'error' }),
  )
}

async function handleDelete() {
  isDeleting.value = true
  try {
    await props.deleteKey(props.keyData.id)
    showDeleteModal.value = false
    toast.add({ title: 'Key deleted', color: 'success' })
    emit('deleted')
  } catch (e: unknown) {
    toast.add({ title: 'Failed to delete key', description: (e as Error).message, color: 'error' })
  } finally {
    isDeleting.value = false
  }
}
</script>

<template>
  <div class="rounded-lg border border-default p-4">
    <div class="flex justify-between items-center mb-2">
      <span class="font-semibold">{{ keyData.name }}</span>
      <div class="flex gap-2">
        <UButton size="xs" variant="soft" @click="copyKey">Copy Key</UButton>
        <UButton size="xs" color="error" variant="soft" @click="showDeleteModal = true">Delete</UButton>
      </div>
    </div>

    <div class="font-mono text-xs text-muted mb-2 break-all">{{ keyData.key }}</div>

    <div class="text-xs text-muted mb-3">
      Created: {{ formatDate(keyData.createdAt) }} |
      Last used: {{ keyData.lastUsedAt ? formatDate(keyData.lastUsedAt) : 'Never' }}
    </div>

    <UsageStats :usage="usage" />

    <TokenLimitsForm
      :key-id="keyData.id"
      :usage="usage"
      :update-limits="updateLimits"
      :reset-usage="resetUsage"
    />

    <UModal v-model:open="showDeleteModal" title="Confirm Delete" :ui="{ width: 'max-w-md' }">
      <template #body>
        <p>Are you sure you want to delete key "<strong>{{ keyData.name }}</strong>"?</p>
      </template>
      <template #footer>
        <div class="flex justify-end gap-2">
          <UButton variant="ghost" @click="showDeleteModal = false">Cancel</UButton>
          <UButton color="error" :loading="isDeleting" @click="handleDelete">Delete</UButton>
        </div>
      </template>
    </UModal>
  </div>
</template>
