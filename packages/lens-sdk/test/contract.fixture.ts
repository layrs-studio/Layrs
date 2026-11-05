import type {
  CanonicalLensReconcileResult,
  LensBundleContract,
  LensDiffWindow,
  LensManifest,
  LensPin,
  LensPreviewWindow,
  LensReconcileRequest,
  LensReconcileResponse,
  LensResourceQuotas
} from "../src/contracts";
import { LENS_BUNDLE_FORMAT, LENS_CONTRACT_VERSION } from "../src/contracts";

const LENS_DIGEST = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

const pin: LensPin = {
  lensId: "layrs.example.markdown",
  version: "1.2.3",
  digest: LENS_DIGEST
};

const analyzerResources: LensResourceQuotas = {
  maxMemoryBytes: 67_108_864,
  maxExecutionMs: 2_000,
  maxInputBytes: 16_777_216,
  maxOutputBytes: 8_388_608,
  maxFuel: 10_000_000
};

const reconcileResources: LensResourceQuotas = {
  maxMemoryBytes: 67_108_864,
  maxExecutionMs: 3_000,
  maxInputBytes: 33_554_432,
  maxOutputBytes: 33_554_432,
  maxFuel: 20_000_000
};

const viewerResources: LensResourceQuotas = {
  maxMemoryBytes: 134_217_728,
  maxExecutionMs: 1_000,
  maxInputBytes: 8_388_608,
  maxOutputBytes: 8_388_608
};

const bundle: LensBundleContract = {
  format: LENS_BUNDLE_FORMAT,
  entrypoints: {
    analyzer: {
      runtime: "wasi",
      path: "analyzer.wasm",
      export: "analyze",
      digest: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      resources: analyzerResources
    },
    reconcile: {
      runtime: "wasi",
      path: "reconcile.wasm",
      export: "reconcile",
      digest: "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
      resources: reconcileResources
    },
    viewer: {
      runtime: "esm",
      path: "viewer.mjs",
      export: "default",
      digest: "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
      resources: viewerResources
    }
  },
  signature: {
    algorithm: "ed25519",
    keyId: "did:key:example#lens-signing",
    signedDigest: LENS_DIGEST,
    signature: "base64:ZXhhbXBsZS1zaWduYXR1cmU="
  }
};

const manifest: LensManifest = {
  contractVersion: LENS_CONTRACT_VERSION,
  id: "layrs.example.markdown",
  name: "Markdown",
  version: "1.2.3",
  digest: LENS_DIGEST,
  analyzer: {
    supportedMediaTypes: ["text/markdown"],
    fileExtensions: ["md", "mdown"],
    capabilities: ["view", "preview", "diff", "reconcile", "metadata"]
  },
  viewer: {
    viewerId: "layrs.viewer.example.markdown",
    schemaVersion: "layrs.viewer.v1",
    component: "MarkdownViewer",
    previewKinds: ["text"],
    diffKinds: ["textLines"],
    reconcileStatuses: [],
    inspectorFields: [
      {
        key: "wordCount",
        label: "Word count",
        valueType: "number"
      }
    ]
  },
  resolutionMethods: [
    { method: "existing", scope: "file" },
    { method: "incoming", scope: "file" },
    { method: "existing", scope: "block" },
    { method: "incoming", scope: "block" },
    { method: "both", scope: "block" },
    { method: "manual", scope: "block" }
  ],
  bundle
};

const previewWindow: LensPreviewWindow = {
  contractVersion: LENS_CONTRACT_VERSION,
  lens: { ...pin },
  artifactId: "artifact-123",
  unit: "lines",
  window: {
    start: 10,
    endExclusive: 30,
    total: 120
  }
};

const diffWindow: LensDiffWindow = {
  contractVersion: LENS_CONTRACT_VERSION,
  lens: { ...pin },
  path: "docs/readme.md",
  lines: {
    start: 40,
    endExclusive: 60,
    total: 200
  },
  columns: {
    start: 0,
    endExclusive: 120,
    total: 400
  }
};

const reconcileInput: LensReconcileRequest = {
  contractVersion: LENS_CONTRACT_VERSION,
  lens: { ...pin },
  path: "docs/readme.md",
  mediaType: "text/markdown",
  base: {
    exists: true,
    bytes: [98, 97, 115, 101, 10],
    contentHash: "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    size: 5
  },
  existing: {
    exists: true,
    bytes: [111, 117, 114, 115, 10],
    contentHash: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    size: 5
  },
  incoming: {
    exists: true,
    bytes: [116, 104, 101, 105, 114, 115, 10],
    contentHash: "sha256:1111111111111111111111111111111111111111111111111111111111111111",
    size: 7
  },
  labels: {
    existing: "Current layer",
    incoming: "Incoming layer"
  }
};

const autoResolved: CanonicalLensReconcileResult = {
  status: "auto_resolved",
  summary: "Existing and incoming content converge",
  resolved: {
    exists: true,
    bytes: [109, 101, 114, 103, 101, 100, 10]
  },
  resolutionMethods: [],
  blocks: [],
  segments: [],
  fields: {
    strategy: "threeWay"
  }
};

const conflicted: CanonicalLensReconcileResult = {
  status: "conflicted",
  summary: "One text block requires resolution",
  conflict: {
    exists: true,
    bytes: [111, 117, 114, 115, 10]
  },
  resolutionMethods: [
    { method: "existing", scope: "file" },
    { method: "incoming", scope: "file" }
  ],
  blocks: [
    {
      blockId: "block-1",
      base: "base\n",
      existing: "ours\n",
      incoming: "theirs\n",
      resolutionMethods: [
        { method: "existing", scope: "block" },
        { method: "incoming", scope: "block" },
        { method: "both", scope: "block" },
        { method: "manual", scope: "block" }
      ]
    }
  ],
  segments: [
    { kind: "text", text: "before\n" },
    { kind: "block", blockId: "block-1" },
    { kind: "text", text: "after\n" }
  ],
  fields: {
    strategy: "textBlocks"
  }
};

const unsupported: CanonicalLensReconcileResult = {
  status: "unsupported",
  summary: "Lens cannot reconcile this media type",
  resolutionMethods: [],
  blocks: [],
  segments: [],
  fields: {
    reasonCode: "mediaType"
  }
};

function response(result: CanonicalLensReconcileResult): LensReconcileResponse {
  return {
    contractVersion: LENS_CONTRACT_VERSION,
    lens: { ...pin },
    result
  };
}

export const contractFixture = {
  manifest,
  previewWindow,
  diffWindow,
  reconcileInput,
  reconcileResults: {
    autoResolved: response(autoResolved),
    conflicted: response(conflicted),
    unsupported: response(unsupported)
  }
};
