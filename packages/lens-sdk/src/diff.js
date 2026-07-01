"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.createTextDiffModel = createTextDiffModel;
exports.normalizeDiffColumnWindow = normalizeDiffColumnWindow;
exports.diffTextLines = diffTextLines;
exports.getTextDiffStats = getTextDiffStats;
function createTextDiffModel(input) {
    const oldLines = splitTextLines(input.oldText);
    const newLines = splitTextLines(input.newText);
    const columnWindow = input.metadata?.columnWindow ??
        input.columnWindow ??
        normalizeDiffColumnWindow(input.fields?.columnWindow);
    const lines = diffTextLines(oldLines, newLines).map((line) => columnWindow ? applyDiffColumnWindow(line, columnWindow) : line);
    const stats = getTextDiffStats(lines);
    const renderedLineCount = lines.length;
    const lineWindow = input.metadata?.lineWindow ?? input.lineWindow;
    const totalLineCount = input.metadata?.totalLineCount ?? renderedLineCount;
    const hasMoreColumns = input.metadata?.hasMoreColumns ??
        columnWindow?.hasMoreColumns ??
        lines.some((line) => line.hasMoreColumns);
    const metadata = {
        ...input.metadata,
        totalLineCount,
        totalDiffLineCount: input.metadata?.totalDiffLineCount ?? renderedLineCount,
        renderedLineCount,
        ...(lineWindow ? { lineWindow } : {}),
        ...(columnWindow ? { columnWindow } : {}),
        ...(hasMoreColumns !== undefined ? { hasMoreColumns } : {}),
        virtualization: {
            strategy: "clientWindow",
            ...(columnWindow ? { maxRenderedColumnCount: Math.max(1, columnWindow.columnEnd - columnWindow.columnStart) } : {}),
            ...input.metadata?.virtualization
        }
    };
    return {
        kind: "textLines",
        summary: input.summary ?? summarizeTextDiff(stats),
        metadata,
        hunks: [
            {
                oldStart: 1,
                oldLines: oldLines.length,
                newStart: 1,
                newLines: newLines.length,
                lines
            }
        ],
        fields: {
            ...input.fields,
            additions: stats.additions,
            deletions: stats.deletions,
            unchanged: stats.unchanged,
            totalLineCount: metadata.totalLineCount,
            totalDiffLineCount: metadata.totalDiffLineCount,
            renderedLineCount: metadata.renderedLineCount,
            ...(metadata.lineWindow ? { lineWindow: metadata.lineWindow } : {}),
            ...(metadata.columnWindow ? { columnWindow: metadata.columnWindow } : {}),
            ...(metadata.hasMoreBefore !== undefined ? { hasMoreBefore: metadata.hasMoreBefore } : {}),
            ...(metadata.hasMoreAfter !== undefined ? { hasMoreAfter: metadata.hasMoreAfter } : {}),
            ...(metadata.hasMoreColumns !== undefined ? { hasMoreColumns: metadata.hasMoreColumns } : {}),
            ...(metadata.truncated !== undefined ? { truncated: metadata.truncated } : {}),
            virtualization: metadata.virtualization,
            ...(input.language ? { language: input.language } : {}),
            ...(input.mediaType ? { mediaType: input.mediaType } : {}),
            ...(input.path ? { path: input.path } : {})
        }
    };
}
function normalizeDiffColumnWindow(value) {
    const record = recordValue(value);
    if (!record) {
        return undefined;
    }
    const columnStart = getNonNegativeInteger(record.columnStart ?? record.column_start ?? record.startColumn ?? record.start_column ?? record.start) ??
        0;
    const explicitColumnEnd = getNonNegativeInteger(record.columnEnd ?? record.column_end ?? record.endColumn ?? record.end_column ?? record.end);
    const limit = getPositiveInteger(record.columnLimit ?? record.column_limit ?? record.limit);
    const textLength = getNonNegativeInteger(record.textLength ?? record.text_length ?? record.totalColumns ?? record.total_columns ?? record.totalLength ?? record.total_length);
    const columnEnd = explicitColumnEnd ??
        (limit !== undefined ? columnStart + limit : textLength);
    if (columnEnd === undefined) {
        return undefined;
    }
    const normalizedColumnEnd = Math.max(columnStart, columnEnd);
    const hasMoreColumns = getBooleanValue(record.hasMoreColumns ?? record.has_more_columns) ??
        (textLength !== undefined ? columnStart > 0 || normalizedColumnEnd < textLength : undefined);
    return {
        columnStart,
        columnEnd: normalizedColumnEnd,
        ...(textLength !== undefined ? { textLength } : {}),
        ...(hasMoreColumns !== undefined ? { hasMoreColumns } : {})
    };
}
function diffTextLines(oldLines, newLines) {
    const lcs = createLcsTable(oldLines, newLines);
    const diff = [];
    let oldIndex = 0;
    let newIndex = 0;
    while (oldIndex < oldLines.length || newIndex < newLines.length) {
        if (oldIndex < oldLines.length && newIndex < newLines.length && oldLines[oldIndex] === newLines[newIndex]) {
            diff.push({
                op: "equal",
                oldLine: oldIndex + 1,
                newLine: newIndex + 1,
                text: oldLines[oldIndex]
            });
            oldIndex += 1;
            newIndex += 1;
            continue;
        }
        if (oldIndex < oldLines.length &&
            (newIndex >= newLines.length || lcs[oldIndex + 1][newIndex] >= lcs[oldIndex][newIndex + 1])) {
            diff.push({
                op: "delete",
                oldLine: oldIndex + 1,
                text: oldLines[oldIndex]
            });
            oldIndex += 1;
            continue;
        }
        if (newIndex < newLines.length) {
            diff.push({
                op: "insert",
                newLine: newIndex + 1,
                text: newLines[newIndex]
            });
            newIndex += 1;
        }
    }
    return diff;
}
function getTextDiffStats(lines) {
    return lines.reduce((stats, line) => {
        if (line.op === "insert") {
            stats.additions += 1;
        }
        else if (line.op === "delete") {
            stats.deletions += 1;
        }
        else {
            stats.unchanged += 1;
        }
        return stats;
    }, { additions: 0, deletions: 0, unchanged: 0 });
}
function splitTextLines(text) {
    return text.length === 0 ? [] : text.split(/\r\n|\n|\r/);
}
function createLcsTable(oldLines, newLines) {
    const table = Array.from({ length: oldLines.length + 1 }, () => new Array(newLines.length + 1).fill(0));
    for (let oldIndex = oldLines.length - 1; oldIndex >= 0; oldIndex -= 1) {
        for (let newIndex = newLines.length - 1; newIndex >= 0; newIndex -= 1) {
            table[oldIndex][newIndex] =
                oldLines[oldIndex] === newLines[newIndex]
                    ? table[oldIndex + 1][newIndex + 1] + 1
                    : Math.max(table[oldIndex + 1][newIndex], table[oldIndex][newIndex + 1]);
        }
    }
    return table;
}
function summarizeTextDiff(stats) {
    if (stats.additions === 0 && stats.deletions === 0) {
        return "No text changes";
    }
    return `${stats.additions} additions, ${stats.deletions} deletions`;
}
function applyDiffColumnWindow(line, columnWindow) {
    const chars = Array.from(line.text);
    const textLength = chars.length;
    const columnStart = Math.min(Math.max(0, columnWindow.columnStart), textLength);
    const columnEnd = Math.min(Math.max(columnStart, columnWindow.columnEnd), textLength);
    const hasMoreColumns = columnWindow.hasMoreColumns ?? (columnStart > 0 || columnEnd < textLength);
    return {
        ...line,
        textSegment: chars.slice(columnStart, columnEnd).join(""),
        textLength,
        columnStart,
        columnEnd,
        hasMoreColumns
    };
}
function recordValue(value) {
    return value && typeof value === "object" && !Array.isArray(value) ? value : undefined;
}
function getPositiveInteger(value) {
    const numberValue = getFiniteNumber(value);
    return numberValue !== undefined && numberValue > 0 ? Math.floor(numberValue) : undefined;
}
function getNonNegativeInteger(value) {
    const numberValue = getFiniteNumber(value);
    return numberValue !== undefined && numberValue >= 0 ? Math.floor(numberValue) : undefined;
}
function getFiniteNumber(value) {
    if (typeof value === "number" && Number.isFinite(value)) {
        return value;
    }
    if (typeof value === "string" && value.trim().length > 0) {
        const parsed = Number(value);
        return Number.isFinite(parsed) ? parsed : undefined;
    }
    return undefined;
}
function getBooleanValue(value) {
    return typeof value === "boolean" ? value : undefined;
}
