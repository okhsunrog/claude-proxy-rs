import { ref } from 'vue'
import { startOauthFlow, exchangeOauthCode, deleteOauth, getOauthStatus } from '../client'

export function useOAuth() {
  const isConnected = ref(false)
  const isLoading = ref(false)
  const error = ref<string | null>(null)
  const showCodeInput = ref(false)

  async function checkStatus() {
    try {
      const { data } = await getOauthStatus()
      isConnected.value = data?.authenticated ?? false
    } catch (e) {
      console.error('Failed to check OAuth status:', e)
    }
  }

  async function connect() {
    isLoading.value = true
    error.value = null
    try {
      const { data } = await startOauthFlow()
      if (data?.url) {
        window.open(data.url, 'oauth', 'width=600,height=800')
        showCodeInput.value = true
      }
    } catch {
      error.value = 'Failed to start OAuth flow'
    } finally {
      isLoading.value = false
    }
  }

  async function exchangeCode(code: string) {
    if (!code.trim()) return
    isLoading.value = true
    error.value = null
    try {
      const { data, error: apiError } = await exchangeOauthCode({
        body: { code: code.trim() },
      })
      if (data?.success) {
        showCodeInput.value = false
        await checkStatus()
        return true
      } else {
        error.value = (apiError as { error?: string })?.error || 'Failed to exchange code'
        return false
      }
    } catch {
      error.value = 'Failed to exchange code'
      return false
    } finally {
      isLoading.value = false
    }
  }

  async function disconnect() {
    isLoading.value = true
    error.value = null
    try {
      await deleteOauth()
      await checkStatus()
      return true
    } catch {
      error.value = 'Failed to disconnect'
      return false
    } finally {
      isLoading.value = false
    }
  }

  return {
    isConnected,
    isLoading,
    error,
    showCodeInput,
    checkStatus,
    connect,
    exchangeCode,
    disconnect,
  }
}
