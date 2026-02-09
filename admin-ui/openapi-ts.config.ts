import { defineConfig } from '@hey-api/openapi-ts'

export default defineConfig({
  input: 'http://localhost:4096/admin/api-docs/openapi.json',
  output: 'src/client',
  plugins: [
    '@hey-api/typescript',
    '@hey-api/sdk',
    {
      name: '@hey-api/client-fetch',
      baseUrl: '/admin',
    },
  ],
})
