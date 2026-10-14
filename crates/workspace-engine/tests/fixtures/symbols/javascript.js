// Accuracy fixture for the heuristic JavaScript extractor. Read as text, never executed.
const path = require("./path");

function uploadRetry(attempts) {
  const limit = 3;
  return attempts < limit;
}

class Uploader {
  constructor(target) {
    this.target = target;
  }
}

export function createUploader() {
  let instance = new Uploader(path);
  return instance;
}

export const DEFAULT_TARGET = "/tmp";

let pending = 0;

const handler = () => {
  const inner = pending;
  return inner;
};

async function fetchRemote() {}

/*
function blockCommented() {}
*/

const message = `
function insideTemplate() {}
`;
