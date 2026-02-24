<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import OAuthSection from '../components/OAuthSection.vue'
import KeysList from '../components/KeysList.vue'
import ModelManagement from '../components/ModelManagement.vue'
import UsageHistory from '../components/UsageHistory.vue'
import { useAuth } from '../composables/useAuth'

const { authRequired, logout } = useAuth()
const router = useRouter()
const activeTab = ref('keys')

const tabs = [
  { label: 'API Keys', value: 'keys', icon: 'i-lucide-key' },
  { label: 'Models', value: 'models', icon: 'i-lucide-cpu' },
  { label: 'Usage History', value: 'usage', icon: 'i-lucide-chart-area' },
]

async function handleLogout() {
  await logout()
  router.push({ name: 'login' })
}
</script>

<template>
  <div class="max-w-[1100px] mx-auto p-8">
    <div class="flex justify-between items-center mb-6">
      <h1 class="text-2xl font-semibold">Claude Proxy Admin</h1>
      <div class="flex items-center gap-2">
        <UColorModeButton />
        <UButton
          v-if="authRequired"
          color="neutral"
          variant="ghost"
          icon="i-lucide-log-out"
          @click="handleLogout"
        >
          Logout
        </UButton>
      </div>
    </div>

    <div class="space-y-6">
      <OAuthSection />

      <UCard>
        <UTabs v-model="activeTab" :items="tabs" :content="false" />
        <div class="mt-4">
          <KeysList v-if="activeTab === 'keys'" />
          <ModelManagement v-else-if="activeTab === 'models'" />
          <UsageHistory v-else-if="activeTab === 'usage'" />
        </div>
      </UCard>
    </div>
  </div>
</template>
