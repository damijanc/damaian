#!/usr/bin/env node
// Verifies the updater signature this build just produced against the public
// key that is compiled into the shipped application. create-updater-manifest.mjs
// copies the .sig file into the manifest without checking it, so without this
// step a release signed by the wrong key — which would brick the updater for
// every existing install — would reach users unnoticed.
import { createHash, createPublicKey, verify } from "node:crypto";
import { readFile } from "node:fs/promises";
import path from "node:path";

// Raw Ed25519 keys need an SPKI wrapper before node:crypto will accept them.
const SPKI_ED25519_PREFIX = Buffer.from("302a300506032b6570032100", "hex");
const PUBLIC_KEY_PAYLOAD_BYTES = 42; // 2-byte algorithm tag + 8-byte key ID + 32-byte key
const SIGNATURE_PAYLOAD_BYTES = 74; // 2-byte algorithm tag + 8-byte key ID + 64-byte signature
// The separator space is part of the line, not of the signed comment.
const TRUSTED_COMMENT_PREFIX = "trusted comment: ";

function fail(message) {
  console.error(`Updater signature verification failed: ${message}`);
  process.exit(1);
}

// Buffer.from(value, "base64") silently skips characters it does not recognise,
// so a corrupt blob decodes to something short instead of throwing. Validate the
// encoding before trusting any length check performed on the result.
function decodeBase64(value, label) {
  const compact = value.replace(/\s+/g, "");
  if (compact.length === 0 || compact.length % 4 !== 0) {
    fail(`${label} is not valid base64`);
  }
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(compact)) {
    fail(`${label} contains characters that are not base64`);
  }
  const decoded = Buffer.from(compact, "base64");
  if (decoded.toString("base64") !== compact) {
    fail(`${label} is not canonical base64`);
  }
  return decoded;
}

// A missing input is a build failure like any other, but it should say which
// file is missing rather than surface a Node stack trace.
async function read(filePath, label, encoding) {
  try {
    return await readFile(filePath, encoding);
  } catch (error) {
    fail(`cannot read ${label} at ${filePath}: ${error.code || error.message}`);
  }
}

function minisignLines(blob, label) {
  const lines = blob.toString("utf8").split("\n");
  if (lines.length < 2 || lines[1].trim().length === 0) {
    fail(`${label} is not a minisign blob`);
  }
  return lines;
}

function parsePublicKey(value) {
  const lines = minisignLines(decodeBase64(value, "TAURI_UPDATER_PUBKEY"), "TAURI_UPDATER_PUBKEY");
  const payload = decodeBase64(lines[1], "public key payload");
  if (payload.length !== PUBLIC_KEY_PAYLOAD_BYTES) {
    fail(`public key payload is ${payload.length} bytes, expected ${PUBLIC_KEY_PAYLOAD_BYTES}`);
  }
  const algorithm = payload.subarray(0, 2).toString("utf8");
  // A minisign public key is always tagged "Ed"; only signatures vary.
  if (algorithm !== "Ed") {
    fail(`public key algorithm is "${algorithm}", expected "Ed"`);
  }
  return {
    keyId: payload.subarray(2, 10),
    key: createPublicKey({
      key: Buffer.concat([SPKI_ED25519_PREFIX, payload.subarray(10)]),
      format: "der",
      type: "spki",
    }),
  };
}

function parseSignature(blob) {
  const lines = minisignLines(blob, "signature file");
  const payload = decodeBase64(lines[1], "signature payload");
  if (payload.length !== SIGNATURE_PAYLOAD_BYTES) {
    fail(`signature payload is ${payload.length} bytes, expected ${SIGNATURE_PAYLOAD_BYTES}`);
  }

  const trustedCommentLine = lines.find((line) => line.startsWith(TRUSTED_COMMENT_PREFIX));
  if (trustedCommentLine === undefined) {
    fail("signature file has no trusted comment line");
  }
  const globalSignatureLine = lines[lines.indexOf(trustedCommentLine) + 1];
  if (globalSignatureLine === undefined || globalSignatureLine.trim().length === 0) {
    fail("signature file has no global signature after its trusted comment");
  }

  return {
    // "Ed" signs the file bytes; "ED" signs a BLAKE2b-512 hash of them. The tag
    // deliberately differs from the public key's, so the two are never compared.
    algorithm: payload.subarray(0, 2).toString("utf8"),
    keyId: payload.subarray(2, 10),
    signature: payload.subarray(10),
    trustedComment: trustedCommentLine.slice(TRUSTED_COMMENT_PREFIX.length),
    globalSignature: decodeBase64(globalSignatureLine, "global signature"),
  };
}

const pubkeyValue = (process.env.TAURI_UPDATER_PUBKEY || "").trim();
if (!pubkeyValue) {
  fail("TAURI_UPDATER_PUBKEY is required");
}

const channel = (process.env.RELEASE_CHANNEL || "preview").trim();
if (channel !== "stable" && channel !== "preview") {
  fail(`RELEASE_CHANNEL must be "stable" or "preview", got "${channel}"`);
}
const tag = (process.env.TAG_NAME || process.argv[2] || "").trim();
if (!tag) {
  fail("TAG_NAME or first argument is required");
}

const bundleDir = "target/release/bundle/macos";
const bundleName = "Damaian.app.tar.gz";
const bundlePath = path.join(bundleDir, bundleName);
const manifestName = channel === "stable" ? "latest.json" : "preview.json";
const manifestPath = path.join("target/release/bundle/updater", manifestName);

const publicKey = parsePublicKey(pubkeyValue);
const signatureFile = await read(`${bundlePath}.sig`, "updater signature", "utf8");
const signature = parseSignature(decodeBase64(signatureFile, ".sig"));

// A signature made by a different private key than the one shipped in the app
// must fail the build rather than reach users, whose installs would then reject
// every future update.
if (!publicKey.keyId.equals(signature.keyId)) {
  fail(
    `signature key ID ${signature.keyId.toString("hex")} does not match ` +
      `public key ID ${publicKey.keyId.toString("hex")}`,
  );
}

const bundleBytes = await read(bundlePath, "updater bundle");
let signedMessage;
if (signature.algorithm === "ED") {
  signedMessage = createHash("blake2b512").update(bundleBytes).digest();
} else if (signature.algorithm === "Ed") {
  signedMessage = bundleBytes;
} else {
  fail(`unknown signature algorithm "${signature.algorithm}"`);
}

if (!verify(null, signedMessage, publicKey.key, signature.signature)) {
  fail(`${bundleName} does not match its signature`);
}

// minisign covers the trusted comment with a second signature over
// signature || trusted_comment, so the name and timestamp the updater displays
// can be checked rather than taken on trust.
const globalMessage = Buffer.concat([
  signature.signature,
  Buffer.from(signature.trustedComment, "utf8"),
]);
if (!verify(null, globalMessage, publicKey.key, signature.globalSignature)) {
  fail("trusted comment does not match its global signature");
}

// A manifest stamped with the wrong version would point installs at another
// release's binary.
const manifest = JSON.parse(await read(manifestPath, "updater manifest", "utf8"));
const url = manifest.platforms?.["darwin-aarch64"]?.url;
if (typeof url !== "string") {
  fail(`${manifestName} has no darwin-aarch64 url`);
}
if (!url.includes(`/download/${tag}/`)) {
  fail(`${manifestName} url ${url} does not point at ${tag}`);
}
if (manifest.channel !== channel) {
  fail(`${manifestName} declares channel "${manifest.channel}", expected "${channel}"`);
}

console.log(
  `Verified ${bundleName} signature against TAURI_UPDATER_PUBKEY ` +
    `(key ${publicKey.keyId.toString("hex")}, ${manifestName} -> ${tag})`,
);
