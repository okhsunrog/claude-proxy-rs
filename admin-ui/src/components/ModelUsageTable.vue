<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { errorMessage, formatCost, microToDollars, dollarsToMicro } from '../utils/format'
import type { Model, ModelUsageEntry, UpdateLimitsRequest } from '../client'
import type { KeyModelUsageResponse } from '../composables/useKeys'

const props = defineProps<{
  keyId: string
  availableModels: Model[]
  loadKeyModelUsage: (id: string) => Promise<KeyModelUsageResponse>
  setModelLimits: (keyId: string, model: string, limits: UpdateLimitsRequest) => Promise<void>
  removeModelLimits: (keyId: string, model: string) => Promise<void>
  resetModelUsage: (
    keyId: string,
    model: string,
    type: 'fiveHour' | 'weekly' | 'total' | 'all',
  ) => Promise<void>
}>()

// Build a lookup map from model id to pricing
const priceMap = computed(() => {
  const map: Record<string, Model> = {}
  for (const m of props.availableModels) {
    map[m.id] = m
  }
  return map
})

const toast = useToast()
const entries = ref<ModelUsageEntry[]>([])
const isLoading = ref(true)
const isExpanded = ref(false)

// Editing state
const editingModel = ref<string | null>(null)
const editFiveHour = ref<number | null>(null)
const editWeekly = ref<number | null>(null)
const editTotal = ref<number | null>(null)
const isSaving = ref(false)

onMounted(async () => {
  await loadData()
})

defineExpose({ loadData })

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

// Calculate cost in dollars for tokens at a given $/MTok price
function tokenCost(tokens: number, pricePerMTok: number): number {
  return (tokens * pricePerMTok) / 1_000_000
}

function formatDollars(dollars: number): string {
  if (dollars === 0) return ''
  if (dollars >= 100) return `$${dollars.toFixed(0)}`
  if (dollars >= 1) return `$${dollars.toFixed(2)}`
  if (dollars >= 0.01) return `$${dollars.toFixed(3)}`
  return `$${dollars.toFixed(4)}`
}

// Total cost for a window period
function windowCost(
  entry: ModelUsageEntry,
  window: 'fiveHour' | 'weekly' | 'total',
): string {
  const model = priceMap.value[entry.model]
  if (!model) return ''
  const w = entry[window]
  const total =
    tokenCost(w.input, model.inputPrice) +
    tokenCost(w.output, model.outputPrice) +
    tokenCost(w.cacheRead, model.cacheReadPrice) +
    tokenCost(w.cacheWrite, model.cacheWritePrice)
  return formatDollars(total)
}

// Cost string for a single token type
function lineCost(tokens: number, pricePerMTok: number): string {
  if (tokens === 0) return ''
  return formatDollars(tokenCost(tokens, pricePerMTok))
}

function startEdit(entry: ModelUsageEntry) {
  editingModel.value = entry.model
  editFiveHour.value = microToDollars(entry.limits.fiveHourLimit)
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
      editFiveHour.value != null || editWeekly.value != null || editTotal.value != null
    if (hasAnyLimit) {
      await props.setModelLimits(props.keyId, model, {
        fiveHourLimit: dollarsToMicro(editFiveHour.value),
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
      description: errorMessage(e),
      color: 'error',
    })
  } finally {
    isSaving.value = false
  }
}

async function handleReset(model: string, type: 'fiveHour' | 'weekly' | 'total' | 'all') {
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
      description: errorMessage(e),
      color: 'error',
    })
  }
}

function resetItems(model: string) {
  return [
    [
      { label: 'Reset 5-Hour', onSelect: () => handleReset(model, 'fiveHour') },
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
          <div v-for="window in ['fiveHour', 'weekly', 'total'] as const" :key="window" class="rounded bg-elevated p-2">
            <div class="flex items-baseline justify-between mb-1">
              <span class="text-muted uppercase">{{ window === 'fiveHour' ? '5-HOUR' : window }}</span>
              <span v-if="windowCost(entry, window)" class="font-mono font-semibold text-primary">{{ windowCost(entry, window) }}</span>
            </div>
            <div class="space-y-0.5">
              <div>In: <span class="font-mono">{{ formatTokens(entry[window].input) }}</span><span v-if="lineCost(entry[window].input, priceMap[entry.model]?.inputPrice ?? 0)" class="text-muted ml-1">({{ lineCost(entry[window].input, priceMap[entry.model]?.inputPrice ?? 0) }})</span></div>
              <div>Out: <span class="font-mono">{{ formatTokens(entry[window].output) }}</span><span v-if="lineCost(entry[window].output, priceMap[entry.model]?.outputPrice ?? 0)" class="text-muted ml-1">({{ lineCost(entry[window].output, priceMap[entry.model]?.outputPrice ?? 0) }})</span></div>
              <div>Cache R: <span class="font-mono">{{ formatTokens(entry[window].cacheRead) }}</span><span v-if="lineCost(entry[window].cacheRead, priceMap[entry.model]?.cacheReadPrice ?? 0)" class="text-muted ml-1">({{ lineCost(entry[window].cacheRead, priceMap[entry.model]?.cacheReadPrice ?? 0) }})</span></div>
              <div>Cache W: <span class="font-mono">{{ formatTokens(entry[window].cacheWrite) }}</span><span v-if="lineCost(entry[window].cacheWrite, priceMap[entry.model]?.cacheWritePrice ?? 0)" class="text-muted ml-1">({{ lineCost(entry[window].cacheWrite, priceMap[entry.model]?.cacheWritePrice ?? 0) }})</span></div>
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
            <UFormField label="5-Hour ($)" class="w-28">
              <UInput
                :model-value="editFiveHour ?? undefined"
                @update:model-value="(v: string | number) => editFiveHour = v == null || v === '' ? null : Number(v)"
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
