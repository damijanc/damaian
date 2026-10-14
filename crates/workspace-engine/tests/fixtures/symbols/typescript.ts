// Accuracy fixture for the heuristic TypeScript extractor. Read as text, never compiled.
import { readFile } from "./files";

export interface UploadOptions {
  retries: number;
}

export function uploadRetry(options: UploadOptions): boolean {
  const attempts = options.retries;
  let remaining = attempts;
  return remaining > 0;
}

export class Uploader {
  upload(path: string): void {
    const local = path;
    console.log(local, readFile);
  }
}

export const MAX_RETRIES = 5;

let counter = 0;

export async function fetchRemote(): Promise<void> {}

export type UploadMap = Map<string, Uploader>;

export enum UploadState {
  Pending,
  Done,
}

function helper(): number {
  return counter;
}

// function lineCommented() {}

/*
function blockCommented() {}
*/

const template = `
function insideTemplate() {}
`;

interface InternalShape {
  value: string;
}
