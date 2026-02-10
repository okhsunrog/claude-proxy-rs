<script setup lang="ts">
import { ref, computed } from 'vue'
import { errorMessage, formatCost } from '../utils/format'
import type { ClientKey, Model, UpdateLimitsRequest } from '../client'
import type { KeyUsageResponse, KeyModelsResponse, KeyModelUsageResponse } from '../composables/useKeys'
import UsageStats from './UsageStats.vue'
import TokenLimitsForm from './TokenLimitsForm.vue'
import KeyModelAccess from './KeyModelAccess.vue'
import ModelUsageTable from './ModelUsageTable.vue'

const props = defineProps<{
  keyData: ClientKey
  usage: KeyUsageResponse | undefined
  availableModels: Model[]
  deleteKey: (id: string) => Promise<void>
  updateLimits: (id: string, limits: UpdateLimitsRequest) => Promise<void>
  resetUsage: (id: string, type: 'fiveHour' | 'weekly' | 'total' | 'all') => Promise<void>
  loadKeyModels: (id: string) => Promise<KeyModelsResponse>
  setKeyModels: (id: string, models: string[]) => Promise<void>
  loadKeyModelUsage: (id: string) => Promise<KeyModelUsageResponse>
  setModelLimits: (keyId: string, model: string, limits: UpdateLimitsRequest) => Promise<void>
  removeModelLimits: (keyId: string, model: string) => Promise<void>
  resetModelUsage: (keyId: string, model: string, type: 'fiveHour' | 'weekly' | 'total' | 'all') => Promise<void>
}>()

const emit = defineEmits<{
  deleted: []
}>()

const toast = useToast()
const isExpanded = ref(false)
const showDeleteModal = ref(false)
const isDeleting = ref(false)

function formatDate(timestamp: number): string {
  return new Date(timestamp).toLocaleString()
}

function maskedKey(key: string): string {
  if (key.length <= 12) return key
  return key.slice(0, 10) + '...' + key.slice(-4)
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
    toast.add({ title: 'Failed to delete key', description: errorMessage(e), color: 'error' })
  } finally {
    isDeleting.value = false
  }
}

function percentColor(used: number, limit: number | null | undefined): string {
  if (!limit) return ''
  const pct = (used / limit) * 100
  if (pct >= 90) return 'text-red-500'
  if (pct >= 70) return 'text-yellow-500'
  return 'text-green-500'
}

interface UsageSummaryItem {
  label: string
  used: number
  limit: number | null | undefined
}

const usageSummary = computed<UsageSummaryItem[]>(() => {
  if (!props.usage) return []
  const { usage, limits } = props.usage
  return [
    { label: '5H', used: usage.fiveHourTokens ?? 0, limit: limits.fiveHourLimit },
    { label: 'W', used: usage.weeklyTokens ?? 0, limit: limits.weeklyLimit },
    { label: 'T', used: usage.totalTokens ?? 0, limit: limits.totalLimit },
  ]
})
</script>

<template>
  <div class="rounded-lg border border-default">
    <!-- Collapsed summary row -->
    <div
      class="flex items-center gap-3 px-4 py-3 cursor-pointer select-none hover:bg-elevated/50 transition-colors"
      @click="isExpanded = !isExpanded"
    >
      <UIcon
        :name="isExpanded ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
        class="w-4 h-4 shrink-0 text-muted"
      />
      <span class="font-semibold truncate">{{ keyData.name }}</span>
      <span class="font-mono text-xs text-muted hidden sm:inline">{{ maskedKey(keyData.key) }}</span>

      <!-- Inline usage summary -->
      <div v-if="usageSummary.length" class="flex items-center gap-3 ml-auto mr-2 text-xs text-muted whitespace-nowrap">
        <span v-for="item in usageSummary" :key="item.label">
          {{ item.label }}: <span class="font-mono">{{ formatCost(item.used) }}</span><template v-if="item.limit">/{{ formatCost(item.limit) }}
            <span :class="percentColor(item.used, item.limit)" class="font-semibold">{{ ((item.used / item.limit) * 100).toFixed(0) }}%</span></template>
        </span>
      </div>
      <div v-else class="ml-auto" />

      <!-- Action buttons (stop propagation to prevent toggle) -->
      <div class="flex gap-1.5 shrink-0" @click.stop>
        <UButton size="xs" variant="soft" @click="copyKey">Copy Key</UButton>
        <UButton size="xs" color="error" variant="soft" @click="showDeleteModal = true">Delete</UButton>
      </div>
    </div>

    <!-- Expanded details -->
    <div v-if="isExpanded" class="px-4 pb-4 border-t border-default">
      <div class="font-mono text-xs text-muted mt-3 mb-2 break-all">{{ keyData.key }}</div>

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

      <KeyModelAccess
        :key-id="keyData.id"
        :available-models="availableModels"
        :load-key-models="loadKeyModels"
        :set-key-models="setKeyModels"
      />

      <ModelUsageTable
        :key-id="keyData.id"
        :available-models="availableModels"
        :load-key-model-usage="loadKeyModelUsage"
        :set-model-limits="setModelLimits"
        :remove-model-limits="removeModelLimits"
        :reset-model-usage="resetModelUsage"
      />
    </div>

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
