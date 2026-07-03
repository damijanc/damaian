export class ModelAdapter {
  async streamResponse() {
    throw new Error('ModelAdapter.streamResponse must be implemented by a provider adapter');
  }

  async requestStructuredOutput() {
    throw new Error('ModelAdapter.requestStructuredOutput must be implemented by a provider adapter');
  }

  cancel() {
    throw new Error('ModelAdapter.cancel must be implemented by a provider adapter');
  }

  estimateTokens(payload) {
    return Math.ceil(JSON.stringify(payload).length / 4);
  }
}
