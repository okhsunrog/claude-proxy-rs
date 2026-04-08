<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { deleteWebSession, getWebSessionStatus, saveWebSession } from '../client/sdk.gen'
import { useOAuth } from '../composables/useOAuth'

const {
  isConnected,
  isLoading,
  isLoadingUsage,
  error,
  showCodeInput,
  subscriptionUsage,
  planName,
  checkStatus,
  connect,
  exchangeCode,
  disconnect,
  loadUsage,
} = useOAuth()
const toast = useToast()
const oauthCode = ref('')
const showDisconnectModal = ref(false)

const webSessionConfigured = ref(false)
const webSessionFormOpen = ref(false)
const webSessionSaving = ref(false)
const webSessionForm = ref({
  session_key: '',
  org_uuid: '',
  device_id: '',
  anonymous_id: '',
})

async function refreshWebSessionStatus() {
  try {
    const res = await getWebSessionStatus()
    webSessionConfigured.value = res.data?.configured ?? false
  } catch {
    webSessionConfigured.value = false
  }
}

async function handleSaveWebSession() {
  webSessionSaving.value = true
  try {
    await saveWebSession({ body: { ...webSessionForm.value } })
    toast.add({ title: 'Web session saved', color: 'success' })
    webSessionFormOpen.value = false
    webSessionForm.value = { session_key: '', org_uuid: '', device_id: '', anonymous_id: '' }
    await refreshWebSessionStatus()
    await loadUsage()
  } catch (e) {
    toast.add({ title: 'Failed to save web session', description: String(e), color: 'error' })
  } finally {
    webSessionSaving.value = false
  }
}

async function handleDeleteWebSession() {
  try {
    await deleteWebSession()
    toast.add({ title: 'Web session cleared', color: 'success' })
    await refreshWebSessionStatus()
    await loadUsage()
  } catch (e) {
    toast.add({ title: 'Failed to clear web session', description: String(e), color: 'error' })
  }
}

onMounted(() => {
  checkStatus()
  refreshWebSessionStatus()
})

async function handleExchangeCode() {
  const success = await exchangeCode(oauthCode.value)
  if (success) {
    oauthCode.value = ''
    toast.add({ title: 'Successfully connected!', color: 'success' })
  }
}

async function handleDisconnect() {
  showDisconnectModal.value = false
  const success = await disconnect()
  if (success) {
    toast.add({ title: 'Disconnected successfully', color: 'success' })
  }
}

function barColor(pct: number): string {
  if (pct >= 90) return 'error'
  if (pct >= 70) return 'warning'
  return 'success'
}

function formatResetTime(isoString: string): string {
  const reset = new Date(isoString)
  const now = new Date()
  const diffMs = reset.getTime() - now.getTime()
  if (diffMs <= 0) return 'Resetting...'
  const hours = Math.floor(diffMs / 3600000)
  const minutes = Math.floor((diffMs % 3600000) / 60000)
  const abs = `${String(reset.getDate()).padStart(2, '0')}/${String(reset.getMonth() + 1).padStart(2, '0')}, ${String(reset.getHours()).padStart(2, '0')}:${String(reset.getMinutes()).padStart(2, '0')}`
  if (hours >= 24) {
    const days = Math.floor(hours / 24)
    const remainingHours = hours % 24
    return `Resets in ${days}d ${remainingHours}h (${abs})`
  }
  return `Resets in ${hours}h ${minutes}m (${abs})`
}

interface UsageDisplayItem {
  label: string
  utilization: number
  resetsAt?: string | null
  subtitle?: string
}

function getUsageItems(): UsageDisplayItem[] {
  if (!subscriptionUsage.value) return []
  const items: UsageDisplayItem[] = []
  const u = subscriptionUsage.value

  if (u.five_hour?.utilization != null) {
    items.push({
      label: 'Session (5h)',
      utilization: u.five_hour.utilization,
      resetsAt: u.five_hour.resets_at,
    })
  }
  if (u.seven_day?.utilization != null) {
    items.push({
      label: 'Weekly (all)',
      utilization: u.seven_day.utilization,
      resetsAt: u.seven_day.resets_at,
    })
  }
  if (u.seven_day_opus?.utilization != null) {
    items.push({
      label: 'Weekly (Opus)',
      utilization: u.seven_day_opus.utilization,
      resetsAt: u.seven_day_opus.resets_at,
    })
  }
  if (u.seven_day_sonnet?.utilization != null) {
    items.push({
      label: 'Weekly (Sonnet)',
      utilization: u.seven_day_sonnet.utilization,
      resetsAt: u.seven_day_sonnet.resets_at,
    })
  }
  if (u.extra_usage) {
    const e = u.extra_usage
    if (e.is_enabled && e.utilization != null) {
      const spent = e.used_credits != null ? `$${(e.used_credits / 100).toFixed(2)}` : null
      const limit = e.monthly_limit != null ? `$${(e.monthly_limit / 100).toFixed(2)}` : null
      items.push({
        label: 'Extra usage',
        utilization: e.utilization,
        subtitle: spent && limit ? `${spent} / ${limit} spent` : undefined,
      })
    }
  }
  return items
}
</script>

<template>
  <UCard>
    <template #header>
      <div class="flex items-center justify-between">
        <h2 class="text-xl font-semibold">Claude Subscription</h2>
        <UButton
          v-if="isConnected && subscriptionUsage"
          size="xs"
          variant="ghost"
          icon="i-lucide-refresh-cw"
          :loading="isLoadingUsage"
          @click="loadUsage"
        />
      </div>
    </template>

    <div class="space-y-4">
      <!-- Status row: badge + plan + connect/disconnect button all inline -->
      <div class="flex items-center gap-3">
        <span class="text-sm">Status:</span>
        <UBadge v-if="isConnected" color="success" variant="subtle">Connected</UBadge>
        <UBadge v-else color="error" variant="subtle">Not connected</UBadge>
        <UBadge v-if="planName" color="info" variant="subtle">{{ planName }}</UBadge>
        <UButton
          v-if="!isConnected"
          size="xs"
          color="primary"
          :loading="isLoading"
          @click="connect"
        >
          Connect with Claude
        </UButton>
        <UButton
          v-else
          size="xs"
          color="error"
          variant="soft"
          :loading="isLoading"
          @click="showDisconnectModal = true"
        >
          Disconnect
        </UButton>
      </div>

      <!-- Upstream error (shown even when no usage data available) -->
      <UAlert
        v-if="isConnected && subscriptionUsage?.upstream_error"
        color="error"
        variant="subtle"
        icon="i-lucide-triangle-alert"
        title="Anthropic usage endpoint failed"
        :description="subscriptionUsage.upstream_error"
      />

      <!-- Subscription Limits -->
      <div v-if="isConnected && subscriptionUsage && getUsageItems().length > 0">
        <div class="flex items-center justify-between mb-2">
          <div class="text-xs text-muted uppercase">Subscription Limits</div>
          <UBadge v-if="subscriptionUsage.is_stale" color="warning" variant="subtle" size="sm">
            Stale (using fallback)
          </UBadge>
        </div>
        <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <div v-for="item in getUsageItems()" :key="item.label" class="rounded-lg bg-elevated p-3">
            <div class="text-xs text-muted uppercase">{{ item.label }}</div>
            <div class="text-lg font-semibold mt-1">{{ Math.floor(item.utilization) }}%</div>
            <UProgress
              :model-value="Math.min(item.utilization, 100)"
              :color="barColor(item.utilization)"
              size="xs"
              class="mt-1.5"
            />
            <div v-if="item.resetsAt" class="text-xs text-muted mt-1">
              {{ formatResetTime(item.resetsAt) }}
            </div>
            <div v-if="item.subtitle" class="text-xs text-muted mt-1">
              {{ item.subtitle }}
            </div>
          </div>
        </div>
      </div>

      <div v-if="isConnected && isLoadingUsage && !subscriptionUsage" class="text-sm text-muted">
        Loading usage...
      </div>

      <!-- Claude.ai web session (bypass rate limit on /oauth/usage) -->
      <div v-if="isConnected" class="rounded-lg border border-default p-3 space-y-2">
        <div class="flex items-center justify-between">
          <div>
            <div class="text-xs text-muted uppercase">Claude.ai web session</div>
            <div class="text-sm text-muted">
              Optional. Uses a browser cookie to fetch usage, bypassing the OAuth rate limit. Cookie
              is auto-rotated on every request.
            </div>
          </div>
          <UBadge :color="webSessionConfigured ? 'success' : 'neutral'" variant="subtle" size="sm">
            {{ webSessionConfigured ? 'Configured' : 'Not configured' }}
          </UBadge>
        </div>
        <div class="flex gap-2">
          <UButton
            size="xs"
            color="primary"
            variant="soft"
            @click="webSessionFormOpen = !webSessionFormOpen"
          >
            {{ webSessionConfigured ? 'Replace' : 'Configure' }}
          </UButton>
          <UButton
            v-if="webSessionConfigured"
            size="xs"
            color="error"
            variant="soft"
            @click="handleDeleteWebSession"
          >
            Clear
          </UButton>
        </div>
        <div v-if="webSessionFormOpen" class="space-y-2 pt-2">
          <p class="text-xs text-muted">
            From claude.ai/settings/usage → DevTools → Network → usage request. Copy sessionKey
            value from Cookie, org uuid from URL path, device-id and anonymous-id from request
            headers.
          </p>
          <UInput
            v-model="webSessionForm.session_key"
            placeholder="sessionKey (sk-ant-sid02-...)"
            size="sm"
          />
          <UInput
            v-model="webSessionForm.org_uuid"
            placeholder="Org UUID (from /api/organizations/{uuid}/usage)"
            size="sm"
          />
          <UInput
            v-model="webSessionForm.device_id"
            placeholder="anthropic-device-id header"
            size="sm"
          />
          <UInput
            v-model="webSessionForm.anonymous_id"
            placeholder="anthropic-anonymous-id header"
            size="sm"
          />
          <div class="flex gap-2 justify-end">
            <UButton size="sm" variant="ghost" @click="webSessionFormOpen = false">
              Cancel
            </UButton>
            <UButton
              size="sm"
              color="primary"
              :loading="webSessionSaving"
              @click="handleSaveWebSession"
            >
              Save
            </UButton>
          </div>
        </div>
      </div>

      <div v-if="showCodeInput" class="space-y-2">
        <p class="text-sm text-muted">Paste the authorization code from the popup:</p>
        <div class="flex gap-2">
          <UInput
            v-model="oauthCode"
            placeholder="Authorization code"
            class="flex-1"
            @keyup.enter="handleExchangeCode"
          />
          <UButton color="primary" :loading="isLoading" @click="handleExchangeCode">
            Submit
          </UButton>
        </div>
      </div>

      <UAlert
        v-if="error"
        color="error"
        title="Error"
        :description="error"
        :close="{
          onClick: () => {
            error = null
          },
        }"
      />
    </div>

    <UModal
      v-model:open="showDisconnectModal"
      title="Confirm Disconnect"
      :ui="{ width: 'max-w-md' }"
    >
      <template #body>
        <p>Are you sure you want to disconnect?</p>
      </template>
      <template #footer>
        <div class="flex justify-end gap-2">
          <UButton variant="ghost" @click="showDisconnectModal = false">Cancel</UButton>
          <UButton color="error" @click="handleDisconnect">Disconnect</UButton>
        </div>
      </template>
    </UModal>
  </UCard>
</template>
