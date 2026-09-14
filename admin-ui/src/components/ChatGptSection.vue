<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { chatgptLogin, chatgptLogout, chatgptPoll, chatgptStatus } from '../client'
import type { DeviceLogin } from '../client'

const connected = ref(false)
const busy = ref(false)
const login = ref<DeviceLogin | null>(null)
const error = ref('')
const message = ref('')

async function refresh() {
  try {
    const result = await chatgptStatus({ throwOnError: true })
    connected.value = result.data.authenticated
  } catch {
    error.value = 'Unable to read ChatGPT connection status.'
  }
}

async function start() {
  busy.value = true
  error.value = ''
  message.value = ''
  try {
    const result = await chatgptLogin({ throwOnError: true })
    login.value = result.data
  } catch {
    error.value =
      'Unable to start login. Check that device code login is enabled in ChatGPT settings.'
  } finally {
    busy.value = false
  }
}

async function check() {
  busy.value = true
  error.value = ''
  try {
    const result = await chatgptPoll({ throwOnError: true })
    if (result.data.authenticated) {
      connected.value = true
      login.value = null
      message.value = ''
    } else {
      message.value = 'Waiting for approval. Finish signing in, then check again.'
    }
  } catch {
    error.value = 'Login failed or expired. Start a new connection.'
    login.value = null
  } finally {
    busy.value = false
  }
}

async function disconnect() {
  busy.value = true
  error.value = ''
  try {
    await chatgptLogout({ throwOnError: true })
    connected.value = false
    login.value = null
    message.value = ''
  } catch {
    error.value = 'Unable to disconnect ChatGPT.'
  } finally {
    busy.value = false
  }
}

onMounted(refresh)
</script>

<template>
  <UCard>
    <div class="flex flex-wrap items-center justify-between gap-3">
      <div>
        <h2 class="text-lg font-semibold">ChatGPT transcription</h2>
        <p class="text-sm text-muted">Connect one account to turn audio recordings into text.</p>
      </div>
      <UBadge :color="connected ? 'success' : 'neutral'">
        {{ connected ? 'Connected' : 'Not connected' }}
      </UBadge>
    </div>
    <UAlert v-if="error" class="mt-4" color="error" :description="error" />
    <div v-if="login" class="mt-4 space-y-3">
      <p>Open ChatGPT and enter this code:</p>
      <p class="font-mono text-xl tracking-wider">{{ login.user_code }}</p>
      <div class="flex flex-wrap gap-2">
        <UButton :to="login.verification_url" target="_blank">Open ChatGPT</UButton>
        <UButton :loading="busy" color="neutral" @click="check">Check sign-in</UButton>
        <UButton :disabled="busy" color="neutral" variant="ghost" @click="disconnect"
          >Cancel</UButton
        >
      </div>
      <p v-if="message" class="text-sm text-muted">{{ message }}</p>
      <p class="text-sm text-muted">The code expires after 15 minutes.</p>
    </div>
    <div v-else class="mt-4">
      <UButton v-if="!connected" :loading="busy" @click="start">Connect ChatGPT</UButton>
      <UButton v-else :loading="busy" color="neutral" variant="outline" @click="disconnect">
        Disconnect
      </UButton>
    </div>
    <p class="mt-4 text-sm text-muted">
      File transcription only. Up to 25 MiB per recording and 10 requests per minute per key.
      Transcription is separate from Claude quotas and token-cost reports.
    </p>
  </UCard>
</template>
