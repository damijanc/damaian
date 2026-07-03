export class ClientError extends Error {
  constructor(message, code, details = {}) {
    super(message);
    this.name = this.constructor.name;
    this.code = code;
    this.details = details;
  }
}

export class AccessDeniedError extends ClientError {
  constructor(message, details = {}) {
    super(message, 'access_denied', details);
  }
}

export class ApprovalRequiredError extends ClientError {
  constructor(message, details = {}) {
    super(message, 'approval_required', details);
  }
}

export class PatchConflictError extends ClientError {
  constructor(message, details = {}) {
    super(message, 'patch_conflict', details);
  }
}

export class PolicyBlockedError extends ClientError {
  constructor(message, details = {}) {
    super(message, 'policy_blocked', details);
  }
}
