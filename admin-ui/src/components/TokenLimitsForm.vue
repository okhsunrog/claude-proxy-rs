<script setup lang="ts">
import { ref, watch } from 'vue'
import type { UpdateLimitsRequest } from '../client'
import type { KeyUsageResponse } from '../composables/useKeys'

const props = defineProps<{
  keyId: string
  usage: KeyUsageResponse | undefined
  updateLimits: (id: string, limits: UpdateLimitsRequest) => Promise<void>
  resetUsage: (id: string, type: 'hourly' | 'weekly' | 'total' | 'all') => Promise<void>
}>()

const toast = useToast()
// Display/edit in dollars, store in microdollars
const hourlyDollars = ref<number | null>(null)
const weeklyDollars = ref<number | null>(null)
const totalDollars = ref<number | null>(null)
const isSaving = ref(false)

function microToDollars(micro: number | null | undefined): number | null {
  if (micro == null) return null
  return micro / 1_000_000
}

function dollarsToMicro(dollars: number | null): number | null {
  if (dollars == null) return null
  return Math.round(dollars * 1_000_000)
}

watch(
  () => props.usage,
  (u) => {
    if (u) {
      hourlyDollars.value = microToDollars(u.limits.hourlyLimit)
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
      hourlyLimit: dollarsToMicro(hourlyDollars.value),
      weeklyLimit: dollarsToMicro(weeklyDollars.value),
      totalLimit: dollarsToMicro(totalDollars.value),
    })
    toast.add({ title: 'Limits saved', color: 'success' })
  } catch (e: unknown) {
    toast.add({ title: 'Failed to save limits', description: (e as Error).message, color: 'error' })
  } finally {
    isSaving.value = false
  }
}

const resetItems = [
  [
    {
      label: 'Reset Hourly',
      onSelect: () => handleReset('hourly'),
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

async function handleReset(type: 'hourly' | 'weekly' | 'total' | 'all') {
  try {
    await props.resetUsage(props.keyId, type)
    toast.add({ title: `${type.charAt(0).toUpperCase() + type.slice(1)} usage reset`, color: 'success' })
  } catch (e: unknown) {
    toast.add({ title: 'Failed to reset usage', description: (e as Error).message, color: 'error' })
  }
}
</script>

<template>
  <div class="border-t border-default pt-3 mt-3">
    <h3 class="text-sm font-semibold text-muted mb-2">Cost Limits (USD)</h3>
    <div class="flex flex-wrap gap-3 items-end">
      <UFormField label="Hourly ($)" class="w-32">
        <UInput
          :model-value="hourlyDollars ?? undefined"
          @update:model-value="(v: string | number) => hourlyDollars = v == null || v === '' ? null : Number(v)"
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
  </div>
</template>

<style scoped>
/* Hide number input spinner arrows */
.no-spinners :deep(input[type="number"]::-webkit-inner-spin-button),
.no-spinners :deep(input[type="number"]::-webkit-outer-spin-button) {
  -webkit-appearance: none;
  margin: 0;
}
.no-spinners :deep(input[type="number"]) {
  -moz-appearance: textfield;
}
</style>
