import {
  createPreviewModelFromArtifactContent,
  normalizeArtifactContentPayload,
  normalizeArtifactPreviewWindowPayload,
  normalizeLayerStep,
  normalizeStepDiffWindow
} from "./index";

const pngHeaderBase64 = "iVBORw0KGgo=";

testBase64Text();
testBase64Image();
testBase64Raw();
testHexTextFallback();
testHexImageFallback();
testWindowedPreviewDiff();
testStepDiffWindowNormalizesLensMetadata();

function testBase64Text(): void {
  const payload = normalizeArtifactContentPayload({
    path: "docs/readme.md",
    content: {
      encoding: "base64",
      mediaType: "text/markdown",
      value: "SGVsbG8gTGF5cnM="
    }
  });
  assert(payload, "base64 text payload normalizes");
  assertEqual(payload.content.value, "Hello Layrs", "base64 text is decoded to utf8 text");
  assertEqual(payload.content.base64, "SGVsbG8gTGF5cnM=", "base64 source is preserved");
  assertEqual(payload.content.bytes?.byteLength, 11, "base64 text exposes decoded byte length");

  const preview = createPreviewModelFromArtifactContent({ payload });
  assert(preview, "base64 text preview is available");
  assertEqual(preview.body, "Hello Layrs", "base64 text preview body is decoded text");
  assertEqual(preview.kind, "text", "base64 text preview kind is text");
}

function testBase64Image(): void {
  const payload = normalizeArtifactContentPayload({
    path: "assets/logo.png",
    content: {
      encoding: "base64",
      mediaType: "image/png",
      value: pngHeaderBase64
    }
  });
  assert(payload, "base64 image payload normalizes");
  assertEqual(payload.content.base64, pngHeaderBase64, "base64 image source is preserved");
  assertEqual(payload.content.value, `data:image/png;base64,${pngHeaderBase64}`, "base64 image becomes data url");

  const preview = createPreviewModelFromArtifactContent({ payload });
  assert(preview, "base64 image preview is available");
  assertEqual(preview.kind, "image", "base64 image preview kind is image");
  assertEqual(preview.fields.dataUrl, `data:image/png;base64,${pngHeaderBase64}`, "base64 image preview exposes data url");
  assertEqual(preview.fields.data, pngHeaderBase64, "base64 image preview exposes base64 data");
}

function testBase64Raw(): void {
  const payload = normalizeArtifactContentPayload({
    path: "bin/blob.dat",
    content: {
      encoding: "base64",
      mediaType: "application/octet-stream",
      value: "AQIDBA=="
    }
  });
  assert(payload, "base64 raw payload normalizes");
  assertEqual(payload.content.value, "AQIDBA==", "base64 raw content stays base64, not binary text");
  assertEqual(payload.content.bytes?.byteLength, 4, "base64 raw exposes decoded bytes");

  const preview = createPreviewModelFromArtifactContent({ payload });
  assert(preview, "base64 raw preview is available");
  assertEqual(preview.kind, "raw", "base64 raw preview kind is raw");
  assertEqual(preview.fields.data, "AQIDBA==", "base64 raw preview exposes base64 data");
}

function testHexTextFallback(): void {
  const payload = normalizeArtifactContentPayload({
    path: "docs/legacy.txt",
    content: {
      encoding: "hex",
      mediaType: "text/plain",
      value: "4865782066616c6c6261636b"
    }
  });
  assert(payload, "hex text payload normalizes");
  assertEqual(payload.content.value, "Hex fallback", "hex text fallback decodes to utf8 text");
  assertEqual(payload.content.base64, "SGV4IGZhbGxiYWNr", "hex text fallback preserves derived base64");
}

function testHexImageFallback(): void {
  const payload = normalizeArtifactContentPayload({
    path: "assets/legacy.png",
    content: {
      encoding: "hex",
      mediaType: "image/png",
      value: "89504e470d0a1a0a"
    }
  });
  assert(payload, "hex image payload normalizes");
  assertEqual(payload.content.base64, pngHeaderBase64, "hex image fallback derives base64");
  assertEqual(payload.content.value, `data:image/png;base64,${pngHeaderBase64}`, "hex image fallback becomes data url");
}

function testWindowedPreviewDiff(): void {
  const payload = normalizeArtifactPreviewWindowPayload({
    artifactId: "artifact_1",
    path: "src/main.ts",
    window: {
      start: 400,
      limit: 400,
      count: 2,
      totalLines: 1000,
      hasMore: true
    },
    preview: {
      kind: "code",
      title: "src/main.ts",
      body: "line 401\nline 402",
      mediaType: "application/typescript",
      fields: {
        lines: ["line 401", "line 402"]
      }
    },
    diff: {
      kind: "textLines",
      summary: "Windowed artifact preview",
      hunks: [
        {
          oldStart: 401,
          oldLines: 2,
          newStart: 401,
          newLines: 2,
          lines: [
            { op: "equal", oldLine: 401, newLine: 401, text: "line 401" },
            { op: "equal", oldLine: 402, newLine: 402, text: "line 402" }
          ]
        }
      ],
      fields: {
        windowed: true
      }
    }
  });

  assert(payload, "windowed preview payload normalizes");
  assertEqual(payload.window.start, 400, "window start is preserved");
  assertEqual(payload.window.hasMore, true, "window hasMore is preserved");
  assertEqual(payload.preview?.kind, "code", "window preview kind is preserved");
  assertEqual(payload.diff?.hunks[0]?.lines.length, 2, "window diff lines normalize");
}

function testStepDiffWindowNormalizesLensMetadata(): void {
  const diffWindowWire = {
    path: "src/main.ts",
    state: "modified",
    lensId: "layrs.code",
    title: "Modified src/main.ts",
    diff: {
      kind: "textLines",
      summary: "Windowed step diff",
      hunks: [
        {
          oldStart: 11,
          oldLines: 1,
          newStart: 11,
          newLines: 1,
          lines: [
            {
              op: "equal",
              oldLine: 11,
              newLine: 11,
              textSegment: "const value = 1;",
              textLength: 120,
              columnStart: 40,
              columnEnd: 56,
              hasMoreColumns: true
            }
          ]
        }
      ],
      fields: {
        source: "localStep:step-1",
        layerId: "layer-1",
        stepId: "step-1",
        totalDiffLines: 80,
        windowStart: 10,
        windowEnd: 11,
        windowLimit: 1,
        hasMore: true,
        columnWindow: {
          columnStart: 40,
          columnEnd: 56,
          textLength: 120,
          hasMoreColumns: true
        },
        virtualization: {
          maxRenderedLineCount: 400,
          maxRenderedColumnCount: 16
        }
      }
    }
  };

  const diffWindow = normalizeStepDiffWindow(diffWindowWire);
  assert(diffWindow, "step diff window normalizes");
  assertEqual(diffWindow.path, "src/main.ts", "step diff path normalizes");
  assertEqual(diffWindow.source, "localStep:step-1", "step diff source normalizes");
  assertEqual(diffWindow.lineWindow?.startLine, 11, "legacy window start becomes line window");
  assertEqual(diffWindow.totalDiffLineCount, 80, "total diff lines normalize");
  assertEqual(diffWindow.hasMoreBefore, true, "previous line window is inferred");
  assertEqual(diffWindow.hasMoreAfter, true, "next line window is preserved");
  assertEqual(diffWindow.columnWindow?.columnStart, 40, "column window normalizes");
  assertEqual(diffWindow.hasMoreColumns, true, "column overflow normalizes");
  assertEqual(diffWindow.diff.metadata?.virtualization?.maxRenderedColumnCount, 16, "column render cap normalizes");
  assertEqual(diffWindow.diff.hunks[0]?.lines[0]?.text, "const value = 1;", "text segment fills text fallback");
  assertEqual(diffWindow.diff.hunks[0]?.lines[0]?.textSegment, "const value = 1;", "text segment is preserved");

  const layerStep = normalizeLayerStep({
    step_id: "step-1",
    layer_id: "layer-1",
    captured_at: 12,
    changed_files: 1,
    diff_stats: {
      files: 1,
      additions: 2,
      deletions: 1
    },
    diffs: [diffWindowWire]
  });
  assert(layerStep, "layer step normalizes");
  assertEqual(layerStep.stepId, "step-1", "layer step id normalizes");
  assertEqual(layerStep.changedFiles, 1, "layer step changed files normalize");
  assertEqual(layerStep.diffStats.deletions, 1, "layer step diff stats normalize");
  assertEqual(layerStep.diffs.length, 1, "layer step diff windows normalize");
}

function assert(value: unknown, message: string): asserts value {
  if (!value) {
    throw new Error(message);
  }
}

function assertEqual(actual: unknown, expected: unknown, message: string): void {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}
