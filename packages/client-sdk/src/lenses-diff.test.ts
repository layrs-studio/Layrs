import { createTextDiffModel, normalizeDiffColumnWindow } from "./index";

testTextDiffModelAddsRenderMetadata();
testTextDiffModelKeepsLegacyFields();
testTextDiffModelSegmentsColumns();
testDiffColumnWindowNormalizesSnakeCase();

function testTextDiffModelAddsRenderMetadata(): void {
  const diff = createTextDiffModel({
    oldText: "a\nb\nc",
    newText: "a\nbeta\nc",
    metadata: {
      totalLineCount: 1200,
      lineWindow: {
        startLine: 401,
        endLine: 800
      },
      hasMoreBefore: true,
      hasMoreAfter: true,
      truncated: true,
      virtualization: {
        strategy: "serverWindow",
        maxRenderedLineCount: 400
      }
    }
  });

  assertEqual(diff.metadata?.totalLineCount, 1200, "metadata exposes total line count");
  assertEqual(diff.metadata?.renderedLineCount, 4, "metadata exposes rendered line count");
  assertEqual(diff.metadata?.lineWindow?.startLine, 401, "metadata exposes line window start");
  assertEqual(diff.metadata?.lineWindow?.endLine, 800, "metadata exposes line window end");
  assertEqual(diff.metadata?.hasMoreBefore, true, "metadata exposes previous window availability");
  assertEqual(diff.metadata?.hasMoreAfter, true, "metadata exposes next window availability");
  assertEqual(diff.metadata?.truncated, true, "metadata exposes truncation");
  assertEqual(
    diff.metadata?.virtualization?.strategy,
    "serverWindow",
    "metadata keeps virtualization strategy"
  );
}

function testTextDiffModelKeepsLegacyFields(): void {
  const diff = createTextDiffModel({
    oldText: "one",
    newText: "one\ntwo",
    lineWindow: {
      startLine: 1,
      endLine: 2
    },
    fields: {
      path: "note.txt"
    }
  });

  assertEqual(diff.fields.path, "note.txt", "custom fields are preserved");
  assertEqual(diff.fields.additions, 1, "legacy additions field is preserved");
  assertEqual(diff.fields.deletions, 0, "legacy deletions field is preserved");
  assertEqual(diff.fields.totalLineCount, 2, "fields mirror total line count");
  assertEqual(diff.fields.renderedLineCount, 2, "fields mirror rendered line count");
  assertEqual(
    (diff.fields.lineWindow as { startLine?: number }).startLine,
    1,
    "fields mirror line window"
  );
}

function testTextDiffModelSegmentsColumns(): void {
  const diff = createTextDiffModel({
    oldText: "abcdefg",
    newText: "abcdefg",
    columnWindow: {
      columnStart: 2,
      columnEnd: 5
    }
  });
  const line = diff.hunks[0]?.lines[0];

  assertEqual(diff.metadata?.columnWindow?.columnStart, 2, "metadata exposes column window start");
  assertEqual(diff.metadata?.columnWindow?.columnEnd, 5, "metadata exposes column window end");
  assertEqual(diff.fields.hasMoreColumns, true, "fields expose column overflow");
  assertEqual(line?.text, "abcdefg", "legacy full text is preserved");
  assertEqual(line?.textSegment, "cde", "line exposes text segment");
  assertEqual(line?.textLength, 7, "line exposes full text length");
  assertEqual(line?.columnStart, 2, "line exposes column start");
  assertEqual(line?.columnEnd, 5, "line exposes column end");
  assertEqual(line?.hasMoreColumns, true, "line exposes column overflow");
}

function testDiffColumnWindowNormalizesSnakeCase(): void {
  const columnWindow = normalizeDiffColumnWindow({
    column_start: "10",
    column_end: "30",
    text_length: "80"
  });

  assertEqual(columnWindow?.columnStart, 10, "snake case column start normalizes");
  assertEqual(columnWindow?.columnEnd, 30, "snake case column end normalizes");
  assertEqual(columnWindow?.textLength, 80, "snake case text length normalizes");
  assertEqual(columnWindow?.hasMoreColumns, true, "column overflow is inferred");
}

function assertEqual(actual: unknown, expected: unknown, message: string): void {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}
