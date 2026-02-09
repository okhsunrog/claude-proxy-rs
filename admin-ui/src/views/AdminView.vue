<script setup lang="ts">
import { useRouter } from 'vue-router'
import OAuthSection from '../components/OAuthSection.vue'
import KeysList from '../components/KeysList.vue'
import { useAuth } from '../composables/useAuth'

const { authRequired, logout } = useAuth()
const router = useRouter()

async function handleLogout() {
  await logout()
  router.push({ name: 'login' })
}
</script>

<template>
  <div class="max-w-[900px] mx-auto p-8">
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
      <KeysList />
    </div>
  </div>
</template>
