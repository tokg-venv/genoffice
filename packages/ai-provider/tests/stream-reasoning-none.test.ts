import { afterEach, describe, expect, it, vi } from 'vitest'
import { streamForProvider } from '../src/stream'
import { okResponse, sseStream } from './test-utils'

afterEach(() => {
  vi.unstubAllGlobals()
})

const TOOL = [{ name: 'read_file', description: 'read a file', inputSchema: { type: 'object' } }]

const sse = () =>
  sseStream([
    'data: {"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}',
    'data: [DONE]',
  ])

function collector() {
  return {
    cb: {
      signal: new AbortController().signal,
      onDelta: vi.fn(),
      onToolCall: vi.fn(),
      onStopReason: vi.fn(),
    },
  }
}

async function toolTurnBody(model: string, provider: 'openai' | 'genspark' | 'custom' = 'openai') {
  const fetchMock = vi.fn().mockImplementation(async () => okResponse(sse()))
  vi.stubGlobal('fetch', fetchMock)
  const { cb } = collector()
  const config =
    provider === 'custom'
      ? { apiKey: 'k', model, baseUrl: 'https://relay.example.internal/v1' }
      : { apiKey: 'k', model }
  await streamForProvider(provider, config, 'sys', [], TOOL, 100, cb)
  return JSON.parse(fetchMock.mock.calls[0][1].body as string) as Record<string, unknown>
}

describe('streamForProvider: reasoning_effort beside function tools', () => {
  it('pins effort none for the reasoning-line families on plain chat-completions endpoints', async () => {
    for (const model of ['gpt-5.6-terra', 'gpt-5.5', 'gpt-6-astra']) {
      const body = await toolTurnBody(model)
      expect(body.tools).toHaveLength(1)
      expect(body.reasoning_effort).toBe('none')
    }
    // OpenRouter spells the vendor into the id; the predicate still matches
    const openrouter = await toolTurnBody('openai/gpt-6-sol', 'custom')
    expect(openrouter.reasoning_effort).toBe('none')
  })

  it('leaves other models and tool-less turns alone', async () => {
    const body = await toolTurnBody('o4-mini')
    expect(body.reasoning_effort).toBeUndefined()
    const fetchMock = vi.fn().mockImplementation(async () => okResponse(sse()))
    vi.stubGlobal('fetch', fetchMock)
    const { cb } = collector()
    await streamForProvider(
      'openai',
      { apiKey: 'k', model: 'gpt-5.6-terra' },
      'sys',
      [],
      [],
      100,
      cb,
    )
    expect(JSON.parse(fetchMock.mock.calls[0][1].body as string).reasoning_effort).toBeUndefined()
  })

  it('the Genspark proxy keeps gpt-6 tool turns at their default reasoning', async () => {
    // its LiteLLM backend serves these live-verified working as-is; the pin
    // must not change behavior the endpoint already handles
    const body = await toolTurnBody('gpt-6-astra', 'genspark')
    expect(body.reasoning_effort).toBeUndefined()
  })
})
