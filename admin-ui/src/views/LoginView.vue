<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { useAuth } from '../composables/useAuth'

const { login } = useAuth()
const router = useRouter()

const username = ref('')
const password = ref('')
const error = ref<string | null>(null)
const isLoading = ref(false)

async function handleLogin() {
  if (!username.value || !password.value) return

  isLoading.value = true
  error.value = null

  const result = await login(username.value, password.value)
  isLoading.value = false

  if (result.success) {
    router.push({ name: 'admin' })
  } else {
    error.value = result.error ?? 'Login failed'
  }
}
</script>

<template>
  <div class="min-h-screen flex items-center justify-center">
    <UCard class="w-full max-w-sm">
      <template #header>
        <div class="flex justify-between items-center">
          <h1 class="text-xl font-semibold">Claude Proxy Admin</h1>
          <UColorModeButton />
        </div>
      </template>

      <form class="space-y-4" @submit.prevent="handleLogin">
        <UFormField label="Username">
          <UInput v-model="username" autocomplete="username" />
        </UFormField>

        <UFormField label="Password">
          <UInput v-model="password" type="password" autocomplete="current-password" />
        </UFormField>

        <UAlert v-if="error" color="error" :title="error" />

        <UButton type="submit" color="primary" block :loading="isLoading">
          Log in
        </UButton>
      </form>
    </UCard>
  </div>
</template>
