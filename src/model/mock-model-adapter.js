import { ModelAdapter } from './model-adapter.js';
import { createId, nowIso } from '../core/hash.js';

export class MockModelAdapter extends ModelAdapter {
  constructor({ response = '' } = {}) {
    super();
    this.response = response;
    this.cancelled = new Set();
  }

  async streamResponse(request, callbacks = {}) {
    const runId = createId('modelrun');
    let content = '';
    for (const token of this.response.match(/.{1,24}/gs) ?? []) {
      if (this.cancelled.has(runId)) break;
      content += token;
      callbacks.onToken?.(token, { runId });
    }
    const result = {
      runId,
      provider: 'mock',
      model: request.model ?? 'mock-model',
      startedAt: nowIso(),
      completedAt: nowIso(),
      content,
      incomplete: this.cancelled.has(runId)
    };
    callbacks.onComplete?.(result);
    return result;
  }

  async requestStructuredOutput(_schema, request) {
    return {
      provider: 'mock',
      model: request.model ?? 'mock-model',
      value: JSON.parse(this.response || '{}')
    };
  }

  cancel(runId) {
    this.cancelled.add(runId);
  }
}
