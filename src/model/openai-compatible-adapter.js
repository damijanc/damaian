import { createId, nowIso } from '../core/hash.js';
import { ModelAdapter } from './model-adapter.js';

function parseSseChunks(buffer) {
  const events = [];
  const parts = buffer.split('\n\n');
  const remainder = parts.pop() ?? '';
  for (const part of parts) {
    const dataLines = part
      .split('\n')
      .filter((line) => line.startsWith('data:'))
      .map((line) => line.slice('data:'.length).trim());
    if (dataLines.length > 0) events.push(dataLines.join('\n'));
  }
  return { events, remainder };
}

export class OpenAICompatibleAdapter extends ModelAdapter {
  constructor({ provider = 'openai-compatible', baseUrl, apiKey, model, timeoutMs = 60_000, fetchImpl = globalThis.fetch } = {}) {
    super();
    this.provider = provider;
    this.baseUrl = baseUrl?.replace(/\/$/, '');
    this.apiKey = apiKey;
    this.model = model;
    this.timeoutMs = timeoutMs;
    this.fetchImpl = fetchImpl;
    this.controllers = new Map();
  }

  async streamResponse(request, callbacks = {}) {
    const runId = createId('modelrun');
    const startedAt = nowIso();
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);
    this.controllers.set(runId, controller);

    try {
      const response = await this.fetchImpl(`${this.baseUrl}/v1/chat/completions`, {
        method: 'POST',
        signal: controller.signal,
        headers: {
          'content-type': 'application/json',
          authorization: `Bearer ${this.apiKey}`
        },
        body: JSON.stringify({
          model: request.model ?? this.model,
          messages: request.messages,
          tools: request.tools,
          temperature: request.temperature,
          reasoning_effort: apiReasoningEffort(request.provider ?? this.provider, request.reasoningLevel),
          stream: true
        })
      });

      if (!response.ok) {
        const body = await response.text();
        const error = new Error(`Model provider error ${response.status}: ${body}`);
        callbacks.onError?.(error);
        throw error;
      }

      let content = '';
      let buffer = '';
      const decoder = new TextDecoder();
      for await (const chunk of response.body) {
        buffer += decoder.decode(chunk, { stream: true });
        const parsed = parseSseChunks(buffer);
        buffer = parsed.remainder;
        for (const event of parsed.events) {
          if (event === '[DONE]') continue;
          const payload = JSON.parse(event);
          const token = payload.choices?.[0]?.delta?.content ?? '';
          if (token) {
            content += token;
            callbacks.onToken?.(token, payload);
          }
        }
      }

      const result = {
        runId,
        provider: request.provider ?? this.provider,
        model: request.model ?? this.model,
        startedAt,
        completedAt: nowIso(),
        content,
        incomplete: false
      };
      callbacks.onComplete?.(result);
      return result;
    } finally {
      clearTimeout(timer);
      this.controllers.delete(runId);
    }
  }

  async requestStructuredOutput(schema, request) {
    const response = await this.fetchImpl(`${this.baseUrl}/v1/chat/completions`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        authorization: `Bearer ${this.apiKey}`
      },
      body: JSON.stringify({
        model: request.model ?? this.model,
        messages: request.messages,
        response_format: schema ? { type: 'json_object' } : undefined,
        temperature: request.temperature ?? 0,
        reasoning_effort: apiReasoningEffort(request.provider ?? this.provider, request.reasoningLevel)
      })
    });
    if (!response.ok) throw new Error(`Model provider error ${response.status}: ${await response.text()}`);
    const payload = await response.json();
    const content = payload.choices?.[0]?.message?.content ?? '{}';
    return {
      provider: request.provider ?? this.provider,
      model: request.model ?? this.model,
      raw: payload,
      value: JSON.parse(content)
    };
  }

  cancel(runId) {
    this.controllers.get(runId)?.abort();
  }
}

function apiReasoningEffort(provider, reasoningLevel) {
  if (!['openai', 'openai-compatible'].includes(provider)) return undefined;
  const level = String(reasoningLevel || '').trim().toLowerCase();
  return ['minimal', 'low', 'medium', 'high'].includes(level) ? level : undefined;
}
