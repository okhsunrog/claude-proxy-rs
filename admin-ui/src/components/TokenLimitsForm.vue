<script setup lang="ts">
import { ref, watch } from 'vue'
import { errorMessage } from '../utils/format'
import type { UpdateLimitsRequest } from '../client'
import type { KeyUsageResponse } from '../composables/useKeys'

const props = defineProps<{
  keyId: string
  allowExtraUsage: boolean
  setAllowExtraUsage: (id: string, allow: boolean) => Promise<void>
  usage: KeyUsageResponse | undefined
  updateLimits: (id: string, limits: UpdateLimitsRequest) => Promise<void>
  resetUsage: (id: string, type: 'fiveHour' | 'weekly' | 'total' | 'all') => Promise<void>
}>()

import { microToDollars, dollarsToMicro } from '../utils/format'

const toast = useToast()
// Display/edit in dollars, store in microdollars
const fiveHourDollars = ref<number | null>(null)
const weeklyDollars = ref<number | null>(null)
const totalDollars = ref<number | null>(null)
const isSaving = ref(false)

watch(
  () => props.usage,
  (u) => {
    if (u) {
      fiveHourDollars.value = microToDollars(u.limits.fiveHourLimit)
      weeklyDollars.value = microToDollars(u.limits.weeklyLimit)
      totalDollars.value = microToDollars(u.limits.totalLimit)
    }
  },
  { immediate: true },
)

async function handleSave() {
  isSaving.value = true
  try {
    await props.updateLimits(props.keyId, {
      fiveHourLimit: dollarsToMicro(fiveHourDollars.value),
      weeklyLimit: dollarsToMicro(weeklyDollars.value),
      totalLimit: dollarsToMicro(totalDollars.value),
    })
    toast.add({ title: 'Limits saved', color: 'success' })
  } catch (e: unknown) {
    toast.add({ title: 'Failed to save limits', description: errorMessage(e), color: 'error' })
  } finally {
    isSaving.value = false
  }
}

const resetItems = [
  [
    {
      label: 'Reset 5-Hour',
      onSelect: () => handleReset('fiveHour'),
    },
    {
      label: 'Reset Weekly',
      onSelect: () => handleReset('weekly'),
    },
    {
      label: 'Reset Total',
      onSelect: () => handleReset('total'),
    },
    {
      label: 'Reset All',
      onSelect: () => handleReset('all'),
    },
  ],
]

async function handleReset(type: 'fiveHour' | 'weekly' | 'total' | 'all') {
  try {
    await props.resetUsage(props.keyId, type)
    toast.add({ title: `${type.charAt(0).toUpperCase() + type.slice(1)} usage reset`, color: 'success' })
  } catch (e: unknown) {
    toast.add({ title: 'Failed to reset usage', description: errorMessage(e), color: 'error' })
  }
}

async function handleToggleExtraUsage(value: boolean) {
  try {
    await props.setAllowExtraUsage(props.keyId, value)
  } catch (e: unknown) {
    toast.add({ title: 'Failed to update setting', description: errorMessage(e), color: 'error' })
  }
}
</script>

<template>
  <div class="border-t border-default pt-3 mt-3">
    <h3 class="text-sm font-semibold text-muted mb-2">Cost Limits (USD)</h3>
    <div class="flex flex-wrap gap-3 items-end">
      <UFormField label="5-Hour ($)" class="w-32">
        <UInput
          :model-value="fiveHourDollars ?? undefined"
          @update:model-value="(v: string | number) => fiveHourDollars = v == null || v === '' ? null : Number(v)"
          type="number"
          placeholder="Unlimited"
          step="0.01"
          :min="0"
          class="no-spinners"
        />
      </UFormField>
      <UFormField label="Weekly ($)" class="w-32">
        <UInput
          :model-value="weeklyDollars ?? undefined"
          @update:model-value="(v: string | number) => weeklyDollars = v == null || v === '' ? null : Number(v)"
          type="number"
          placeholder="Unlimited"
          step="0.01"
          :min="0"
          class="no-spinners"
        />
      </UFormField>
      <UFormField label="Total ($)" class="w-32">
        <UInput
          :model-value="totalDollars ?? undefined"
          @update:model-value="(v: string | number) => totalDollars = v == null || v === '' ? null : Number(v)"
          type="number"
          placeholder="Unlimited"
          step="0.01"
          :min="0"
          class="no-spinners"
        />
      </UFormField>
    </div>
    <div class="flex gap-2 mt-3">
      <UButton size="sm" color="primary" :loading="isSaving" @click="handleSave">
        Save Limits
      </UButton>
      <UDropdownMenu :items="resetItems">
        <UButton size="sm" variant="soft">Reset Usage</UButton>
      </UDropdownMenu>
    </div>
    <div class="flex items-center gap-2 mt-3">
      <USwitch
        :model-value="allowExtraUsage"
        @update:model-value="handleToggleExtraUsage"
      />
      <span class="text-sm">Allow extra usage</span>
      <span class="text-xs text-muted">— use paid credits when subscription limits are full</span>
    </div>
  </div>
</template>
