<script setup lang="ts">
import { ref, onMounted } from 'vue'
import type { ModelUsageEntry, UpdateLimitsRequest } from '../client'
import type { KeyModelUsageResponse } from '../composables/useKeys'

const props = defineProps<{
  keyId: string
  loadKeyModelUsage: (id: string) => Promise<KeyModelUsageResponse>
  setModelLimits: (keyId: string, model: string, limits: UpdateLimitsRequest) => Promise<void>
  removeModelLimits: (keyId: string, model: string) => Promise<void>
  resetModelUsage: (
    keyId: string,
    model: string,
    type: 'hourly' | 'weekly' | 'total' | 'all',
  ) => Promise<void>
}>()

const toast = useToast()
const entries = ref<ModelUsageEntry[]>([])
const isLoading = ref(true)
const isExpanded = ref(false)

// Editing state
const editingModel = ref<string | null>(null)
const editHourly = ref<number | null>(null)
const editWeekly = ref<number | null>(null)
const editTotal = ref<number | null>(null)
const isSaving = ref(false)

onMounted(async () => {
  await loadData()
})

async function loadData() {
  isLoading.value = true
  try {
    const result = await props.loadKeyModelUsage(props.keyId)
    entries.value = result.entries
  } catch (e) {
    console.error('Failed to load model usage:', e)
  } finally {
    isLoading.value = false
  }
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`
  return n.toString()
}

function formatCost(microdollars: number): string {
  const dollars = microdollars / 1_000_000
  if (dollars >= 100) return `$${dollars.toFixed(0)}`
  if (dollars >= 1) return `$${dollars.toFixed(2)}`
  if (dollars >= 0.01) return `$${dollars.toFixed(3)}`
  if (microdollars === 0) return '$0'
  return `$${dollars.toFixed(4)}`
}

function microToDollars(micro: number | null | undefined): number | null {
  if (micro == null) return null
  return micro / 1_000_000
}

function dollarsToMicro(dollars: number | null): number | null {
  if (dollars == null) return null
  return Math.round(dollars * 1_000_000)
}

function startEdit(entry: ModelUsageEntry) {
  editingModel.value = entry.model
  editHourly.value = microToDollars(entry.limits.hourlyLimit)
  editWeekly.value = microToDollars(entry.limits.weeklyLimit)
  editTotal.value = microToDollars(entry.limits.totalLimit)
}

function cancelEdit() {
  editingModel.value = null
}

async function saveEdit(model: string) {
  isSaving.value = true
  try {
    const hasAnyLimit =
      editHourly.value != null || editWeekly.value != null || editTotal.value != null
    if (hasAnyLimit) {
      await props.setModelLimits(props.keyId, model, {
        hourlyLimit: dollarsToMicro(editHourly.value),
        weeklyLimit: dollarsToMicro(editWeekly.value),
        totalLimit: dollarsToMicro(editTotal.value),
      })
    } else {
      await props.removeModelLimits(props.keyId, model)
    }
    editingModel.value = null
    await loadData()
    toast.add({ title: 'Model limits updated', color: 'success' })
  } catch (e: unknown) {
    toast.add({
      title: 'Failed to update limits',
      description: (e as Error).message,
      color: 'error',
    })
  } finally {
    isSaving.value = false
  }
}

async function handleReset(model: string, type: 'hourly' | 'weekly' | 'total' | 'all') {
  try {
    await props.resetModelUsage(props.keyId, model, type)
    await loadData()
    toast.add({
      title: `${type.charAt(0).toUpperCase() + type.slice(1)} usage reset for ${model}`,
      color: 'success',
    })
  } catch (e: unknown) {
    toast.add({
      title: 'Failed to reset usage',
      description: (e as Error).message,
      color: 'error',
    })
  }
}

function resetItems(model: string) {
  return [
    [
      { label: 'Reset Hourly', onSelect: () => handleReset(model, 'hourly') },
      { label: 'Reset Weekly', onSelect: () => handleReset(model, 'weekly') },
      { label: 'Reset Total', onSelect: () => handleReset(model, 'total') },
      { label: 'Reset All', onSelect: () => handleReset(model, 'all') },
    ],
  ]
}
</script>

<template>
  <div class="border-t border-default pt-3 mt-3">
    <button
      class="flex items-center gap-1 text-sm font-semibold text-muted hover:text-default cursor-pointer w-full text-left"
      @click="isExpanded = !isExpanded; if (isExpanded && entries.length === 0) loadData()"
    >
      <UIcon :name="isExpanded ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'" class="w-4 h-4" />
      Per-Model Usage
      <span v-if="entries.length > 0" class="text-xs font-normal ml-1">({{ entries.length }} models)</span>
    </button>

    <div v-if="isExpanded" class="mt-3 space-y-3">
      <div v-if="isLoading" class="text-xs text-muted">Loading...</div>

      <div v-else-if="entries.length === 0" class="text-xs text-muted">
        No per-model usage recorded yet
      </div>

      <div
        v-else
        v-for="entry in entries"
        :key="entry.model"
        class="rounded-lg border border-default p-3"
      >
        <div class="flex items-center justify-between mb-2">
          <span class="font-mono text-sm font-semibold">{{ entry.model }}</span>
          <div class="flex gap-1.5">
            <UButton
              v-if="editingModel !== entry.model"
              size="xs"
              variant="ghost"
              icon="i-lucide-pencil"
              @click="startEdit(entry)"
            />
            <UDropdownMenu :items="resetItems(entry.model)">
              <UButton size="xs" variant="ghost" icon="i-lucide-rotate-ccw" />
            </UDropdownMenu>
          </div>
        </div>

        <!-- Token breakdown -->
        <div class="grid grid-cols-3 gap-2 text-xs">
          <div v-for="window in ['hourly', 'weekly', 'total'] as const" :key="window" class="rounded bg-elevated p-2">
            <div class="text-muted uppercase mb-1">{{ window }}</div>
            <div class="space-y-0.5">
              <div>In: <span class="font-mono">{{ formatTokens(entry[window].input) }}</span></div>
              <div>Out: <span class="font-mono">{{ formatTokens(entry[window].output) }}</span></div>
              <div>Cache R: <span class="font-mono">{{ formatTokens(entry[window].cacheRead) }}</span></div>
              <div>Cache W: <span class="font-mono">{{ formatTokens(entry[window].cacheWrite) }}</span></div>
            </div>
            <div v-if="entry.limits[`${window}Limit` as keyof typeof entry.limits]" class="mt-1 pt-1 border-t border-default">
              Limit: <span class="font-semibold">{{ formatCost(entry.limits[`${window}Limit` as keyof typeof entry.limits] as number) }}</span>
            </div>
          </div>
        </div>

        <!-- Edit limits inline -->
        <div v-if="editingModel === entry.model" class="mt-3 pt-3 border-t border-default">
          <div class="text-xs font-semibold text-muted mb-2">Cost Limits (USD)</div>
          <div class="flex flex-wrap gap-3 items-end">
            <UFormField label="Hourly ($)" class="w-28">
              <UInput
                :model-value="editHourly ?? undefined"
                @update:model-value="(v: string | number) => editHourly = v == null || v === '' ? null : Number(v)"
                type="number"
                placeholder="None"
                step="0.01"
                :min="0"
                size="xs"
                class="no-spinners"
              />
            </UFormField>
            <UFormField label="Weekly ($)" class="w-28">
              <UInput
                :model-value="editWeekly ?? undefined"
                @update:model-value="(v: string | number) => editWeekly = v == null || v === '' ? null : Number(v)"
                type="number"
                placeholder="None"
                step="0.01"
                :min="0"
                size="xs"
                class="no-spinners"
              />
            </UFormField>
            <UFormField label="Total ($)" class="w-28">
              <UInput
                :model-value="editTotal ?? undefined"
                @update:model-value="(v: string | number) => editTotal = v == null || v === '' ? null : Number(v)"
                type="number"
                placeholder="None"
                step="0.01"
                :min="0"
                size="xs"
                class="no-spinners"
              />
            </UFormField>
          </div>
          <div class="flex gap-2 mt-2">
            <UButton size="xs" color="primary" :loading="isSaving" @click="saveEdit(entry.model)">Save</UButton>
            <UButton size="xs" variant="ghost" @click="cancelEdit">Cancel</UButton>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.no-spinners :deep(input[type="number"]::-webkit-inner-spin-button),
.no-spinners :deep(input[type="number"]::-webkit-outer-spin-button) {
  -webkit-appearance: none;
  margin: 0;
}
.no-spinners :deep(input[type="number"]) {
  -moz-appearance: textfield;
}
</style>
