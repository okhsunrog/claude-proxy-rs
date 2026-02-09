import { ref } from 'vue'

interface AuthCheckResponse {
  authenticated: boolean
  authRequired: boolean
}

const isAuthenticated = ref(false)
const authRequired = ref(true)
const isChecking = ref(false)

export function useAuth() {
  async function checkAuth(): Promise<AuthCheckResponse> {
    isChecking.value = true
    try {
      const res = await fetch('/admin/auth/check', { credentials: 'same-origin' })
      const data: AuthCheckResponse = await res.json()
      isAuthenticated.value = data.authenticated
      authRequired.value = data.authRequired
      return data
    } catch {
      isAuthenticated.value = false
      authRequired.value = true
      return { authenticated: false, authRequired: true }
    } finally {
      isChecking.value = false
    }
  }

  async function login(
    username: string,
    password: string,
  ): Promise<{ success: boolean; error?: string }> {
    try {
      const res = await fetch('/admin/auth/login', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'same-origin',
        body: JSON.stringify({ username, password }),
      })

      if (res.ok) {
        isAuthenticated.value = true
        return { success: true }
      } else {
        try {
          const data = await res.json()
          return { success: false, error: data.error || 'Invalid credentials' }
        } catch {
          return { success: false, error: `Server error (${res.status})` }
        }
      }
    } catch {
      return { success: false, error: 'Network error' }
    }
  }

  async function logout(): Promise<void> {
    try {
      await fetch('/admin/auth/logout', {
        method: 'POST',
        credentials: 'same-origin',
      })
    } finally {
      isAuthenticated.value = false
    }
  }

  return {
    isAuthenticated,
    authRequired,
    isChecking,
    checkAuth,
    login,
    logout,
  }
}
