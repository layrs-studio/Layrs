import { randomUUID } from "node:crypto";
import { request as httpsRequest } from "node:https";

/** Node-only private fixture input. Never pass it to page.evaluate or attach it. */
export interface NativeProofFixture {
  origin: string;
  caPem: string;
  adminAccessToken: string;
  spaceId: string;
  layerId: string;
  firstStepId: string;
}

export interface NativeProofSeed {
  runId: string;
  pendingRunId: string;
  proofId: string;
  proofObjectId: string;
}

// Immutable ID of the locally inspected ubuntu:22.04 image. This helper does
// not pull or execute an image. A real runner is exercised by runner_drill.rs.
const fixtureImage = "sha256:8a4eacce82dfd41dd9dfb4473188b4a728a35a75c121e4c5387dda2930ea0063";
const fixtureLog = "UI proof fixture";
type JsonRecord = Record<string, unknown>;

/**
 * Synthesizes a successful run through the real authenticated HTTPS service.
 * This is UI test data, NOT evidence that a container or repository test ran.
 * It changes the fixture Layer's protection and must run without concurrent
 * policy changes. Both administrator and service credentials stay in Node.
 */
export async function seedNativeProof(fixture: NativeProofFixture): Promise<NativeProofSeed> {
  const origin = validatedOrigin(fixture.origin);
  const image = process.env.LAYRS_TEST_RUNNER_IMAGE ?? fixtureImage;
  if (!/^(?:[^\s]+@)?sha256:[a-fA-F0-9]{64}$/.test(image)) {
    throw new Error("Native proof fixture requires an immutable image pin");
  }
  for (const value of [fixture.caPem, fixture.adminAccessToken, fixture.spaceId, fixture.layerId, fixture.firstStepId]) {
    if (typeof value !== "string" || value.length === 0) throw new Error("Native proof fixture input is incomplete");
  }
  const call = (method: string, path: string, token: string, body?: JsonRecord) =>
    requestJson(origin, fixture.caPem, method, path, token, body);
  const space = `/v3/spaces/${encodeURIComponent(fixture.spaceId)}`;
  const service = await call("POST", "/v3/auth/service-accounts", fixture.adminAccessToken, {
    displayName: "UI proof fixture (synthetic)", label: "UI fixture only", expiresIn: 3600,
  });
  const principal = requiredString(service, "principal_id");
  const serviceToken = requiredString(service, "access_token");
  await call("POST", `${space}/members`, fixture.adminAccessToken, {
    principalId: principal, role: "automation",
  });
  const protection = await call("PUT", `${space}/layers/${encodeURIComponent(fixture.layerId)}/protection`, fixture.adminAccessToken, {
    minimumApprovals: 0, allowSelfApproval: false, rejectConflicts: true,
    recipe: { image, command: ["/bin/sh", "-c", "printf 'UI proof fixture'"], timeoutSeconds: 15, maximumLogBytes: 4096 },
    trustedRunnerPrincipalIds: [principal], maximumProofAgeSeconds: 3600,
  });
  const run = await call("POST", `${space}/runs`, serviceToken, {
    idempotencyKey: `ui-proof-fixture-${randomUUID()}`,
    stepId: fixture.firstStepId, recipeObjectId: requiredString(protection, "recipeObjectId"),
  });
  const runId = requiredString(run, "runId");
  const completed = await call("POST", `${space}/runs/${encodeURIComponent(runId)}/complete`, serviceToken, {
    outcome: "passed", log: fixtureLog,
  });
  const proofObjectId = requiredString(completed, "proofObjectId");
  const detail = await call("GET", `${space}/runs/${encodeURIComponent(runId)}`, fixture.adminAccessToken);
  const product = record(detail.product);
  const proof = record(product.record);
  const evidence = Array.isArray(detail.evidence) ? detail.evidence.map(record) : [];
  if (detail.stepId !== fixture.firstStepId || detail.proofObjectId !== proofObjectId
      || detail.outcome !== "passed" || !evidence.some((entry) => entry.log === fixtureLog)) {
    throw new Error("Native proof fixture read-back verification failed");
  }
  const pending = await call("POST", `${space}/runs/pending`, serviceToken, {
    idempotencyKey: `ui-cancel-fixture-${randomUUID()}`,
    stepId: fixture.firstStepId, recipeObjectId: requiredString(protection, "recipeObjectId"),
  });
  return { runId, pendingRunId: requiredString(pending,"runId"), proofObjectId, proofId: requiredString(proof, "proofId") };
}

function validatedOrigin(value: string): URL {
  let url: URL;
  try { url = new URL(value); } catch { throw new Error("Native proof fixture origin is invalid"); }
  if (url.protocol !== "https:" || !["localhost", "127.0.0.1", "[::1]"].includes(url.hostname)
      || url.username || url.password || url.pathname !== "/" || url.search || url.hash) {
    throw new Error("Native proof seeding requires the disposable loopback HTTPS fixture");
  }
  return url;
}

function record(value: unknown): JsonRecord {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Native proof fixture response shape is invalid");
  }
  return value as JsonRecord;
}

function requiredString(value: JsonRecord, key: string): string {
  const field = value[key];
  if (typeof field !== "string" || field.length === 0) throw new Error("Native proof fixture response field is missing");
  return field;
}

/** Errors deliberately omit URLs, response bodies, tokens and original causes. */
function requestJson(origin: URL, ca: string, method: string, path: string, token: string, body?: JsonRecord): Promise<JsonRecord> {
  return new Promise((resolve, reject) => {
    let deadline: ReturnType<typeof setTimeout> | undefined;
    const fail = (message: string) => { clearTimeout(deadline); reject(new Error(message)); };
    try {
      const payload = body === undefined ? undefined : JSON.stringify(body);
      const request = httpsRequest(new URL(path, origin), {
        method, ca, rejectUnauthorized: true,
        headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      }, (response) => {
        const chunks: Buffer[] = [];
        let length = 0;
        response.on("data", (chunk: Buffer) => {
          length += chunk.length;
          if (length > 256 * 1024) {
            response.destroy();
            fail("Native proof fixture response exceeds limit");
          } else chunks.push(chunk);
        });
        response.on("error", () => fail("Native proof fixture response failed"));
        response.on("aborted", () => fail("Native proof fixture response was interrupted"));
        response.on("end", () => {
          clearTimeout(deadline);
          if (!response.statusCode || response.statusCode < 200 || response.statusCode >= 300) {
            fail(`Native proof fixture request failed (HTTP ${response.statusCode ?? 0})`);
            return;
          }
          try { resolve(record(JSON.parse(Buffer.concat(chunks).toString("utf8")))); }
          catch { fail("Native proof fixture response is invalid JSON"); }
        });
      });
      deadline = setTimeout(() => {
        request.destroy(); fail("Native proof fixture request timed out");
      }, 15_000);
      request.on("error", () => fail("Native proof fixture HTTPS request failed"));
      request.end(payload);
    } catch { fail("Native proof fixture HTTPS request could not start"); }
  });
}
